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
    prelude::{MessageExt, MessageField, wa},
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

    /// Answer a business message's quick-reply button the way WhatsApp's own apps do: with a
    /// response that quotes the message. The button is read from the archived original, so a
    /// caller cannot make the account send anything the business did not offer.
    pub async fn press_button(
        &self,
        chat: String,
        target: MessageCursor,
        button: usize,
    ) -> Result<Message> {
        let original = self
            .archive
            .raw_message(chat.clone(), target.sender.clone(), target.id.clone())
            .await?;
        let mut view = Message::default();
        crate::content::project(&mut view, &original);
        let action = view
            .actions
            .get(button)
            .filter(|action| action.kind == "reply")
            .context("This button cannot be answered from here")?
            .clone();
        let jid: whatsapp_rust::Jid = chat.parse()?;
        let context = whatsapp_rust::wacore::proto_helpers::build_quote_context_with_info(
            target.id.clone(),
            &self.author(&target.sender).await?,
            &jid,
            &jid,
            &original,
        );
        let id = action.id.clone().unwrap_or_default();
        let mut message = wa::Message::default();
        match action.flow.as_str() {
            "template" => {
                let mut reply = wa::message::TemplateButtonReplyMessage::default();
                reply.selected_id = Some(id);
                reply.selected_display_text = Some(action.label.clone());
                reply.selected_index = Some(action.index);
                reply.context_info = MessageField::some(context);
                message.template_button_reply_message = MessageField::some(reply);
            }
            "list" => {
                use wa::message::list_response_message::{ListType, SingleSelectReply};
                let mut select = SingleSelectReply::default();
                select.selected_row_id = Some(id);
                let mut reply = wa::message::ListResponseMessage::default();
                reply.title = Some(action.label.clone());
                reply.description = action.target.clone();
                reply.list_type = Some(ListType::SINGLE_SELECT);
                reply.single_select_reply = MessageField::some(select);
                reply.context_info = MessageField::some(context);
                message.list_response_message = MessageField::some(reply);
            }
            "buttons" => {
                use wa::message::buttons_response_message::{Response, Type};
                let mut reply = wa::message::ButtonsResponseMessage::default();
                reply.selected_button_id = Some(id);
                reply.response = Some(Response::SelectedDisplayText(action.label.clone()));
                reply.r#type = Some(Type::DISPLAY_TEXT);
                reply.context_info = MessageField::some(context);
                message.buttons_response_message = MessageField::some(reply);
            }
            _ => {
                use wa::message::interactive_response_message::{Body, NativeFlowResponseMessage};
                let mut body = Body::default();
                body.text = Some(action.label.clone());
                let mut flow = NativeFlowResponseMessage::default();
                flow.name = Some("quick_reply".into());
                flow.params_json =
                    Some(serde_json::json!({"display_text": action.label, "id": id}).to_string());
                flow.version = Some(3);
                let mut reply = wa::message::InteractiveResponseMessage::default();
                reply.body = MessageField::some(body);
                reply.interactive_response_message = Some(
                    wa::message::interactive_response_message::InteractiveResponseMessage::NativeFlowResponseMessage(Box::new(flow)),
                );
                reply.context_info = MessageField::some(context);
                message.interactive_response_message = MessageField::some(reply);
            }
        }
        if self.config.demo {
            return self.demo_send(chat, message, None).await;
        }
        let client = self.connected().await?;
        self.send_prepared(client, jid, message, None).await
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
