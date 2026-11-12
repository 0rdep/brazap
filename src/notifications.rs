use std::time::{Duration, Instant};

use anyhow::{Result, ensure};
#[cfg(all(target_os = "windows", feature = "desktop"))]
use diesel::{
    prelude::*,
    sql_types::{BigInt, Text},
};
use serde_json::{Value, json};

use crate::service::Service;
#[cfg(all(target_os = "windows", feature = "desktop"))]
use crate::{
    model::Message,
    storage::{MESSAGE_SELECT, MessageRow, db_error},
};

/// UI ownership is a lease: a headless service must not consume a deep link.
#[derive(Default)]
pub(crate) struct Desktop {
    seen: Option<Instant>,
    pid: u32,
    chat: Option<String>,
    activation: Option<Option<String>>,
    /// The connection (by its start time) and presence last announced to WhatsApp.
    presence: Option<(u64, bool)>,
}

/// What a desktop activation opens: a chat from a notification link, or a WhatsApp
/// `whatsapp://send?phone=…` link, which is passed on whole to be checked and opened.
pub fn activation_target(link: &str) -> Result<String> {
    if link
        .get(..11)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("whatsapp://"))
    {
        ensure!(
            link.len() <= 8192 && crate::phone_links::parse(link, None).is_some(),
            "This WhatsApp link does not name a phone number"
        );
        return Ok(format!("whatsapp://{}", &link[11..]));
    }
    chat_from_link(link)
}

pub fn chat_from_link(link: &str) -> Result<String> {
    let chat = link
        .strip_prefix("brazap://chat/")
        .or_else(|| link.strip_prefix("brazap-demo://chat/"))
        .unwrap_or(link);
    ensure!(
        chat.len() <= 160
            && chat
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "+-_.@".contains(c)),
        "Invalid chat link"
    );
    let jid: whatsapp_rust::Jid = chat.parse()?;
    ensure!(
        matches!(jid.server.as_str(), "s.whatsapp.net" | "lid" | "g.us"),
        "Unsupported chat address"
    );
    Ok(jid.to_string())
}

impl Service {
    pub async fn activate_desktop(&self, chat: Option<String>) -> Result<Value> {
        let chat = chat.map(|chat| activation_target(&chat)).transpose()?;
        let mut desktop = self.desktop.lock().await;
        let available = desktop
            .seen
            .is_some_and(|seen| seen.elapsed() < Duration::from_secs(5));
        if available {
            desktop.activation = Some(chat);
        }
        Ok(json!({"accepted": available, "pid": desktop.pid}))
    }

    pub async fn poll_desktop(
        self: &std::sync::Arc<Self>,
        pid: u32,
        chat: Option<String>,
        focused: bool,
    ) -> Value {
        let mut desktop = self.desktop.lock().await;
        desktop.seen = Some(Instant::now());
        desktop.pid = pid;
        desktop.chat = chat.clone();
        let activation = desktop.activation.take();
        drop(desktop);
        #[cfg(all(target_os = "windows", feature = "desktop"))]
        let focused = focused && crate::windows_integration::is_foreground(pid);
        self.sync_presence(focused).await;
        if let Some(chat) = chat {
            if self.desktop_viewing(&chat).await
                && self
                    .archive
                    .chat_unread(&chat)
                    .await
                    .is_ok_and(|unread| unread > 0)
            {
                if let Err(error) = self.mark_chat_read(chat).await {
                    log::warn!("Could not persist the visible chat read: {error}");
                }
            }
        }
        match activation {
            Some(chat) => json!({"activate":true,"chat":chat}),
            None => json!({"activate":false}),
        }
    }

    /// WhatsApp treats an "available" device as the place you read, and does not notify your
    /// phone while one is. So this device is announced as online only while its window has
    /// focus, as WhatsApp Web does, and as offline when it is in the background or closed to
    /// the tray. The server forgets presence when a connection ends, so each connection starts
    /// unannounced.
    async fn sync_presence(&self, active: bool) {
        if self.config.demo {
            return;
        }
        let online = active && self.settings.read().await.privacy.show_online;
        let connection = self.connected_at.load(std::sync::atomic::Ordering::Acquire);
        let sent = self.desktop.lock().await.presence;
        if sent == Some((connection, online)) {
            return;
        }
        // A new connection starts offline, so there is nothing to retract.
        if !online && sent.is_none_or(|(at, _)| at != connection) {
            self.desktop.lock().await.presence = Some((connection, false));
            return;
        }
        let Ok(client) = self.connected().await else {
            return;
        };
        let result = if online {
            client.presence().set_available().await
        } else {
            client.presence().set_unavailable().await
        };
        match result {
            Ok(()) => self.desktop.lock().await.presence = Some((connection, online)),
            // The account name arrives shortly after connecting; the next poll tries again.
            Err(error) => log::debug!("Presence not updated yet: {error}"),
        }
    }

