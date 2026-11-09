//! Status updates share the durable message/media archive but never the chat list.
use crate::{
    model::{Message, StatusUpdate, now},
    service::Service,
    storage::{Archive, db_error},
};
use anyhow::{Context, Result};
use diesel::{
    prelude::*,
    sql_types::{BigInt, Text},
};

pub(crate) const CHAT: &str = "status@broadcast";

#[derive(QueryableByName)]
struct Row {
    #[diesel(sql_type=Text)]
    payload: String,
    #[diesel(sql_type=BigInt)]
    viewed: i64,
}

impl Archive {
    pub(crate) async fn mark_status_views(
        &self,
        sender: Option<&str>,
        ids: &[String],
    ) -> Result<()> {
        let account = self.account as i64;
        let aliases = match sender {
            Some(sender) => self.identities().await?.aliases_json(sender),
            None => "[]".into(),
        };
        let ids = serde_json::to_string(ids)?;
        let when = now();
        self.backend.shared().run(move |conn| {
            diesel::sql_query("INSERT OR IGNORE INTO brazap_status_views(account,sender,id,viewed_at) SELECT account,sender,id,? FROM brazap_messages WHERE account=? AND chat='status@broadcast' AND id IN (SELECT value FROM json_each(?)) AND (?='[]' OR sender IN (SELECT value FROM json_each(?)))")
                .bind::<BigInt,_>(when).bind::<BigInt,_>(account).bind::<Text,_>(ids).bind::<Text,_>(&aliases).bind::<Text,_>(&aliases).execute(conn).map_err(db_error)?;
            Ok(())
        }).await?;
        Ok(())
    }
    pub async fn next_audio(
        &self,
        chat: String,
        cursor: crate::model::MessageCursor,
    ) -> Result<Option<Message>> {
        self.allow_chat(&chat).await?;
        anyhow::ensure!(chat != CHAT, "Use the status viewer for status updates");
        let account = self.account as i64;
        let identities = self.identities().await?;
        let aliases = identities.aliases_json(&chat);
        let row: Option<crate::storage::MessageRow> = self.backend.shared().read(move |conn| {
            diesel::sql_query(format!("{} WHERE m.account=? AND m.chat IN (SELECT value FROM json_each(?)) AND json_extract(m.payload,'$.kind')='audio' AND (m.timestamp,m.id,m.sender)>(?,?,?) AND NOT EXISTS(SELECT 1 FROM brazap_deletions d WHERE d.account=m.account AND d.chat=m.chat AND d.id=m.id) ORDER BY m.timestamp,m.id,m.sender LIMIT 1",crate::storage::MESSAGE_SELECT))
                .bind::<BigInt,_>(account).bind::<Text,_>(aliases).bind::<BigInt,_>(cursor.timestamp).bind::<Text,_>(cursor.id).bind::<Text,_>(cursor.sender)
                .get_result(conn).optional().map_err(db_error)
        }).await?;
        row.map(crate::search::decode)
            .transpose()
            .map(|m| m.map(|m| identities.message(m)))
    }

    pub async fn statuses(&self) -> Result<Vec<StatusUpdate>> {
        let account = self.account as i64;
        let cutoff = now() - 24 * 3600;
        let rows: Vec<Row> = self.backend.shared().read(move |conn| {
            diesel::sql_query("SELECT m.payload, EXISTS(SELECT 1 FROM brazap_status_views v WHERE v.account=m.account AND v.sender=m.sender AND v.id=m.id) AS viewed FROM brazap_messages m WHERE m.account=? AND m.chat='status@broadcast' AND m.timestamp>? AND json_extract(m.payload,'$.kind') IN ('text','image','video','audio') AND NOT EXISTS(SELECT 1 FROM brazap_deletions d WHERE d.account=m.account AND d.chat=m.chat AND d.id=m.id) ORDER BY m.timestamp,m.id LIMIT 1000")
                .bind::<BigInt,_>(account).bind::<BigInt,_>(cutoff).load(conn).map_err(db_error)
        }).await?;
        let identities = self.identities().await?;
        let mut updates = Vec::new();
        for row in rows {
            let mut message: Message = serde_json::from_str(&row.payload)?;
            if identities.is_locked(&message.sender) {
                continue;
            }
            if message.from_me {
                message.sender_name = "My status".into();
            } else {
                let (name, phone) = identities.person(&message.sender, &message.sender_name);
                message.sender_name = name;
                message.sender_phone = phone;
            }
            updates.push(StatusUpdate {
                message,
                viewed: row.viewed != 0,
            });
        }
        Ok(updates)
    }
}

impl Service {
    pub async fn reply_status(
        &self,
        sender: String,
        id: String,
        text: String,
        message_id: String,
    ) -> Result<Message> {
        let update = self
            .archive
            .statuses()
            .await?
            .into_iter()
            .find(|s| s.message.sender == sender && s.message.id == id)
            .context("This status has expired or is unavailable")?;
        anyhow::ensure!(
            !update.message.from_me,
            "You cannot reply to your own status"
        );
        let jid: whatsapp_rust::Jid = sender.parse()?;
        anyhow::ensure!(
            matches!(jid.server.as_str(), "s.whatsapp.net" | "lid"),
            "This status does not accept replies"
        );
        self.send_text_with_source(
            sender,
            text,
            Some(update.message.cursor()),
            Some(CHAT.into()),
            message_id,
            None,
        )
        .await
    }

    pub async fn view_status(
        self: &std::sync::Arc<Self>,
        sender: String,
        id: String,
    ) -> Result<()> {
        let status = self
            .archive
            .statuses()
            .await?
            .into_iter()
            .find(|s| s.message.sender == sender && s.message.id == id)
            .context("This status has expired or is unavailable")?;
        if status.viewed {
            return Ok(());
        }
        self.archive
            .mark_status_views(Some(&sender), std::slice::from_ref(&id))
            .await?;
        self.changed();
        if !status.message.from_me
            && self.settings.read().await.privacy.send_read_receipts
            && let Ok(client) = self.connected().await
        {
            tokio::spawn(async move {
                let result = async {
                    client
                        .mark_as_read(&CHAT.parse()?, Some(&sender.parse()?), &[id.as_str()])
                        .await
                }
                .await;
                if let Err(error) = result {
                    log::debug!("Could not send status view receipt: {error}");
                }
            });
        }
        Ok(())
    }
}
