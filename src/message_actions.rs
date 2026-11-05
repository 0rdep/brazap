use crate::{
    model::{ForwardResult, Message, MessageCursor, ReactionUpdate},
    service::Service,
};
use anyhow::{Context, Result, ensure};
use diesel::{
    prelude::*,
    sql_types::{BigInt, Text},
};
use whatsapp_rust::{
    prelude::{MessageExt, wa},
    wacore_binary::JidExt,
};

pub(crate) fn store_reaction(
    conn: &mut SqliteConnection,
    account: i64,
    message: &Message,
) -> diesel::QueryResult<()> {
    if let Some(update) = &message.reaction_update {
        if update.target_id.is_empty() {
            return Ok(());
        }
        diesel::sql_query("INSERT INTO brazap_reactions(account,chat,target_sender,target_id,reactor,emoji,timestamp_ms) VALUES(?,?,?,?,?,?,?) ON CONFLICT(account,chat,target_sender,target_id,reactor) DO UPDATE SET emoji=excluded.emoji,timestamp_ms=excluded.timestamp_ms WHERE excluded.timestamp_ms>=brazap_reactions.timestamp_ms")
            .bind::<BigInt,_>(account).bind::<Text,_>(&message.chat)
            .bind::<Text,_>(&update.target_sender).bind::<Text,_>(&update.target_id)
            .bind::<Text,_>(&message.sender).bind::<Text,_>(&update.emoji)
            .bind::<BigInt,_>(update.timestamp_ms).execute(conn)?;
    }
    Ok(())
}

impl Service {
    pub(crate) async fn author(&self, sender: &str) -> Result<whatsapp_rust::Jid> {
        if sender == "self" {
            if self.config.demo {
                return Ok(crate::demo::OWN.parse()?);
            }
            let client = self.connected().await?;
            client
                .lid()
                .or_else(|| client.pn())
                .context("Your WhatsApp identity is unavailable")
        } else {
            Ok(sender.parse()?)
        }
    }

    pub async fn reply_text(
        &self,
        chat: String,
        text: String,
        target: MessageCursor,
    ) -> Result<Message> {
        self.send_text_with_id(chat, text, Some(target), crate::outbox::new_id(), None)
            .await
    }

    pub async fn react(&self, chat: String, target: MessageCursor, emoji: String) -> Result<()> {
        ensure!(emoji.chars().count() <= 16, "Choose a single emoji");
        // Verify the exact archived target before sending an add-on.
        let source = self
            .archive
            .message_source(&chat, &target.sender, &target.id)
            .await?;
        self.archive
            .raw_message(chat.clone(), target.sender.clone(), target.id.clone())
            .await?;
        let jid: whatsapp_rust::Jid = chat.parse()?;
        let mut key = wa::MessageKey::default();
        key.remote_jid = Some(chat.clone());
        key.from_me = Some(target.sender == "self");
        key.id = Some(target.id.clone());
        key.participant = if jid.is_group() {
            Some(self.author(&target.sender).await?.to_string())
        } else {
            None
        };
        if !self.config.demo {
            self.connected()
                .await?
                .send_reaction_raw(jid, key, &emoji)
                .await?;
        }
        let message = Message {
            chat: source.chat,
            sender: "self".into(),
            reaction_update: Some(ReactionUpdate {
                target_sender: source.sender,
                target_id: target.id,
                emoji,
                timestamp_ms: chrono::Utc::now().timestamp_millis(),
            }),
            ..Default::default()
        };
        let account = self.archive.account as i64;
        self.archive
            .backend
            .shared()
            .run(move |conn| {
                store_reaction(conn, account, &message).map_err(crate::storage::db_error)
            })
            .await?;
        self.changed();
        Ok(())
    }

    pub async fn forward(
        &self,
        source: String,
        destination: String,
        targets: Vec<MessageCursor>,
    ) -> Result<ForwardResult> {
        ensure!(
            !targets.is_empty() && targets.len() <= 50,
            "Select between 1 and 50 messages"
        );
        let jid: whatsapp_rust::Jid = destination.parse()?;
        ensure!(
            !jid.is_broadcast_list() && !jid.is_status_broadcast(),
            "Choose a person or group"
        );
        let client = if self.config.demo {
            None
        } else {
            Some(self.connected().await?)
        };
        let mut prepared = Vec::new();
        let mut unique = std::collections::HashSet::new();
        let mut targets = targets;
        targets
            .sort_by(|a, b| (a.timestamp, &a.id, &a.sender).cmp(&(b.timestamp, &b.id, &b.sender)));
        // Resolve everything first so a missing local target cannot cause a partial send.
        for target in targets {
            if !unique.insert((target.sender.clone(), target.id.clone())) {
                continue;
            }
            let original = self
                .archive
                .raw_message(source.clone(), target.sender.clone(), target.id.clone())
                .await?;
            let projection = crate::storage::project_message(
                source.clone(),
                target.sender.clone(),
                target.id.clone(),
                target.sender == "self",
                target.timestamp,
                &original,
                String::new(),
                String::new(),
            )
            .view;
            ensure!(projection.actionable(), "This message cannot be forwarded");
            prepared.push((target, original));
        }
        let mut result = ForwardResult {
            destination,
            sent: Vec::new(),
            error: None,
        };
        for (target, original) in prepared {
            let forwarded = original.get_base_message().prepare_for_forward();
            let media = match self
                .archive
                .media(source.clone(), target.sender.clone(), target.id.clone())
                .await
            {
                Ok(media) => media,
                Err(error) => {
                    result.error = Some(format!(
                        "Forwarded {} message(s). {error:#}",
                        result.sent.len()
                    ));
                    break;
                }
            };
            let sent = if let Some(client) = &client {
                self.send_prepared(client.clone(), jid.clone(), forwarded, media)
                    .await
            } else {
                self.demo_send(result.destination.clone(), forwarded, media)
                    .await
            };
            match sent {
                Ok(_) => result.sent.push(target),
                Err(error) => {
                    result.error = Some(format!(
                        "Forwarded {} message(s). {error:#}",
                        result.sent.len()
                    ));
                    break;
                }
            }
        }
        Ok(result)
    }
}
