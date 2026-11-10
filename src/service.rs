use std::{
    io::Read,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use anyhow::{Context, Result, bail, ensure};
use tokio::sync::{Mutex, Notify, RwLock, Semaphore};
use whatsapp_rust::{
    Client, HistorySyncDurabilityHook, HistorySyncKey, HistorySyncMetadata, InboundDurabilityHook,
    prelude::*,
};

use crate::{
    config::Config,
    model::{Connection, Message, Phase, Snapshot, now},
    storage::{Archive, StoredMessage, outgoing, project_message},
};

pub struct Service {
    pub(crate) live_calls: crate::calling::Calls,
    pub(crate) read_slots: Semaphore,
    pub(crate) read_jobs: std::sync::Mutex<std::collections::HashSet<(String, i64)>>,
    pub(crate) outbox_locks:
        Mutex<std::collections::HashMap<(String, String), std::sync::Weak<Mutex<()>>>>,
    pub archive: Archive,
    pub(crate) config: Config,
    pub(crate) settings: RwLock<crate::settings::Settings>,
    pub(crate) updates: RwLock<crate::updates::UpdateState>,
    pub(crate) update_lock: Mutex<()>,
    pub(crate) avatar_warming: AtomicBool,
    pub(crate) notifications_ready: AtomicBool,
    pub(crate) chat_locks_ready: AtomicBool,
    pub(crate) connected_at: AtomicU64,
    pub(crate) notification_bursts: Mutex<std::collections::HashMap<String, std::time::Instant>>,
    pub(crate) logout_in_progress: AtomicBool,
    reset_session: AtomicBool,
    pub(crate) logout_request: Mutex<Option<tokio::sync::oneshot::Sender<Result<(), String>>>>,
    pub(crate) connection: RwLock<Connection>,
    pub(crate) client: RwLock<Option<Arc<Client>>>,
    revision: Arc<AtomicU64>,
    pub(crate) restart: Notify,
    phone: Mutex<Option<String>>,
    media_slots: Arc<Semaphore>,
    pub(crate) stopping: AtomicBool,
    pub(crate) history_prefetch: crate::history_prefetch::Queue,
    pub(crate) history_lock: Mutex<()>,
    pub(crate) desktop: Mutex<crate::notifications::Desktop>,
    media_jobs: std::sync::Mutex<std::collections::HashSet<String>>,
}

impl Service {
    pub async fn open(config: &Config) -> Result<Arc<Self>> {
        if config.demo {
            crate::demo::prepare(config)?;
        }
        let archive = Archive::open(&config.database()).await?;
        if config.demo {
            crate::demo::seed(&archive)
                .await
                .context("Populate offline demo profile")?;
        }
        // A confirmed unlink may have been interrupted before local cleanup.
        // No SDK client exists yet, so this is a safe storage reset boundary.
        if archive
            .preference("session_reset_pending".into())
            .await?
            .as_deref()
            == Some("1")
        {
            archive.clear_read_sync().await?;
            archive.database.reset_device(archive.account).await?;
            archive
                .set_preference("session_reset_pending".into(), "0".into())
                .await?;
        }
        archive.recover_live_calls().await?;
        archive.recover_pending_sends().await?;
        let settings = archive.app_settings().await?;
        // Load and validate the library's saved device, rather than guessing
        // authentication from a database file or the presence of archived chats.
        use whatsapp_rust::wacore::store::traits::DeviceStore;
        let has_session = config.demo
            || DeviceStore::load(archive.backend.as_ref())
                .await?
                .is_some_and(|device| device.pn.is_some() && device.account.is_some());
        let updates =
            if config.demo && std::env::var("BRAZAP_DEMO_UPDATE").as_deref() != Ok("check") {
                crate::demo::updates()
            } else {
                let config = config.clone();
                tokio::task::spawn_blocking(move || crate::updates::restored(&config)).await?
            };
        Ok(Arc::new(Self {
            live_calls: Default::default(),
            read_slots: Semaphore::new(2),
            read_jobs: Default::default(),
            outbox_locks: Mutex::new(Default::default()),
            config: config.clone(),
            settings: RwLock::new(settings),
            updates: RwLock::new(updates),
            update_lock: Mutex::new(()),
            avatar_warming: AtomicBool::new(false),
            notifications_ready: AtomicBool::new(false),
            chat_locks_ready: AtomicBool::new(false),
            connected_at: AtomicU64::new(0),
            notification_bursts: Mutex::new(Default::default()),
            logout_in_progress: AtomicBool::new(false),
            reset_session: AtomicBool::new(false),
            logout_request: Mutex::new(None),
            archive,
            connection: RwLock::new(if config.demo {
                crate::demo::connection()
            } else {
                Connection {
                    has_session,
                    ..Default::default()
                }
            }),
            client: RwLock::new(None),
            revision: Arc::new(AtomicU64::new(0)),
            restart: Notify::new(),
            phone: Mutex::new(None),
            media_slots: Arc::new(Semaphore::new(2)),
            stopping: AtomicBool::new(false),
            history_prefetch: Default::default(),
            history_lock: Mutex::new(()),
            desktop: Mutex::new(crate::notifications::Desktop::default()),
            media_jobs: std::sync::Mutex::new(std::collections::HashSet::new()),
        }))
    }

    pub(crate) fn changed(&self) {
        self.revision.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn revision(&self) -> u64 {
        self.revision.load(Ordering::Relaxed)
    }

    pub async fn run(self: Arc<Self>) {
        if !self.config.demo || std::env::var("BRAZAP_DEMO_UPDATE").as_deref() == Ok("check") {
            self.spawn_update_checker();
        }
        if self.config.demo {
            while !self.stopping.load(Ordering::Relaxed) {
                self.restart.notified().await;
            }
            return;
        }
        if let Err(error) = self.archive.restore_chat_settings().await {
            log::warn!("Could not restore chat settings: {error}");
        }
        if let Err(error) = self.archive.restore_contact_names().await {
            log::warn!("Could not restore contact names: {error}");
        }
        if let Err(error) = self.archive.backfill_calls().await {
            log::warn!("Could not restore cached call history: {error}");
        }
        if let Err(error) = self.archive.recover_archived_history().await {
            log::warn!("History recovery failed: {error}");
        }
        match self.archive.rebuild_projections().await {
            // The open conversation may have been read while the upgrade was
            // still rewriting messages; reload it with the new projection.
            Ok(true) => self.changed(),
            Ok(false) => {}
            Err(error) => log::warn!("Message projection upgrade failed: {error}"),
        }
        if let Err(error) = self.archive.backfill_call_messages().await {
            log::warn!("Call message projection upgrade failed: {error}");
        }
        if let Err(error) = self.archive.backfill_deletions().await {
            log::warn!("Could not restore deletion markers: {error}");
        }
        self.spawn_history_prefetch();
        self.spawn_storage_maintenance();
        loop {
            if self.stopping.load(Ordering::Relaxed) {
                break;
            }
            if self.reset_session.load(Ordering::Acquire) {
                if let Err(error) = self.reset_unlinked_session().await {
                    self.connection.write().await.notice =
                        Some(format!("Could not prepare a fresh session: {error}"));
                    self.changed();
                    tokio::select! {
                        _=tokio::time::sleep(std::time::Duration::from_secs(3)) => {},
                        _=self.restart.notified() => {},
                    }
                    continue;
                }
            }
            let weak = Arc::downgrade(&self);
            let hooks = Persistence {
                archive: self.archive.clone(),
                revision: self.revision.clone(),
            };
            let mut sync = whatsapp_rust::wacore::store::device::default_history_sync_config();
            sync.full_sync_days_limit = Some(365);
            sync.on_demand_ready = Some(true);
            sync.complete_on_demand_ready = Some(true);
            let props = whatsapp_rust::wacore::store::DevicePropsOverride::new()
                .with_os("Brazap")
                .with_platform_type(wa::device_props::PlatformType::UWP)
                .with_require_full_sync(true)
                .with_history_sync_config(sync);
            let built = Bot::builder()
                .with_device_props(props)
                .with_presence_policy(if self.settings.read().await.privacy.show_online {
                    whatsapp_rust::PresencePolicy::Automatic
                } else {
                    whatsapp_rust::PresencePolicy::Manual
                })
                .with_backend_arc(self.archive.backend.clone())
                .with_inbound_durability_hook(hooks.clone())
                .with_history_sync_durability_hook(hooks)
                .on_event(move |event, client| {
                    let weak = weak.clone();
                    async move {
                        if let Some(service) = weak.upgrade()
                            && let Err(error) = service.on_event(event, client).await
                        {
                            log::error!("Could not process WhatsApp event: {error:#}");
                            service.connection.write().await.notice =
                                Some(format!("Could not save an update: {error}"));
                            service.changed();
                        }
                    }
                })
                .build()
                .await;
            match built {
                Ok(bot) => {
                    *self.client.write().await = Some(bot.client());
                    let client = bot.client();
                    let released = client.store_release();
                    let mut handle = bot.spawn();
                    tokio::select! {
                        outcome=&mut handle => { log::info!("WhatsApp connection ended: {outcome:?}"); }
                        _=self.restart.notified() => {}
                    }
                    let logout = self.logout_request.lock().await.take();
                    let mut logout_result = Ok(());
                    if logout.is_some() {
                        let report = client.logout().await;
                        if let Err(error) = report.shutdown.device {
                            log::error!("Could not flush the session during logout: {error}");
                        }
                        match report.deregistration {
                            whatsapp_rust::DeregistrationOutcome::Confirmed => {
                                self.reset_session.store(true, Ordering::Release);
                                if let Err(error) = self
                                    .archive
                                    .set_preference("session_reset_pending".into(), "1".into())
                                    .await
                                {
                                    log::error!(
                                        "Could not record pending session cleanup: {error}"
                                    );
                                }
                                log::info!("WhatsApp confirmed device unlinking");
                            }
                            outcome => {
                                log::warn!(
                                    "WhatsApp did not confirm device unlinking: {outcome:?}"
                                );
                                logout_result = Err("WhatsApp did not confirm unlinking. Your login was kept; reconnect and try again.".into());
                            }
                        }
                    }
                    let report = handle.shutdown().await;
                    if let Err(error) = report.shutdown.device {
                        log::error!("Could not flush the WhatsApp session: {error}");
                    }
                    *self.client.write().await = None;
                    drop(client);
                    // Shutdown is not a storage-ownership fence. In particular,
                    // detached savers must finish before fresh keys are written.
                    let wait = released.wait();
                    tokio::pin!(wait);
                    tokio::select! {
                        _=&mut wait => {},
                        _=tokio::time::sleep(std::time::Duration::from_secs(15)) => {
                            log::warn!("Waiting for the old WhatsApp client to release session storage");
                            self.connection.write().await.notice = Some("Finishing the previous connection…".into());
                            self.changed();
                            wait.await;
                        }
                    }
                    if self.reset_session.load(Ordering::Acquire) {
                        if let Err(error) = self.reset_unlinked_session().await {
                            logout_result = Err(format!(
                                "Device unlinked, but local session cleanup needs to retry: {error}"
                            ));
                        }
                    } else if !self.stopping.load(Ordering::Relaxed) {
                        let mut state = self.connection.write().await;
                        *state = Connection {
                            // A failed unlink keeps the stored login and
                            // reconnects; the pending state hid it from the UI.
                            has_session: logout.is_some() || state.has_session,
                            initial_sync: logout.is_none() && state.initial_sync,
                            qr: state.qr.take(),
                            qr_expires_at: state.qr_expires_at,
                            notice: logout_result.as_ref().err().cloned(),
                            ..Default::default()
                        };
                    }
                    if let Some(reply) = logout {
                        self.logout_in_progress.store(false, Ordering::Release);
                        let _ = reply.send(logout_result);
                    }
                }
                Err(error) => {
                    let mut state = self.connection.write().await;
                    state.phase = Phase::Error;
                    state.notice = Some(format!("Could not start WhatsApp: {error}"));
                }
            }
            *self.client.write().await = None;
            self.changed();
            if self.stopping.load(Ordering::Relaxed) {
                break;
            }
            tokio::select! {
                _=tokio::time::sleep(std::time::Duration::from_secs(3)) => {}
                _=self.restart.notified() => {}
            }
        }
    }

    async fn refresh_chat_names(&self) -> Result<()> {
        let chats = self.archive.chats().await?;
        let missing: std::collections::HashSet<_> = chats
            .iter()
            .filter(|chat| {
                chat.jid.ends_with("@g.us")
                    && !crate::storage::meaningful_name(&chat.jid, &chat.name)
            })
            .map(|chat| chat.jid.clone())
            .collect();
        let client = self.connected().await?;
        if !missing.is_empty() {
            match client.groups().list_participating().await {
                Ok(groups) => {
                    for group in groups {
                        if missing.contains(&group.id.to_string()) {
                            if let Some(subject) = group.subject.filter(|s| !s.trim().is_empty()) {
                                self.archive
                                    .name_chat(group.id.to_string(), subject, 0, 0)
                                    .await?;
                            }
                        }
                    }
                }
                Err(error) => log::debug!("Group names refresh: {error}"),
            }
        }
        for chat in chats.iter().filter(|chat| {
            chat.jid.ends_with("@newsletter")
                && !crate::storage::meaningful_name(&chat.jid, &chat.name)
        }) {
            let jid = chat.jid.parse()?;
            if let Ok(Ok(channel)) = tokio::time::timeout(
                std::time::Duration::from_secs(10),
                client.newsletter().get_metadata(&jid),
            )
            .await
            {
                if !channel.name.trim().is_empty() {
                    self.archive
                        .name_chat(chat.jid.clone(), channel.name, 0, 0)
                        .await?;
                }
            }
        }
        self.changed();
        Ok(())
    }

    async fn reset_unlinked_session(&self) -> Result<()> {
        self.archive.clear_read_sync().await?;
        self.chat_locks_ready.store(false, Ordering::Release);
        // Call only before starting a client, or after its store_release fence.
        self.archive.preserve_identity_links().await?;
        self.archive
            .database
            .reset_device(self.archive.account)
            .await?;
        self.archive
            .set_preference("session_reset_pending".into(), "0".into())
            .await?;
        self.reset_session.store(false, Ordering::Release);
        *self.phone.lock().await = None;
        *self.connection.write().await = Connection {
            phase: Phase::LoggedOut,
            ..Default::default()
        };
        self.changed();
        Ok(())
    }

    async fn on_event(self: &Arc<Self>, event: Arc<Event>, client: Arc<Client>) -> Result<()> {
        if self.logout_in_progress.load(Ordering::Acquire)
            && matches!(
                &*event,
                Event::Connected(_)
                    | Event::PairSuccess(_)
                    | Event::PairingQrCode(_)
                    | Event::PairingCode(_)
                    | Event::OfflineSyncPreview(_)
                    | Event::OfflineSyncCompleted(_)
            )
        {
            return Ok(());
        }
        match &*event {
            Event::Connected(_) => {
                self.notifications_ready.store(false, Ordering::Release);
                self.connected_at.store(now() as u64, Ordering::Release);
                {
                    let mut state = self.connection.write().await;
                    let initial_sync = state.initial_sync;
                    *state = Connection {
                        phase: Phase::Connected,
                        has_session: true,
                        initial_sync,
                        syncing: initial_sync,
                        ..Default::default()
                    };
                }
                let weak = Arc::downgrade(self);
                let ready_client = client.clone();
                tokio::spawn(async move {
                    let ready = ready_client
                        .wait_for_startup_sync(std::time::Duration::from_secs(120))
                        .await
                        .is_ok();
                    if let Some(service) = weak.upgrade() {
                        let current = service.client.read().await;
                        if current
                            .as_ref()
                            .is_some_and(|c| Arc::ptr_eq(c, &ready_client))
                        {
                            let mut state = service.connection.write().await;
                            state.initial_sync = false;
                            state.syncing = false;
                            if ready {
                                state.sync_progress = None;
                            }
                            service.changed();
                        }
                    }
                    if ready {
                        if let Some(service) = weak.upgrade() {
                            if !service.chat_locks_ready.load(Ordering::Acquire) {
                                // The app previously ignored lock mutations. Replay this
                                // collection once per process so cached SDK versions cannot
                                // leave the application unaware of the phone's locks.
                                let synced=ready_client.resync_app_state(
                                    [whatsapp_rust::wacore::appstate::patch_decode::WAPatchName::RegularLow],
                                    whatsapp_rust::AppStateResyncMode::Snapshot,
                                ).await.is_ok_and(|report|report.all_synced());
                                if synced {
                                    service.chat_locks_ready.store(true, Ordering::Release);
                                } else {
                                    log::warn!(
                                        "Could not refresh all chat privacy state; automatic history remains paused. Notifications use archived chat visibility."
                                    );
                                }
                            }
                            let current = service.client.read().await;
                            if current
                                .as_ref()
                                .is_some_and(|c| Arc::ptr_eq(c, &ready_client))
                            {
                                service.notifications_ready.store(true, Ordering::Release);
                                service.history_prefetch.wake();
                                service.changed();
                            }
                            drop(current);
                            if service
                                .archive
                                .preference("phone_contacts_synced_v1".into())
                                .await
                                .ok()
                                .flatten()
                                .is_none()
                            {
                                match ready_client.resync_app_state(
                                    [whatsapp_rust::wacore::appstate::patch_decode::WAPatchName::CriticalUnblockLow],
                                    whatsapp_rust::AppStateResyncMode::Snapshot,
                                ).await {
                                    Ok(report) if report.all_synced() => {
                                        if let Err(error) = service.archive.set_preference("phone_contacts_synced_v1".into(),"1".into()).await { log::warn!("Could not save contact sync coverage: {error}"); }
                                        service.changed();
                                    }
                                    result => log::debug!("Phone contact snapshot not ready: {result:?}"),
                                }
                            }
                            service.flush_read_sync().await;
                            if let Err(error) = service.refresh_chat_names().await {
                                log::debug!("Chat names refresh: {error}");
                            }
                            if let Err(error) = service.warm_avatars().await {
                                log::debug!("Avatar cache warmup: {error}");
                            }
                        }
                    }
                });
                self.history_prefetch.synced();
                let archive = self.archive.clone();
                tokio::spawn(async move {
                    loop {
                        let Ok(pending) = archive.pending_history().await else {
                            break;
                        };
                        if pending.is_empty() {
                            break;
                        }
                        let mut failed = false;
                        for row in pending {
                            let key = HistorySyncKey::new(row.chat, row.sender, row.id);
                            if let Err(error) = client
                                .replay_history_sync_notification(&key, &row.data)
                                .await
                            {
                                log::warn!("History recovery will retry on reconnect: {error}");
                                failed = true;
                            }
                        }
                        if failed {
                            break;
                        }
                    }
                });
            }
            Event::OfflineSyncCompleted(_) => {
                self.notifications_ready.store(true, Ordering::Release);
            }
            Event::OfflineSyncPreview(_) => {
                self.connection.write().await.syncing = true;
            }
            Event::PairSuccess(_) => {
                let mut state = self.connection.write().await;
                state.has_session = true;
                state.initial_sync = true;
                state.syncing = false;
                state.phase = Phase::Reconnecting;
                state.qr = None;
                state.pairing_code = None;
                state.notice = None;
            }
            Event::Disconnected(_) => {
                if let Some(call) = self.live_call().await.filter(|c| !c.ended()) {
                    self.finish_call(
                        &call.id,
                        Some("The call ended because WhatsApp disconnected.".into()),
                    )
                    .await;
                }
                self.notifications_ready.store(false, Ordering::Release);
                let mut state = self.connection.write().await;
                if state.phase != Phase::LoggedOut {
                    state.phase = Phase::Reconnecting;
                }
            }
            Event::LoggedOut(event) => {
                // Client::logout emits this event even when the unlink IQ failed.
                // The explicit logout path must decide from its typed report.
                if self.logout_in_progress.load(Ordering::Acquire) {
                    return Ok(());
                }
                if matches!(
                    event.reason,
                    whatsapp_rust::types::events::ConnectFailureReason::LoggedOut
                        | whatsapp_rust::types::events::ConnectFailureReason::UnknownLogout
                ) {
                    self.reset_session.store(true, Ordering::Release);
                    self.restart.notify_one();
                    self.archive
                        .set_preference("session_reset_pending".into(), "1".into())
                        .await?;
                    *self.connection.write().await = Connection {
                        phase: Phase::LoggedOut,
                        notice: Some(
                            "This device was unlinked. Preparing a new pairing code…".into(),
                        ),
                        ..Default::default()
                    };
                } else {
                    *self.connection.write().await = Connection {
                        phase: Phase::Error,
                        notice: Some(format!(
                            "WhatsApp refused this connection ({:?}).",
                            event.reason
                        )),
                        ..Default::default()
                    };
                }
            }
            Event::PairingQrCode(qr) => {
                let mut state = self.connection.write().await;
                state.has_session = false;
                state.initial_sync = false;
                state.syncing = false;
                state.phase = Phase::Pairing;
                state.notice = None;
                state.qr = Some(qr.code.clone());
                state.qr_expires_at = Some(now() + qr.timeout.as_secs() as i64);
            }
            Event::PairingCode(code) => {
                let mut state = self.connection.write().await;
                state.pairing_code = Some(code.code.clone());
                state.code_expires_at = Some(now() + code.timeout.as_secs() as i64);
                state.pair_retry_at = None;
                state.notice = None;
            }
            Event::PairingCodeError(error) => {
                let mut state = self.connection.write().await;
                state.notice = Some(error.error.clone());
                state.pairing_code = None;
                state.code_expires_at = None;
                state.pair_retry_at = error
                    .backoff
                    .map(|delay| now() + delay.as_secs() as i64)
                    .or_else(|| {
                        error
                            .rejection
                            .as_ref()
                            .filter(|rejection| rejection.is_throttled())
                            .map(|_| now() + 60)
                    });
            }
            Event::PairingCodeRefresh(refresh) => {
                let mut state = self.connection.write().await;
                state.pairing_code = None;
                state.code_expires_at = None;
                state.notice = Some("The pairing code expired. Request a new code.".into());
                drop(state);
                if !refresh.force_manual
                    && let Some(phone) = self.phone.lock().await.clone()
                {
                    let service = self.clone();
                    tokio::spawn(async move {
                        if let Err(error) = service.pair_phone(phone).await {
                            log::debug!("Pairing refresh did not complete: {error}");
                        }
                    });
                }
            }
            Event::PairingQrCodesExhausted(_) => {
                // The SDK renews QR-only sessions after exhausting its refs.
                // Keep the last image until the replacement arrives; expiry
                // metadata remains available to non-UI consumers.
            }
            Event::Messages(batch) => {
                // Also covers event-only recoveries which bypass the durability hook.
                self.archive
                    .store_messages(project_inbound(&batch.messages), true)
                    .await?;
                let mut viewed = std::collections::HashSet::new();
                for inbound in batch.messages.iter() {
                    let chat = inbound.info.source.chat.to_string();
                    if !inbound.info.source.is_from_me
                        && viewed.insert(chat.clone())
                        && self.desktop_viewing(&chat).await
                    {
                        if let Err(error) = self.mark_chat_read(chat).await {
                            log::warn!("Could not mark the visible conversation read: {error}");
                        }
                    }
                    let row = project_inbound(std::slice::from_ref(inbound)).remove(0);
                    #[cfg(all(target_os = "windows", feature = "desktop"))]
                    if !inbound.info.is_offline && inbound.info.unavailable_request_id.is_none() {
                        if let Err(error) = self.notify_incoming(&row.view).await {
                            log::warn!("Could not prepare an incoming notification: {error:#}");
                        }
                    }
                    if row.view.attachment() {
                        self.queue_media(row.view, inbound.message.as_ref().clone());
                    }
                }
            }
            Event::MarkChatAsReadUpdate(update) => {
                if let Some(read) = update.action.read {
                    let range = update.action.message_range.as_option();
                    let through = range
                        .and_then(|r| r.last_message_timestamp)
                        .or_else(|| update.action_timestamp.map(|t| t.timestamp()));
                    if through.is_some() || !update.from_full_sync || !read {
                        let refs = range
                            .map(|r| {
                                r.messages
                                    .iter()
                                    .filter_map(|m| {
                                        let key = m.key.as_option()?;
                                        Some(crate::read_state::ReadTarget {
                                            id: key.id.clone()?,
                                            sender: key.participant.clone().unwrap_or_default(),
                                            timestamp: m.timestamp.unwrap_or(0),
                                        })
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        self.archive
                            .apply_chat_read(
                                &update.jid.to_string(),
                                through,
                                refs,
                                update
                                    .action_timestamp
                                    .unwrap_or(update.timestamp)
                                    .timestamp_millis(),
                                !read,
                            )
                            .await?;
                    }
                }
            }
            Event::Receipt(receipt) => {
                use whatsapp_rust::wacore::types::presence::ReceiptType;
                if matches!(
                    receipt.r#type,
                    ReceiptType::ReadSelf | ReceiptType::PlayedSelf
                ) && receipt.source.is_from_me
                {
                    if receipt.source.chat.to_string() == crate::stories::CHAT {
                        let author = receipt.source.recipient.as_ref().map(ToString::to_string);
                        let ids: Vec<_> = receipt
                            .message_ids
                            .iter()
                            .map(ToString::to_string)
                            .collect();
                        self.archive
                            .mark_status_views(author.as_deref(), &ids)
                            .await?;
                    }
                    self.archive
                        .read_self(
                            &receipt.source.chat.to_string(),
                            &receipt
                                .message_ids
                                .iter()
                                .map(ToString::to_string)
                                .collect::<Vec<_>>(),
                            receipt.timestamp.timestamp_millis(),
                        )
                        .await?;
                }
                let status = match receipt.r#type {
                    ReceiptType::Read | ReceiptType::Played => Some("read"),
                    ReceiptType::Delivered => Some("delivered"),
                    ReceiptType::Sent => Some("sent"),
                    _ => None,
                };
                if let Some(status) = status {
                    self.archive
                        .receipt(
                            receipt.source.chat.to_string(),
                            receipt
                                .message_ids
                                .iter()
                                .map(ToString::to_string)
                                .collect(),
                            status.into(),
                        )
                        .await?;
                }
            }
            Event::ServerAck(ack) if ack.class.as_deref() == Some("message") => {
                if let Some(chat) = &ack.from {
                    self.archive
                        .receipt(
                            chat.to_string(),
                            vec![ack.id.clone()],
                            if ack.error.is_some() {
                                "failed"
                            } else {
                                "sent"
                            }
                            .into(),
                        )
                        .await?;
                }
            }
            Event::PinUpdate(update) => {
                self.archive
                    .set_chat_action(
                        update.jid.to_string(),
                        crate::chat::ChatAction::Pin,
                        update.action.pinned.unwrap_or(false),
                        0,
                        update
                            .action_timestamp
                            .unwrap_or(update.timestamp)
                            .timestamp_millis(),
                    )
                    .await?;
                self.history_prefetch.wake();
            }
            Event::MuteUpdate(update) => {
                self.archive
                    .set_chat_action(
                        update.jid.to_string(),
                        crate::chat::ChatAction::Mute,
                        update.action.muted.unwrap_or(false),
                        update.action.mute_end_timestamp.unwrap_or(-1),
                        update
                            .action_timestamp
                            .unwrap_or(update.timestamp)
                            .timestamp_millis(),
                    )
                    .await?;
            }
            Event::ArchiveUpdate(update) => {
                self.archive
                    .set_chat_action(
                        update.jid.to_string(),
                        crate::chat::ChatAction::Archive,
                        update.action.archived.unwrap_or(false),
                        0,
                        update
                            .action_timestamp
                            .unwrap_or(update.timestamp)
                            .timestamp_millis(),
                    )
                    .await?;
            }
            Event::DeleteChatUpdate(update) => {
                let chat = update.jid.to_string();
                let through = match update
                    .action
                    .message_range
                    .as_option()
                    .and_then(|r| r.last_message_timestamp)
                {
                    Some(timestamp) => timestamp,
                    None => self.archive.chat_boundary(&chat).await?,
                };
                self.archive.delete_chat_through(&chat, through).await?;
            }
            Event::ContactUpdate(contact) => {
                let name = contact
                    .action
                    .full_name
                    .as_deref()
                    .filter(|name| !name.trim().is_empty())
                    .or(contact.action.first_name.as_deref())
                    .unwrap_or("");
                self.archive
                    .contact(
                        contact.jid.to_string(),
                        name.into(),
                        serde_json::to_string(contact)?,
                        true,
                    )
                    .await?;
            }
            Event::FavoriteStickerUpdate(event) => {
                self.archive.favorite_sticker(event).await?;
            }
            Event::RemoveRecentStickerUpdate(event) => {
                use diesel::{
                    prelude::*,
                    sql_types::{BigInt, Text},
                };
                let (account, hash, before) = (
                    self.archive.account as i64,
                    event.filehash.clone(),
                    event
                        .action
                        .last_sticker_sent_ts
                        .map(|t| if t > 100_000_000_000 { t / 1000 } else { t })
                        .unwrap_or(i64::MAX),
                );
                self.archive.backend.shared().run(move |conn| {
                    diesel::sql_query("UPDATE brazap_stickers SET recent_at=0 WHERE account=? AND hash=? AND recent_at<=?")
                        .bind::<BigInt,_>(account).bind::<Text,_>(hash).bind::<BigInt,_>(before).execute(conn).map_err(crate::storage::db_error)?;
                    Ok(())
                }).await?;
            }
            Event::ContactRemoved(event) => {
                use diesel::{
                    prelude::*,
                    sql_types::{BigInt, Text},
                };
                let (account, jid) = (self.archive.account as i64, event.jid.to_string());
                self.archive
                    .backend
                    .shared()
                    .run(move |conn| {
                        diesel::sql_query(
                            "DELETE FROM brazap_saved_contacts WHERE account=? AND jid=?",
                        )
                        .bind::<BigInt, _>(account)
                        .bind::<Text, _>(jid)
                        .execute(conn)
                        .map_err(crate::storage::db_error)?;
                        Ok(())
                    })
                    .await?;
            }
            Event::GroupUpdate(event) => {
                if let whatsapp_rust::wacore::stanza::groups::GroupNotificationAction::Subject {
                    subject,
                    ..
                } = &*event.action
                {
                    if self
                        .archive
                        .allow_chat(&event.group_jid.to_string())
                        .await
                        .is_ok()
                    {
                        self.archive
                            .name_chat(event.group_jid.to_string(), subject.clone(), 0, 0)
                            .await?;
                    }
                }
            }
            Event::LockChatUpdate(event) => {
                if let Some(locked) = event.action.locked {
                    self.archive
                        .set_chat_locked(
                            event.jid.to_string(),
                            locked,
                            event
                                .action_timestamp
                                .unwrap_or(event.timestamp)
                                .timestamp_millis(),
                        )
                        .await?;
                    if locked {
                        if let Some(call) = self.live_call().await.filter(|call| !call.ended()) {
                            if self.archive.identities().await?.is_locked(&call.chat) {
                                let _ = self.end_call(&call.id).await;
                                let _ = self.dismiss_call(&call.id).await;
                            }
                        }
                    }
                }
            }
            Event::PictureUpdate(picture) => {
                self.archive
                    .invalidate_avatar(picture.jid.to_string())
                    .await?;
            }
            Event::IncomingCall(call) => {
                self.incoming_call(call).await?;
            }
            Event::CallEndedElsewhere(call) => {
                if let Some(current) = self.live_call().await.filter(|c| {
                    c.call_id.as_deref().unwrap_or(&c.id) == call.call_id.as_str() && !c.ended()
                }) {
                    self.finish_call(&current.id, None).await;
                }
            }
            Event::CallLogSync(call) => {
                let device = client.persistence_manager().get_device_snapshot();
                let own: Vec<_> = [device.pn.as_ref(), device.lid.as_ref()]
                    .into_iter()
                    .flatten()
                    .map(|jid| jid.to_non_ad().to_string())
                    .collect();
                self.archive
                    .store_call(
                        &call.record,
                        Some(call.from_me),
                        Some(call.call_creator_jid.to_string()),
                        &own,
                    )
                    .await?;
            }
            Event::CallLogHistory(call) => {
                let own = self.archive.own_jids().await?;
                self.archive
                    .store_call(
                        &call.record,
                        call.from_me,
                        call.call_creator_jid.as_ref().map(ToString::to_string),
                        &own,
                    )
                    .await?;
            }
            Event::HistorySync(history) => {
                let request_id = history.peer_data_request_session_id().map(str::to_owned);
                let progress = history.progress();
                let background = history.sync_type()
                    != wa::history_sync::HistorySyncType::ON_DEMAND as i32
                    && history.sync_type()
                        != wa::history_sync::HistorySyncType::RECENT_CONVERSATIONS_ON_DEMAND as i32;
                if background {
                    let mut state = self.connection.write().await;
                    state.syncing = true;
                    state.sync_progress = progress.map(|value| value.clamp(0, 100) as u32);
                    self.changed();
                }
                let history = history.clone();
                let decoded = tokio::task::spawn_blocking(move || history.get().cloned()).await?;
                if let Some(decoded) = decoded {
                    ingest_history(&self.archive, &decoded).await?;
                    self.archive
                        .history_response(request_id.as_deref(), decoded.progress.or(progress))
                        .await?;
                }
                if background {
                    let mut state = self.connection.write().await;
                    if state.sync_progress == Some(100) {
                        state.syncing = false;
                    }
                    self.history_prefetch.synced();
                }
            }
            _ => {}
        }
        if matches!(
            &*event,
            Event::GroupUpdate(_)
                | Event::ContactUpdate(_)
                | Event::ContactRemoved(_)
                | Event::LockChatUpdate(_)
                | Event::Receipt(_)
                | Event::PinUpdate(_)
                | Event::MuteUpdate(_)
                | Event::ArchiveUpdate(_)
                | Event::StarUpdate(_)
                | Event::MarkChatAsReadUpdate(_)
                | Event::DeleteChatUpdate(_)
                | Event::ClearChatUpdate(_)
                | Event::DeleteMessageForMeUpdate(_)
        ) {
            self.archive
                .event(
                    format!("{:?}", event.kind()),
                    serde_json::to_string(&*event)?,
                )
                .await?;
        }
        self.changed();
        Ok(())
    }

    pub async fn snapshot(&self, selected_chat: Option<String>) -> Result<Snapshot> {
        let identities = self.archive.identities().await?;
        let selected_chat = selected_chat
            .map(|chat| identities.canonical(&chat))
            .filter(|chat| chat != crate::stories::CHAT && !identities.is_locked(chat));
        let messages = match &selected_chat {
            Some(chat) => {
                self.archive
                    .messages(chat.clone(), crate::history::CHAT_PAGE_SIZE, None)
                    .await?
            }
            None => Vec::new(),
        };
        let all_chats = self.archive.chats().await?;
        let selected_chat =
            selected_chat.filter(|jid| all_chats.iter().any(|c| c.matches_jid(jid)));
        let messages = if selected_chat.is_some() {
            messages
        } else {
            Vec::new()
        };
        let current_chat = selected_chat
            .as_ref()
            .and_then(|jid| all_chats.iter().find(|c| &c.jid == jid).cloned());
        let chats = all_chats;
        Ok(Snapshot {
            statuses: self.archive.statuses().await?,
            updates: self.updates.read().await.clone(),
            connection: self.connection.read().await.clone(),
            chats,
            messages,
            selected_chat,
            revision: self.revision.load(Ordering::Relaxed),
            calls: self.archive.calls().await?,
            live_call: self.live_call().await,
            current_chat,
            settings: self.settings.read().await.clone(),
        })
    }

    pub(crate) async fn connected(&self) -> Result<Arc<Client>> {
        ensure!(
            !self.config.demo,
            "This operation is not simulated in demo mode"
        );
        ensure!(
            !self.logout_in_progress.load(Ordering::Acquire),
            "Logout is in progress"
        );
        ensure!(
            self.connection.read().await.phase == Phase::Connected,
            "WhatsApp is not connected. Complete pairing or wait for reconnection."
        );
        self.client
            .read()
            .await
            .clone()
            .context("WhatsApp is starting")
    }

    pub async fn pair_phone(&self, phone: String) -> Result<()> {
        if let Some(retry_at) = self.connection.read().await.pair_retry_at {
            ensure!(
                retry_at <= now(),
                "Wait {} seconds before requesting another code",
                retry_at - now()
            );
        }
        let phone = phone
            .trim()
            .trim_start_matches('+')
            .replace([' ', '-', '(', ')'], "");
        ensure!(
            (8..=15).contains(&phone.len()) && phone.bytes().all(|b| b.is_ascii_digit()),
            "Enter a phone number including country code."
        );
        ensure!(
            self.connection.read().await.phase != Phase::Connected,
            "WhatsApp is already connected"
        );
        let client = self
            .client
            .read()
            .await
            .clone()
            .context("Wait for WhatsApp to finish starting")?;
        let mut guard = self.phone.lock().await;
        client
            .pair_with_code(whatsapp_rust::pair_code::PairCodeOptions {
                phone_number: phone.clone(),
                ..Default::default()
            })
            .await?;
        *guard = Some(phone);
        Ok(())
    }

    pub async fn reconnect(&self) {
        self.restart.notify_one();
    }

    pub async fn send_text(&self, chat: String, text: String) -> Result<Message> {
        self.send_text_with_id(chat, text, None, crate::outbox::new_id(), None)
            .await
    }

    pub async fn send_voice(&self, upload: crate::model::VoiceUpload) -> Result<Message> {
        self.archive.allow_chat(&upload.chat).await?;
        ensure!(
            (1..=600).contains(&upload.duration_seconds),
            "Voice messages must be between one second and ten minutes"
        );
        ensure!(upload.waveform.len() <= 64, "Invalid voice waveform");
        let metadata = tokio::fs::metadata(&upload.path)
            .await
            .context("Cannot open voice recording")?;
        ensure!(
            metadata.is_file() && metadata.len() <= 8 * 1024 * 1024,
            "Voice recording is too large"
        );
        let bytes = tokio::fs::read(&upload.path).await?;
        let header = bytes.get(26).map(|segments| 27 + usize::from(*segments));
        ensure!(
            bytes.starts_with(b"OggS")
                && header.is_some_and(|at| bytes.get(at..at + 8) == Some(b"OpusHead")),
            "Voice recording must be Ogg Opus audio"
        );
        let jid: whatsapp_rust::Jid = upload.chat.parse()?;
        let context = if let Some(target) = &upload.reply_to {
            let original = self
                .archive
                .raw_message(
                    upload.chat.clone(),
                    target.sender.clone(),
                    target.id.clone(),
                )
                .await?;
            Some(
                whatsapp_rust::wacore::proto_helpers::build_quote_context_with_info(
                    target.id.clone(),
                    &self.author(&target.sender).await?,
                    &jid,
                    &jid,
                    &original,
                ),
            )
        } else {
            None
        };
        if self.config.demo {
            let mut audio = wa::message::AudioMessage::default();
            audio.mimetype = Some("audio/ogg; codecs=opus".into());
            audio.ptt = Some(true);
            audio.seconds = Some(upload.duration_seconds);
            audio.waveform = Some(upload.waveform);
            if let Some(context) = context {
                audio.context_info = MessageField::some(context);
            }
            let mut message = wa::Message::default();
            message.audio_message = MessageField::some(audio);
            return self.demo_send(upload.chat, message, Some(bytes)).await;
        }
        let client = self.connected().await?;
        let uploaded = client
            .upload(
                bytes.clone(),
                whatsapp_rust::download::MediaType::Audio,
                Default::default(),
            )
            .await?;
        let mut options = whatsapp_rust::media::AudioOptions::default()
            .with_ptt(true)
            .with_duration_seconds(upload.duration_seconds)
            .with_waveform(upload.waveform);
        if let Some(context) = context {
            options = options.with_context_info(Box::new(context));
        }
        let message = whatsapp_rust::media::audio_message(uploaded, options);
        self.send_prepared(client, jid, message, Some(bytes)).await
    }

    pub async fn send_attachment(
        &self,
        chat: String,
        path: String,
        kind: String,
        caption: String,
    ) -> Result<Message> {
        self.archive.allow_chat(&chat).await?;
        ensure!(caption.len() <= 65_536, "Message is too long");
        ensure!(
            kind != "audio" || caption.trim().is_empty(),
            "Audio files do not support captions."
        );
        if self.config.demo {
            return self.demo_attachment(chat, path, kind, caption).await;
        }
        let client = self.connected().await?;
        let path = Path::new(&path);
        let metadata = tokio::fs::metadata(path)
            .await
            .context("Cannot open attachment")?;
        ensure!(
            metadata.is_file() && metadata.len() <= 64 * 1024 * 1024,
            "Choose a file smaller than 64 MiB"
        );
        let bytes = tokio::fs::read(path).await?;
        let name = path
            .file_name()
            .context("Missing file name")?
            .to_string_lossy()
            .into_owned();
        let mime = mime_guess::from_path(path)
            .first_or_octet_stream()
            .to_string();
        let media_type = match kind.as_str() {
            "image" => whatsapp_rust::download::MediaType::Image,
            "video" => whatsapp_rust::download::MediaType::Video,
            "audio" => whatsapp_rust::download::MediaType::Audio,
            "sticker" => {
                ensure!(mime == "image/webp", "Stickers must be WebP files");
                whatsapp_rust::download::MediaType::Sticker
            }
            "file" => whatsapp_rust::download::MediaType::Document,
            _ => bail!("Unsupported attachment kind"),
        };
        let uploaded = client
            .upload(bytes.clone(), media_type, Default::default())
            .await?;
        use whatsapp_rust::media;
        let message = match kind.as_str() {
            "image" => media::image_message(
                uploaded,
                media::ImageOptions::default()
                    .with_mimetype(mime)
                    .with_caption(caption),
            ),
            "video" => media::video_message(
                uploaded,
                media::VideoOptions::default()
                    .with_mimetype(mime)
                    .with_caption(caption),
            ),
            "audio" => media::audio_message(
                uploaded,
                media::AudioOptions::default()
                    .with_mimetype(mime)
                    .with_ptt(false),
            ),
            "sticker" => {
                let mut sticker = wa::message::StickerMessage::default();
                sticker.url = Some(uploaded.url);
                sticker.direct_path = Some(uploaded.direct_path);
                sticker.media_key = Some(uploaded.media_key.to_vec());
                sticker.file_sha256 = Some(uploaded.file_sha256.to_vec());
                sticker.file_enc_sha256 = Some(uploaded.file_enc_sha256.to_vec());
                sticker.file_length = Some(uploaded.file_length);
                sticker.mimetype = Some("image/webp".into());
                let mut message = wa::Message::default();
                message.sticker_message = MessageField::some(sticker);
                message
            }
            _ => media::document_message(
                uploaded,
                media::DocumentOptions::default()
                    .with_file_name(name)
                    .with_mimetype(mime)
                    .with_caption(caption),
            ),
        };
        self.send_prepared(client, chat.parse()?, message, Some(bytes))
            .await
    }

    pub(crate) async fn send_prepared(
        &self,
        client: Arc<Client>,
        jid: whatsapp_rust::Jid,
        message: wa::Message,
        media: Option<Vec<u8>>,
    ) -> Result<Message> {
        self.archive.allow_chat(&jid.to_string()).await?;
        let chat = self.archive.canonical_chat(&jid.to_string()).await?;
        let jid: whatsapp_rust::Jid = chat.parse()?;
        let id = client.generate_message_id();
        let mut row = outgoing(chat.clone(), id.to_string(), &message);
        row.view.status = "sending".into();
        let mut view = row.view.clone();
        self.archive.store_messages(vec![row], true).await?;
        if let Some(bytes) = media {
            self.archive.put_media(&view, bytes).await?;
            view.has_media = true;
        }
        self.changed();
        let request = whatsapp_rust::SendRequest::new(&jid, message)
            .with_options(whatsapp_rust::SendOptions::default().with_message_id(id.clone()));
        match client.send(request).await {
            Ok(_) => {
                self.archive
                    .receipt(chat, vec![id.to_string()], "sent".into())
                    .await?;
                view.status = "sent".into();
                self.changed();
                Ok(view)
            }
            Err(error) => {
                self.archive
                    .receipt(chat, vec![id.to_string()], "failed".into())
                    .await?;
                self.changed();
                Err(error.into())
            }
        }
    }

    fn queue_media(self: &Arc<Self>, message: Message, proto: wa::Message) {
        let key = format!("{}:{}:{}", message.chat, message.sender, message.id);
        if !self
            .media_jobs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key.clone())
        {
            return;
        }
        let service = self.clone();
        tokio::spawn(async move {
            let _permit = service.media_slots.acquire().await;
            if let Err(error) = service.download_media(&message, proto).await {
                log::debug!("Attachment download deferred: {error}");
            }
            service
                .media_jobs
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&key);
        });
    }

    pub fn prepare_media(self: &Arc<Self>, chat: String, items: Vec<(String, String)>) {
        let service = self.clone();
        tokio::spawn(async move {
            for (sender, id) in items.into_iter().take(100) {
                if let Ok(proto) = service
                    .archive
                    .raw_message(chat.clone(), sender.clone(), id.clone())
                    .await
                {
                    let row = project_message(
                        chat.clone(),
                        sender,
                        id,
                        false,
                        0,
                        &proto,
                        String::new(),
                        String::new(),
                    );
                    // Videos are large; they download when played, not when a chat opens.
                    if matches!(row.view.kind.as_str(), "image" | "sticker") {
                        service.queue_media(row.view, proto);
                    }
                }
            }
        });
    }

    pub async fn download_media(&self, message: &Message, proto: wa::Message) -> Result<Vec<u8>> {
        self.archive.allow_chat(&message.chat).await?;
        if let Some(bytes) = self
            .archive
            .media(
                message.chat.clone(),
                message.sender.clone(),
                message.id.clone(),
            )
            .await?
        {
            return Ok(bytes);
        }
        let client = self.connected().await?;
        let base = crate::content::media_message(&proto);
        let downloadable: &dyn whatsapp_rust::download::Downloadable =
            if let Some(m) = base.image_message.as_option() {
                m
            } else if let Some(m) = base.video_message.as_option() {
                m
            } else if let Some(m) = base.document_message.as_option() {
                m
            } else if let Some(m) = base.audio_message.as_option() {
                m
            } else if let Some(m) = base.sticker_message.as_option() {
                m
            } else {
                bail!("Message has no downloadable attachment")
            };
        let bytes = crate::media_recovery::download(&client, message, downloadable).await?;
        self.archive.put_media(message, bytes.clone()).await?;
        self.changed();
        Ok(bytes)
    }

    pub async fn save_attachment(
        &self,
        chat: String,
        sender: String,
        id: String,
        path: String,
    ) -> Result<()> {
        let proto = self
            .archive
            .raw_message(chat.clone(), sender.clone(), id.clone())
            .await?;
        let view = project_message(
            chat,
            sender,
            id,
            false,
            0,
            &proto,
            String::new(),
            "{}".into(),
        )
        .view;
        let bytes = self.download_media(&view, proto).await?;
        tokio::fs::write(path, bytes).await?;
        Ok(())
    }

    pub async fn shutdown(&self) {
        self.stop_calls().await;
        self.stopping.store(true, Ordering::Relaxed);
        self.history_prefetch.wake();
        if let Some(client) = self.client.read().await.clone() {
            let report = client.shutdown().await;
            if let Err(error) = report.device {
                log::error!("Could not flush the WhatsApp session: {error}");
            }
        }
        self.restart.notify_one();
    }
}

fn project_inbound(batch: &[InboundMessage]) -> Vec<StoredMessage> {
    batch
        .iter()
        .map(|m| {
            let info = &m.info;
            let name = if !info.source.is_group && !info.source.is_from_me {
                info.push_name.to_string()
            } else {
                String::new()
            };
            project_message(
                info.source.chat.to_string(),
                info.source.sender.to_string(),
                info.id.to_string(),
                info.source.is_from_me,
                info.timestamp.timestamp(),
                &m.message,
                name,
                serde_json::to_string(info).unwrap_or_default(),
            )
        })
        .collect()
}

#[derive(Clone)]
struct Persistence {
    archive: Archive,
    revision: Arc<AtomicU64>,
}

#[async_trait::async_trait]
impl InboundDurabilityHook for Persistence {
    async fn on_messages(&self, _client: Arc<Client>, batch: &[InboundMessage]) -> Result<()> {
        self.archive
            .store_messages(project_inbound(batch), true)
            .await?;
        self.revision.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

#[async_trait::async_trait]
impl HistorySyncDurabilityHook for Persistence {
    async fn on_notification(
        &self,
        key: &HistorySyncKey,
        metadata: &HistorySyncMetadata<'_>,
        notification: &[u8],
    ) -> Result<()> {
        self.archive
            .capture(key, 0, notification.to_vec(), format!("{metadata:?}"))
            .await
    }
    async fn on_compressed_chunk(
        &self,
        key: &HistorySyncKey,
        metadata: &HistorySyncMetadata<'_>,
        compressed: &[u8],
    ) -> Result<()> {
        self.archive
            .capture(key, 1, compressed.to_vec(), format!("{metadata:?}"))
            .await?;
        let bytes = compressed.to_vec();
        let history = tokio::task::spawn_blocking(move || -> Result<wa::HistorySync> {
            let mut decoded = Vec::new();
            flate2::read::ZlibDecoder::new(bytes.as_slice())
                .take(128 * 1024 * 1024 + 1)
                .read_to_end(&mut decoded)?;
            ensure!(
                decoded.len() <= 128 * 1024 * 1024,
                "History chunk exceeds 128 MiB"
            );
            Ok(whatsapp_rust::waproto::codec::history_sync_decode(
                &decoded,
            )?)
        })
        .await??;
        ingest_history(&self.archive, &history).await?;
        self.archive.finish_history(key).await?;
        self.archive
            .history_response(
                metadata.peer_data_request_session_id,
                history.progress.or(metadata.progress),
            )
            .await?;
        self.revision.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

pub(crate) async fn ingest_history(archive: &Archive, history: &wa::HistorySync) -> Result<()> {
    for conversation in &history.conversations {
        if let Some(locked) = conversation.locked {
            archive
                .set_chat_locked(conversation.id.clone(), locked, 0)
                .await?;
        } else {
            archive
                .note_history_visibility(conversation.id.clone())
                .await?;
        }
        if archive.identities().await?.is_locked(&conversation.id) {
            continue;
        }
        if conversation.end_of_history_transfer_type == Some(wa::conversation::EndOfHistoryTransferType::COMPLETE_AND_NO_MORE_MESSAGE_REMAIN_ON_PRIMARY) {
            archive
                .complete_history(conversation.id.clone(), "server_complete")
                .await?;
        }
        if let Some(pinned) = conversation.pinned {
            archive
                .set_chat_action(
                    conversation.id.clone(),
                    crate::chat::ChatAction::Pin,
                    pinned > 0,
                    0,
                    0,
                )
                .await?;
        }
        archive
            .name_chat(
                conversation.id.clone(),
                conversation.name.clone().unwrap_or_default(),
                conversation.unread_count.unwrap_or(0) as i64,
                conversation.conversation_timestamp.unwrap_or(0) as i64,
            )
            .await?;
        let mut rows = Vec::new();
        for item in &conversation.messages {
            if let Some(info) = item.message.as_option()
                && let Some(key) = info.key.as_option()
            {
                let empty = wa::Message::default();
                let message = info.message.as_option().unwrap_or(&empty);
                let mut row = project_message(
                    conversation.id.clone(),
                    key.participant
                        .clone()
                        .or_else(|| info.participant.clone())
                        .unwrap_or_else(|| conversation.id.clone()),
                    key.id.clone().unwrap_or_default(),
                    key.from_me.unwrap_or(false),
                    info.message_timestamp.unwrap_or(0) as i64,
                    message,
                    String::new(),
                    serde_json::to_string(info)?,
                );
                if info.message.is_unset() && !row.view.is_call() {
                    continue;
                }
                if row.view.from_me {
                    row.view.status = crate::storage::history_status(info, &row.view.chat).into();
                }
                rows.push(row);
            }
        }
        archive.store_messages(rows, false).await?;
    }
    for contact in &history.pushnames {
        if let Some(jid) = &contact.id {
            archive
                .contact(
                    jid.clone(),
                    contact.pushname.clone().unwrap_or_default(),
                    serde_json::to_string(contact)?,
                    false,
                )
                .await?;
        }
    }
    if !history.call_log_records.is_empty() {
        let own = archive.own_jids().await?;
        for record in &history.call_log_records {
            archive.store_call(record, None, None, &own).await?;
        }
    }
    Ok(())
}