    pub(crate) async fn desktop_viewing(&self, chat: &str) -> bool {
        let (active, pid) = {
            let desktop = self.desktop.lock().await;
            if desktop
                .seen
                .is_none_or(|seen| seen.elapsed() >= Duration::from_secs(5))
            {
                return false;
            }
            (desktop.chat.clone(), desktop.pid)
        };
        let Some(active) = active else {
            return false;
        };
        let Ok(identities) = self.archive.identities().await else {
            return false;
        };
        if identities.canonical(&active) != identities.canonical(chat) {
            return false;
        }
        #[cfg(all(target_os = "windows", feature = "desktop"))]
        return crate::windows_integration::is_foreground(pid);
        #[cfg(not(all(target_os = "windows", feature = "desktop")))]
        {
            let _ = pid;
            true
        }
    }

    #[cfg(all(target_os = "windows", feature = "desktop"))]
    pub(crate) async fn notify_incoming(&self, message: &Message) -> Result<()> {
        // Only fresh, visible inbound messages. Offline history and protocol updates stay silent.
        if message.timestamp < self.connected_at.load(std::sync::atomic::Ordering::Acquire) as i64
            || message.timestamp > crate::model::now() + 30
            || message.from_me
            || !message.visible()
            || message.chat == "status@broadcast"
            || message.timestamp < crate::model::now() - 120
        {
            return Ok(());
        }
        let identities = self.archive.identities().await?;
        if identities.is_locked(&message.chat) {
            return Ok(());
        }
        if !self
            .chat_locks_ready
            .load(std::sync::atomic::Ordering::Acquire)
            && !self.archive.chat_visibility_known(&message.chat).await?
        {
            return Ok(());
        }
        if self.archive.incoming_is_read(message).await? {
            return Ok(());
        }
        let canonical = identities.canonical(&message.chat);
        let viewing = self.desktop_viewing(&message.chat).await;
        let aliases = identities.aliases_json(&message.chat);
        let delivery_sender = if message.chat.ends_with("@g.us") {
            identities.canonical(&message.sender)
        } else {
            "inbound".into()
        };
        let delivery_chat = canonical.clone();
        let (account, chat, sender, id) = (
            self.archive.account as i64,
            message.chat.clone(),
            message.sender.clone(),
            message.id.clone(),
        );
        let rows = self.archive.backend.shared().run(move |conn| {
            conn.transaction::<_, diesel::result::Error, _>(|conn| {
                let inserted = diesel::sql_query("INSERT OR IGNORE INTO brazap_notification_deliveries(account,chat,sender,id) SELECT ?,?,?,? WHERE NOT EXISTS(SELECT 1 FROM brazap_notification_deliveries WHERE account=? AND chat IN (SELECT value FROM json_each(?)) AND id=?)")
                    .bind::<BigInt,_>(account).bind::<Text,_>(&delivery_chat).bind::<Text,_>(&delivery_sender).bind::<Text,_>(&id)
                    .bind::<BigInt,_>(account).bind::<Text,_>(&aliases).bind::<Text,_>(&id).execute(conn)?;
                if inserted == 0 { return Ok(Vec::new()); }
                diesel::sql_query(format!("{MESSAGE_SELECT} WHERE m.account=? AND m.chat=? AND m.sender=? AND m.id=? AND NOT EXISTS(SELECT 1 FROM brazap_chat_settings s WHERE s.account=m.account AND s.jid IN (SELECT value FROM json_each(?)) AND s.muted=1 AND (s.mute_until_ms=-1 OR s.mute_until_ms>CAST(strftime('%s','now') AS INTEGER)*1000))"))
                    .bind::<BigInt,_>(account).bind::<Text,_>(chat).bind::<Text,_>(sender).bind::<Text,_>(id).bind::<Text,_>(&aliases).load::<MessageRow>(conn)
            }).map_err(db_error)
        }).await?;
        let Some(row) = rows.into_iter().next() else {
            return Ok(());
        };
        if viewing {
            return Ok(());
        }
        let message = self
            .archive
            .identities()
            .await?
            .message(crate::search::decode(row)?);
        {
            let mut bursts = self.notification_bursts.lock().await;
            bursts.retain(|_, at| at.elapsed() < Duration::from_secs(60));
            if bursts
                .get(&canonical)
                .is_some_and(|at| at.elapsed() < Duration::from_secs(4))
            {
                return Ok(());
            }
            bursts.insert(canonical, Instant::now());
        }
        let title = if message.chat.ends_with("@g.us") {
            self.archive
                .chats()
                .await?
                .into_iter()
                .find(|chat| chat.jid == message.chat)
                .map(|chat| chat.name)
                .unwrap_or_else(|| "Group message".into())
        } else {
            message.sender_name.clone()
        };
        let preview = if message.chat.ends_with("@g.us") {
            format!("{}: {}", message.sender_name, message.preview())
        } else {
            message.preview()
        };
        let options = self.settings.read().await.notifications.clone();
        if options.enabled {
            let (title, preview) = if options.preview {
                (title, preview)
            } else {
                ("Brazap".into(), "New message".into())
            };
            crate::windows_integration::notify(title, preview, message.chat, options.sound);
        }
        Ok(())
    }
}
