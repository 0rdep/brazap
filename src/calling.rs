//! Live calls share the same WhatsApp client as messages and MCP. Device workers
//! are owned by the service; closing a dialog never abandons a live call.
use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use whatsapp_rust::{
    Client,
    voip::{CallHandle, VideoUpgradeToken},
    wacore::types::call::{CallAction, IncomingCall},
};

use crate::{model::now, service::Service};
#[cfg(feature = "desktop")]
use whatsapp_rust::voip::{CallEvent, VideoState};

#[cfg(feature = "desktop")]
mod media;
#[cfg(feature = "desktop")]
mod video;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LiveCall {
    pub id: String,
    #[serde(default)]
    pub call_id: Option<String>,
    #[serde(default)]
    pub creator: Option<String>,
    pub chat: String,
    pub name: String,
    pub phone: String,
    pub photo_version: i64,
    pub incoming: bool,
    pub video: bool,
    pub phase: String,
    pub muted: bool,
    pub camera: bool,
    #[serde(default)]
    pub camera_fallback: bool,
    pub peer_camera: bool,
    pub video_requested: bool,
    pub created_at: i64,
    pub connected_at: Option<i64>,
    pub ended_at: Option<i64>,
    pub error: Option<String>,
    #[serde(default)]
    pub outcome: Option<String>,
}
impl LiveCall {
    pub fn ended(&self) -> bool {
        self.ended_at.is_some()
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct PreviewFrame {
    pub sequence: u64,
    pub jpeg: String,
}

struct Session {
    view: LiveCall,
    offer: Option<IncomingCall>,
    handle: Option<CallHandle>,
    upgrade: Option<VideoUpgradeToken>,
    cancel: tokio::sync::watch::Sender<bool>,
    #[cfg(feature = "desktop")]
    audio: Option<media::Audio>,
    #[cfg(feature = "desktop")]
    video: Option<video::Video>,
}

#[derive(Default)]
pub struct Calls {
    session: Mutex<Option<Session>>,
    control: Mutex<()>,
}

impl Service {
    pub async fn live_call(&self) -> Option<LiveCall> {
        self.live_calls
            .session
            .lock()
            .await
            .as_ref()
            .map(|s| s.view.clone())
    }

    pub async fn call_preview(&self, id: &str, local: bool, after: u64) -> Option<PreviewFrame> {
        let state = self.live_calls.session.lock().await;
        let session = state
            .as_ref()
            .filter(|s| s.view.id == id && !s.view.ended())?;
        #[cfg(feature = "desktop")]
        return session.video.as_ref()?.preview(local, after);
        #[cfg(not(feature = "desktop"))]
        {
            let _ = (session, local, after);
            None
        }
    }

    async fn new_call(
        &self,
        chat: &str,
        incoming: bool,
        is_video: bool,
        id: String,
    ) -> Result<Session> {
        let identities = self.archive.identities().await?;
        ensure!(!identities.is_locked(chat), "This chat is locked");
        let chat = identities.canonical(chat);
        let peer: whatsapp_rust::Jid = chat.parse()?;
        ensure!(
            matches!(peer.server.as_str(), "s.whatsapp.net" | "lid" | "g.us"),
            "This address cannot receive calls"
        );
        ensure!(
            !chat.ends_with("@g.us"),
            "Group calling is not available in Brazap yet"
        );
        let contact = self
            .archive
            .chats()
            .await?
            .into_iter()
            .find(|c| c.matches_jid(&chat));
        let (name, phone, photo_version) = contact
            .map(|c| (c.name, c.phone, c.photo_version))
            .unwrap_or_else(|| {
                let (name, phone) = identities.person(&chat, "");
                (name, phone, 0)
            });
        Ok(Session {
            view: LiveCall {
                id,
                call_id: None,
                creator: None,
                chat,
                name,
                phone,
                photo_version,
                incoming,
                video: is_video,
                phase: if incoming { "ringing" } else { "preparing" }.into(),
                muted: false,
                camera: false,
                camera_fallback: false,
                peer_camera: is_video,
                video_requested: false,
                created_at: now(),
                connected_at: None,
                ended_at: None,
                error: None,
                outcome: None,
            },
            offer: None,
            handle: None,
            upgrade: None,
            cancel: tokio::sync::watch::channel(false).0,
            #[cfg(feature = "desktop")]
            audio: None,
            #[cfg(feature = "desktop")]
            video: None,
        })
    }

    pub async fn start_call(self: &Arc<Self>, chat: String, video: bool) -> Result<LiveCall> {
        #[cfg(not(feature = "desktop"))]
        {
            let _ = (chat, video);
            anyhow::bail!("Calling requires a desktop build of Brazap");
        }
        #[cfg(feature = "desktop")]
        {
            let client = if self.config.demo {
                None
            } else {
                Some(self.connected().await?)
            };
            let session = self
                .new_call(&chat, false, video, uuid::Uuid::new_v4().to_string())
                .await?;
            let view = session.view.clone();
            {
                let mut state = self.live_calls.session.lock().await;
                ensure!(
                    state.as_ref().is_none_or(|s| s.view.ended()),
                    "Another call is already in progress"
                );
                self.archive.journal_live_call(&session.view).await?;
                *state = Some(session);
            }
            self.changed();
            let service = self.clone();
            let id = view.id.clone();
            tokio::spawn(async move {
                service.connect_call(id, client, None, video).await;
            });
            Ok(view)
        }
    }

    pub async fn answer_call(self: &Arc<Self>, id: String, video: bool) -> Result<()> {
        #[cfg(not(feature = "desktop"))]
        {
            let _ = (id, video);
            anyhow::bail!("Calling requires a desktop build of Brazap");
        }
        #[cfg(feature = "desktop")]
        {
            let client = if self.config.demo {
                None
            } else {
                Some(self.connected().await?)
            };
            let (offer, video) = {
                let mut state = self.live_calls.session.lock().await;
                let session = state
                    .as_mut()
                    .filter(|s| s.view.id == id)
                    .context("Call is no longer available")?;
                ensure!(
                    session.view.incoming && session.view.phase == "ringing",
                    "Call is no longer ringing"
                );
                session.view.phase = "preparing".into();
                self.archive.journal_live_call(&session.view).await?;
                let video = video && session.view.video;
                (session.offer.clone(), video)
            };
            self.changed();
            let service = self.clone();
            tokio::spawn(async move {
                service.connect_call(id, client, offer, video).await;
            });
            Ok(())
        }
    }

    pub async fn end_call(&self, id: &str) -> Result<()> {
        let (handle, offer) = {
            let mut state = self.live_calls.session.lock().await;
            let session = state
                .as_mut()
                .filter(|s| s.view.id == id)
                .context("Call is no longer available")?;
            if session.view.ended() {
                return Ok(());
            }
            session.view.outcome = Some(
                if session.view.connected_at.is_some() {
                    "completed"
                } else if session.view.incoming {
                    "declined"
                } else {
                    "cancelled"
                }
                .into(),
            );
            (session.handle.clone(), session.offer.clone())
        };
        // Invalidate local work before awaiting network; late startup completion
        // must terminate its own handle instead of reviving the cancelled call.
        self.finish_call(id, None).await;
        if let Some(handle) = handle {
            let result = handle.terminate().await;
            ensure!(
                result.peer_notified(),
                "The call ended locally, but WhatsApp could not be notified"
            );
        } else if let Some(offer) = offer
            && !self.config.demo
        {
            self.connected().await?.voip().reject(&offer).await?;
        }
        Ok(())
    }

    pub async fn dismiss_call(&self, id: &str) -> Result<()> {
        let mut state = self.live_calls.session.lock().await;
        ensure!(
            state
                .as_ref()
                .is_none_or(|s| s.view.id != id || s.view.ended()),
            "End the call before closing it"
        );
        if state.as_ref().is_some_and(|s| s.view.id == id) {
            *state = None;
        }
        self.changed();
        Ok(())
    }

    pub async fn mute_call(&self, id: &str, muted: bool) -> Result<()> {
        let _guard = self.live_calls.control.lock().await;
        let handle = {
            let mut state = self.live_calls.session.lock().await;
            let s = state
                .as_mut()
                .filter(|s| s.view.id == id && !s.view.ended())
                .context("Call is no longer available")?;
            s.view.muted = muted;
            #[cfg(feature = "desktop")]
            if let Some(audio) = &s.audio {
                audio.mute(muted);
            }
            self.archive.journal_live_call(&s.view).await?;
            s.handle.clone()
        };
        self.changed();
        if let Some(handle) = handle {
            handle.set_muted(muted).await?;
        }
        Ok(())
    }

    pub async fn camera_call(&self, id: &str, enabled: bool) -> Result<()> {
        #[cfg(not(feature = "desktop"))]
        {
            let _ = (id, enabled);
            anyhow::bail!("Video calling requires a desktop build of Brazap");
        }
        #[cfg(feature = "desktop")]
        {
            let _guard = self.live_calls.control.lock().await;
            let (handle, upgrade, had_video, mut cancel) = {
                let state = self.live_calls.session.lock().await;
                let s = state
                    .as_ref()
                    .filter(|s| s.view.id == id && !s.view.ended())
                    .context("Call is no longer available")?;
                ensure!(s.view.phase == "active", "Wait for the call to connect");
                if s.view.camera == enabled {
                    return Ok(());
                }
                (
                    s.handle.clone(),
                    s.upgrade.clone(),
                    s.video.is_some(),
                    s.cancel.subscribe(),
                )
            };
            if !enabled {
                // Release local capture even if signaling fails; keep the peer's decoder.
                {
                    let mut state = self.live_calls.session.lock().await;
                    if let Some(s) = state
                        .as_mut()
                        .filter(|s| s.view.id == id && !s.view.ended())
                    {
                        if let Some(video) = &s.video {
                            video.stop_camera();
                        }
                        s.view.camera = false;
                        s.view.camera_fallback = false;
                        self.archive.journal_live_call(&s.view).await?;
                    }
                }
                self.changed();
                if let Some(handle) = &handle {
                    handle.stop_video().await?;
                }
            } else {
                if had_video {
                    tokio::time::timeout(Duration::from_secs(3), async {
                        loop {
                            let stopped = self
                                .live_calls
                                .session
                                .lock()
                                .await
                                .as_ref()
                                .filter(|s| s.view.id == id && !s.view.ended())
                                .and_then(|s| s.video.as_ref())
                                .is_none_or(|video| video.camera_stopped());
                            if stopped {
                                break;
                            }
                            tokio::time::sleep(Duration::from_millis(10)).await;
                        }
                    })
                    .await
                    .context("Camera took too long to stop")?;
                }
                let devices = tokio::select! {
                    biased;
                    _=cancel.changed()=>return Ok(()),
                    result=video::Video::start(self.config.demo)=>result?,
                };
                if let Some(handle) = &handle {
                    if let Some(token) = upgrade {
                        handle
                            .accept_video(token, devices.source(), devices.sink())
                            .await?;
                    } else if had_video {
                        handle
                            .resume_video(devices.source(), devices.sink())
                            .await?;
                    } else {
                        handle.start_video(devices.source(), devices.sink()).await?;
                    }
                }
                let mut state = self.live_calls.session.lock().await;
                if let Some(s) = state
                    .as_mut()
                    .filter(|s| s.view.id == id && !s.view.ended())
                {
                    s.view.camera_fallback = devices.is_black();
                    s.video = Some(devices);
                    s.upgrade = None;
                    s.view.video_requested = false;
                    s.view.video = true;
                    s.view.camera = true;
                    if self.config.demo {
                        s.view.peer_camera = true;
                    }
                    self.archive.journal_live_call(&s.view).await?;
                }
            }
            self.changed();
            Ok(())
        }
    }

    pub async fn demo_incoming_call(
        self: &Arc<Self>,
        chat: String,
        video: bool,
    ) -> Result<LiveCall> {
        ensure!(
            self.config.demo,
            "Simulated calls are only available in demo mode"
        );
        self.archive.note_history_visibility(chat.clone()).await?;
        let id = uuid::Uuid::new_v4().to_string();
        let peer: whatsapp_rust::Jid = chat.parse()?;
        let offer = IncomingCall::builder()
            .from(peer.clone())
            .stanza_id(format!("demo-{id}"))
            .offline(false)
            .action(CallAction::Offer {
                call_id: id.clone(),
                call_creator: peer,
                caller_pn: None,
                caller_country_code: None,
                device_class: None,
                joinable: false,
                is_video: video,
                audio: Vec::new(),
                group_jid: None,
            })
            .build();
        self.incoming_call(&offer).await?;
        self.live_call()
            .await
            .filter(|c| c.id == id)
            .context("The incoming call could not be shown")
    }

    pub(crate) async fn incoming_call(self: &Arc<Self>, call: &IncomingCall) -> Result<()> {
        match &call.action {
            CallAction::Offer {
                is_video,
                group_jid,
                ..
            } if !call.offline => {
                if group_jid.is_some() || call.group.is_some() {
                    return Ok(());
                }
                // A collection resync can fail independently of a known chat's
                // visibility. Use the same per-chat policy as notifications.
                if !self
                    .chat_locks_ready
                    .load(std::sync::atomic::Ordering::Acquire)
                    && !self
                        .archive
                        .chat_visibility_known(&call.from.to_string())
                        .await?
                {
                    log::debug!("Incoming call hidden while chat visibility is unverified");
                    return Ok(());
                }
                let mut session = match self
                    .new_call(
                        &call.from.to_string(),
                        true,
                        *is_video,
                        call.action.call_id().into(),
                    )
                    .await
                {
                    Ok(s) => s,
                    Err(error) => {
                        log::debug!("Incoming call cannot be shown: {error}");
                        return Ok(());
                    }
                };
                if !crate::storage::meaningful_name(&session.view.chat, &session.view.name) {
                    let identity = self
                        .archive
                        .identities()
                        .await?
                        .person(&session.view.chat, call.notify.as_deref().unwrap_or(""));
                    session.view.name = identity.0;
                    session.view.phone = identity.1;
                }
                session.offer = Some(call.clone());
                session.view.call_id = Some(call.action.call_id().into());
                session.view.creator = Some(call.action.call_creator().to_string());
                let id = session.view.id.clone();
                {
                    let mut state = self.live_calls.session.lock().await;
                    if state.as_ref().is_some_and(|s| !s.view.ended()) {
                        return Ok(());
                    }
                    self.archive.journal_live_call(&session.view).await?;
                    *state = Some(session);
                }
                self.changed();
                #[cfg(all(target_os = "windows", feature = "desktop"))]
                if !self.config.demo
                    && let Some(call) = self.live_call().await
                {
                    let settings = self.settings.read().await;
                    if settings.notifications.enabled {
                        let title = if settings.notifications.preview {
                            call.name.clone()
                        } else {
                            "Brazap".into()
                        };
                        let preview = if call.video {
                            "Incoming video call"
                        } else {
                            "Incoming voice call"
                        };
                        crate::windows_integration::notify(
                            title,
                            crate::ui::call_notification_text(
                                preview,
                                &settings.appearance.language,
                            ),
                            call.chat,
                            settings.notifications.sound,
                        );
                    }
                }
                let weak = Arc::downgrade(self);
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    if let Some(s) = weak.upgrade() {
                        if s.live_call()
                            .await
                            .is_some_and(|c| c.id == id && c.phase == "ringing")
                        {
                            s.finish_call(&id, None).await;
                        }
                    }
                });
            }
            CallAction::Terminate { .. } | CallAction::Reject { reason: None, .. } => {
                let id = {
                    let state = self.live_calls.session.lock().await;
                    state
                        .as_ref()
                        .filter(|s| {
                            s.offer
                                .as_ref()
                                .is_some_and(|o| o.action.call_id() == call.action.call_id())
                        })
                        .map(|s| s.view.id.clone())
                };
                if let Some(id) = id {
                    self.finish_call(&id, None).await;
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub(crate) async fn finish_call(&self, id: &str, error: Option<String>) {
        let record = {
            let mut state = self.live_calls.session.lock().await;
            let Some(s) = state
                .as_mut()
                .filter(|s| s.view.id == id && !s.view.ended())
            else {
                return;
            };
            s.cancel.send_replace(true);
            s.view.ended_at = Some(now());
            s.view.phase = if error.is_some() { "failed" } else { "ended" }.into();
            if error.is_some() {
                s.view.outcome = Some("failed".into());
            }
            s.view.error = error;
            s.view.camera = false;
            s.view.camera_fallback = false;
            s.view.peer_camera = false;
            #[cfg(feature = "desktop")]
            {
                s.audio = None;
                s.video = None;
            }
            let call_id = s
                .handle
                .as_ref()
                .map(|h| h.call_id().to_string())
                .unwrap_or_else(|| s.view.id.clone());
            let creator = s
                .handle
                .as_ref()
                .map(|h| h.call_creator().to_string())
                .or_else(|| {
                    s.offer
                        .as_ref()
                        .map(|o| o.action.call_creator().to_string())
                });
            s.handle = None;
            s.offer = None;
            s.upgrade = None;
            (s.view.clone(), call_id, creator)
        };
        if let Err(e) = self.archive.journal_live_call(&record.0).await {
            log::error!("Could not save call lifecycle: {e}");
        }
        if let Err(e) = self
            .archive
            .store_live_call(&record.0, record.1, record.2)
            .await
        {
            log::error!("Could not save call history: {e}");
        }
        self.changed();
    }

    pub(crate) async fn stop_calls(&self) {
        if let Some(call) = self.live_call().await.filter(|c| !c.ended()) {
            let _ = self.end_call(&call.id).await;
        }
    }

    #[cfg(feature = "desktop")]
    async fn connect_call(
        self: Arc<Self>,
        id: String,
        client: Option<Arc<Client>>,
        offer: Option<IncomingCall>,
        video: bool,
    ) {
        let mut cancel = {
            let state = self.live_calls.session.lock().await;
            let Some(s) = state
                .as_ref()
                .filter(|s| s.view.id == id && !s.view.ended())
            else {
                return;
            };
            s.cancel.subscribe()
        };
        if *cancel.borrow() {
            return;
        }
        tokio::select! {
            biased;
            _ = cancel.changed() => {},
            result = self.prepare_call(&id, client, offer, video) => {
                if let Err(error)=result {
                    log::warn!("Call setup failed: {error:#}");
                    let handle=self.live_calls.session.lock().await.as_ref().filter(|s|s.view.id==id).and_then(|s|s.handle.clone());
                    self.finish_call(&id,Some(error.to_string())).await;
                    if let Some(handle)=handle { handle.terminate().await; }
                }
            }
        }
    }

    #[cfg(feature = "desktop")]
    async fn prepare_call(
        self: &Arc<Self>,
        id: &str,
        client: Option<Arc<Client>>,
        offer: Option<IncomingCall>,
        video: bool,
    ) -> Result<()> {
        let audio = if self.config.demo {
            None
        } else {
            Some(media::Audio::start().await?)
        };
        let video = if video {
            Some(video::Video::start(self.config.demo).await?)
        } else {
            None
        };
        let chat = self
            .live_call()
            .await
            .filter(|s| s.id == id && !s.ended())
            .context("Call was cancelled")?
            .chat;
        let handle = if let Some(client) = client {
            let endpoints = audio.as_ref().context("Microphone is not ready")?;
            let peer = chat.parse()?;
            let voip = client.voip();
            let mut builder = if let Some(offer) = &offer {
                voip.accept(offer)
            } else {
                let mut outgoing = voip
                    .call(&peer)
                    .audio_opus(endpoints.source(), endpoints.sink());
                if let Some(video) = &video {
                    outgoing = outgoing.video(video.source(), video.sink());
                }
                let h = outgoing.start().await?;
                return self.install_call(id, Some(h), audio, video).await;
            }
            .audio_opus(endpoints.source(), endpoints.sink());
            if let Some(video) = &video {
                builder = builder.video(video.source(), video.sink());
            }
            Some(builder.start().await?)
        } else {
            None
        };
        self.install_call(id, handle, audio, video).await
    }

    #[cfg(feature = "desktop")]
    async fn install_call(
        self: &Arc<Self>,
        id: &str,
        handle: Option<CallHandle>,
        audio: Option<media::Audio>,
        video: Option<video::Video>,
    ) -> Result<()> {
        {
            let mut state = self.live_calls.session.lock().await;
            let Some(s) = state
                .as_mut()
                .filter(|s| s.view.id == id && !s.view.ended())
            else {
                drop(state);
                if let Some(h) = handle {
                    h.terminate().await;
                }
                return Ok(());
            };
            s.view.phase = if self.config.demo {
                "active"
            } else {
                "calling"
            }
            .into();
            s.view.camera = video.is_some();
            s.view.camera_fallback = video.as_ref().is_some_and(|v| v.is_black());
            s.view.peer_camera = video.is_some();
            if self.config.demo {
                s.view.connected_at = Some(now());
            }
            s.audio = audio;
            s.video = video;
            s.handle = handle.clone();
            if let Some(h) = &handle {
                s.view.call_id = Some(h.call_id().into());
                s.view.creator = Some(h.call_creator().to_string());
            }
            self.archive.journal_live_call(&s.view).await?;
        }
        self.changed();
        let weak = Arc::downgrade(self);
        let id = id.to_string();
        tokio::spawn(async move {
            let mut events = handle.as_ref().and_then(|h| h.take_events());
            loop {
                let event = if let Some(events) = events.as_mut() {
                    tokio::select! { event = events.recv() => event.ok(), _ = tokio::time::sleep(Duration::from_millis(250)) => None }
                } else {
                    tokio::time::sleep(Duration::from_millis(250)).await;
                    None
                };
                let Some(service) = weak.upgrade() else {
                    if let Some(h) = &handle {
                        h.terminate().await;
                    }
                    break;
                };
                if service
                    .live_call()
                    .await
                    .is_none_or(|s| s.id != id || s.ended())
                {
                    break;
                }
                if let Some(event) = event {
                    if let CallEvent::Closed(_) = event {
                        service.finish_call(&id, None).await;
                        break;
                    }
                    if matches!(
                        event,
                        CallEvent::MediaSetupFailed(_)
                            | CallEvent::RelayAllocateFailed(_)
                            | CallEvent::RelayAllocateTimedOut
                            | CallEvent::RelayReconnectTimedOut
                    ) {
                        if let Some(h) = &handle {
                            h.terminate().await;
                        }
                        service
                            .finish_call(
                                &id,
                                Some("Could not connect the call. Please try again.".into()),
                            )
                            .await;
                        break;
                    }
                    let mut state = service.live_calls.session.lock().await;
                    if let Some(s) = state
                        .as_mut()
                        .filter(|s| s.view.id == id && !s.view.ended())
                    {
                        match event {
                            CallEvent::MediaConnected => {
                                s.view.phase = "active".into();
                                s.view.connected_at.get_or_insert(now());
                                if let Err(e) = service.archive.journal_live_call(&s.view).await {
                                    log::warn!("Could not save connected call: {e}");
                                }
                            }
                            CallEvent::VideoKeyframeNeeded => {
                                if let Some(v) = &s.video {
                                    v.keyframe();
                                }
                            }
                            CallEvent::PeerVideoStateChanged {
                                state,
                                upgrade_token,
                                ..
                            } => {
                                s.view.peer_camera = state == VideoState::Enabled;
                                if let Some(token) = upgrade_token {
                                    s.upgrade = Some(token);
                                    s.view.video_requested = true;
                                }
                            }
                            _ => {}
                        }
                    }
                    service.changed();
                }
                let failure = {
                    let mut state = service.live_calls.session.lock().await;
                    let Some(s) = state
                        .as_mut()
                        .filter(|s| s.view.id == id && !s.view.ended())
                    else {
                        break;
                    };
                    if let Some(h) = &handle {
                        if h.media_connected() && s.view.phase != "active" {
                            s.view.phase = "active".into();
                            s.view.connected_at.get_or_insert(now());
                            if let Err(e) = service.archive.journal_live_call(&s.view).await {
                                log::warn!("Could not save connected call: {e}");
                            }
                            service.changed();
                        }
                        if h.phase().is_none_or(|p| {
                            p == whatsapp_rust::wacore::voip_control::CallPhase::Ended
                        }) {
                            drop(state);
                            service.finish_call(&id, None).await;
                            break;
                        }
                    }
                    let fallback = s.view.camera && s.video.as_ref().is_some_and(|v| v.is_black());
                    if s.view.camera_fallback != fallback {
                        s.view.camera_fallback = fallback;
                        service.changed();
                    }
                    if let (Some(video), Some(handle)) = (&s.video, &handle) {
                        if video.needs_keyframe() {
                            handle.request_peer_keyframe(
                                whatsapp_rust::voip::KeyframeUrgency::Immediate,
                            );
                        }
                    }
                    s.audio
                        .as_ref()
                        .and_then(|a| a.error())
                        .or_else(|| s.video.as_ref().and_then(|v| v.error()))
                };
                if let Some(error) = failure {
                    service.finish_call(&id, Some(error)).await;
                    if let Some(h) = &handle {
                        h.terminate().await;
                    }
                    break;
                }
                if service
                    .live_call()
                    .await
                    .is_some_and(|s| s.connected_at.is_none() && now() - s.created_at >= 60)
                {
                    let _ = service.end_call(&id).await;
                    break;
                }
            }
        });
        Ok(())
    }
}
