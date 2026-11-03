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
}

pub fn chat_from_link(link: &str) -> Result<String> {
    let chat = link.strip_prefix("brazap://chat/").unwrap_or(link);
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
        let chat = chat.map(|chat| chat_from_link(&chat)).transpose()?;
        let mut desktop = self.desktop.lock().await;
        let available = desktop
            .seen
            .is_some_and(|seen| seen.elapsed() < Duration::from_secs(5));
        if available {
            desktop.activation = Some(chat);
        }
        Ok(json!({"accepted": available, "pid": desktop.pid}))
    }

    pub async fn poll_desktop(&self, pid: u32, chat: Option<String>) -> Value {
        let mut desktop = self.desktop.lock().await;
        desktop.seen = Some(Instant::now());
        desktop.pid = pid;
        desktop.chat = chat;
        match desktop.activation.take() {
            Some(chat) => json!({"activate": true, "chat": chat}),
            None => json!({"activate": false}),
        }
    }

    #[cfg(all(target_os = "windows", feature = "desktop"))]
    pub(crate) async fn notify_incoming(&self, message: &Message) -> Result<()> {
        // Only fresh, visible inbound messages. Offline history and protocol updates stay silent.
        if !self
            .chat_locks_ready
            .load(std::sync::atomic::Ordering::Acquire)
            || !self
                .notifications_ready
                .load(std::sync::atomic::Ordering::Acquire)
            || message.timestamp
                < self.connected_at.load(std::sync::atomic::Ordering::Acquire) as i64
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
        let canonical = identities.canonical(&message.chat);
        let (pid, active_chat) = {
            let desktop = self.desktop.lock().await;
            (desktop.pid, desktop.chat.clone())
        };
        let viewing = active_chat
            .as_ref()
            .is_some_and(|chat| identities.canonical(chat) == canonical)
            && crate::windows_integration::is_foreground(pid);
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
