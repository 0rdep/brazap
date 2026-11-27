//! Stable outgoing identities let the desktop show pending sends immediately.
use crate::{
    model::{Message, MessageCursor, now},
    service::Service,
    storage::{Archive, MESSAGE_SELECT, MessageRow, db_error, outgoing},
};
use anyhow::{Result, ensure};
use diesel::{
    prelude::*,
    sql_types::{BigInt, Text},
};
use whatsapp_rust::prelude::{MessageBuilderExt, wa};

/// A caller-chosen message id: WhatsApp's alphabet and length, so it can be sent as is.
pub(crate) fn checked_id(id: String) -> Result<String> {
    ensure!(
        (1..=80).contains(&id.len()) && id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-'),
        "Invalid message ID"
    );
    Ok(id)
}

pub(crate) fn new_id() -> String {
    whatsapp_rust::wacore::request::RequestUtils::message_id_at(None, now() as u64)
}
pub(crate) fn confirmed(status: &str) -> bool {
    matches!(status, "sent" | "delivered" | "read" | "played")
}
impl Archive {
    pub(crate) async fn outgoing_view(&self, chat: &str, id: &str) -> Result<Option<Message>> {
        self.allow_chat(chat).await?;
        let aliases = self.identities().await?.aliases_json(chat);
        let (account, id) = (self.account as i64, id.to_owned());
        let row = self.sql().read(move |conn| {
            diesel::sql_query(format!("{MESSAGE_SELECT} WHERE m.account=? AND m.chat IN (SELECT value FROM json_each(?)) AND m.id=? AND json_extract(m.payload,'$.from_me')=1 LIMIT 1"))
                .bind::<BigInt,_>(account).bind::<Text,_>(aliases).bind::<Text,_>(id)
                .get_result::<MessageRow>(conn).optional().map_err(db_error)
        }).await?;
        row.map(crate::search::decode).transpose()
    }
    pub(crate) async fn recover_pending_sends(&self) -> Result<()> {
        let account = self.account as i64;
        self.sql().run(move |conn| {
            conn.transaction::<_,diesel::result::Error,_>(|conn| {
                diesel::sql_query("UPDATE brazap_messages SET status='failed' WHERE account=? AND status='sending' AND json_extract(payload,'$.from_me')=1")
                    .bind::<BigInt,_>(account).execute(conn)?;
                diesel::sql_query("UPDATE brazap_receipts SET status='failed' WHERE account=? AND status='sending'")
                    .bind::<BigInt,_>(account).execute(conn)?;
                Ok(())
            }).map_err(db_error)
        }).await?;
        Ok(())
    }
    async fn retry_sending(&self, chat: &str, id: &str) -> Result<()> {
        let (account, id) = (self.account as i64, id.to_owned());
        let aliases = self.identities().await?.aliases_json(chat);
        self.sql().run(move|conn| {
            diesel::sql_query("UPDATE brazap_messages SET status='sending' WHERE account=? AND chat IN (SELECT value FROM json_each(?)) AND id=? AND status='failed' AND json_extract(payload,'$.from_me')=1")
                .bind::<BigInt,_>(account).bind::<Text,_>(aliases).bind::<Text,_>(id).execute(conn).map_err(db_error)
        }).await?;
        Ok(())
    }
}
impl Service {
    pub async fn send_text_with_id(
        &self,
        chat: String,
        text: String,
        reply: Option<MessageCursor>,
        id: String,
        timestamp: Option<i64>,
    ) -> Result<Message> {
        self.send_text_with_source(chat, text, reply, None, id, timestamp, Vec::new(), false)
            .await
    }

    /// Sends text that tags group members: each `@<number>` in `text` names one
    /// of `mentions`, which WhatsApp uses to notify them and show their names.
    pub async fn send_text_mentioning(
        &self,
        chat: String,
        text: String,
        reply: Option<MessageCursor>,
        id: String,
        timestamp: Option<i64>,
        mentions: Vec<String>,
        everyone: bool,
    ) -> Result<Message> {
        ensure!(
            mentions.len() <= 1024
                && mentions
                    .iter()
                    .all(|jid| jid.parse::<whatsapp_rust::Jid>().is_ok()),
            "Invalid mention"
        );
        self.send_text_with_source(chat, text, reply, None, id, timestamp, mentions, everyone)
            .await
    }

    pub(crate) async fn send_text_with_source(
        &self,
        chat: String,
        text: String,
        reply: Option<MessageCursor>,
        source_chat: Option<String>,
        id: String,
        timestamp: Option<i64>,
        mentions: Vec<String>,
        everyone: bool,
    ) -> Result<Message> {
        ensure!(
            !text.trim().is_empty() && text.len() <= 65_536,
            "Enter a message up to 65,536 bytes"
        );
        let id = checked_id(id)?;
        let chat = self.archive.canonical_chat(&chat).await?;
        self.archive.allow_chat(&chat).await?;
        // Only identical sends share a lock. Other pending messages can reach
        // SQLite while an earlier network request is still awaiting its ACK.
        let send_lock = {
            let mut locks = self.outbox_locks.lock().await;
            locks.retain(|_, lock| lock.strong_count() > 0);
            let key = (chat.clone(), id.clone());
            if let Some(lock) = locks.get(&key).and_then(std::sync::Weak::upgrade) {
                lock
            } else {
                let lock = std::sync::Arc::new(tokio::sync::Mutex::new(()));
                locks.insert(key, std::sync::Arc::downgrade(&lock));
                lock
            }
        };
        let _guard = send_lock.lock().await;
        let saved = self.archive.outgoing_view(&chat, &id).await?;
        let (mut view, proto) = if let Some(saved) = saved {
            ensure!(
                saved.kind == "text" && saved.text == text,
                "Message ID already belongs to another message"
            );
            if confirmed(&saved.status) {
                return Ok(saved);
            }
            let proto = self
                .archive
                .raw_message(chat.clone(), saved.sender.clone(), id.clone())
                .await?;
            self.archive.retry_sending(&chat, &id).await?;
            (saved, proto)
        } else {
            let jid = chat.parse()?;
            let proto = if let Some(target) = reply {
                let source = source_chat.as_deref().unwrap_or(&chat);
                let original = self
                    .archive
                    .raw_message(source.to_owned(), target.sender.clone(), target.id.clone())
                    .await?;
                let author = self.author(&target.sender).await?;
                let mut context =
                    whatsapp_rust::wacore::proto_helpers::build_quote_context_with_info(
                        target.id,
                        &author,
                        &source.parse()?,
                        &jid,
                        &crate::content::quotable(&original),
                    );
                context.mentioned_jid = mentions;
                if everyone {
                    context.non_jid_mentions = Some(1);
                }
                wa::Message::text_with_context(text, context)
            } else if !mentions.is_empty() || everyone {
                let mut context = wa::ContextInfo::default();
                context.mentioned_jid = mentions;
                // As WhatsApp sends `@all`: the text tag and bit 1 of nonJidMentions.
                if everyone {
                    context.non_jid_mentions = Some(1);
                }
                wa::Message::text_with_context(text, context)
            } else {
                wa::Message::text(text)
            };
            let mut row = outgoing(chat.clone(), id.clone(), &proto);
            row.view.status = "sending".into();
            if let Some(timestamp) = timestamp.filter(|t| t.abs_diff(now()) <= 86_400) {
                row.view.timestamp = timestamp;
            }
            let view = row.view.clone();
            self.archive.store_messages(vec![row], false).await?;
            (view, proto)
        };
        view.status = "sending".into();
        self.changed();
        let result = if self.config.demo {
            Ok(())
        } else {
            async {
                let client = self.connected().await?;
                let jid = chat.parse()?;
                client
                    .send(
                        whatsapp_rust::SendRequest::new(&jid, proto).with_options(
                            whatsapp_rust::SendOptions::default()
                                .with_message_id(whatsapp_rust::MessageId::new(id.clone())?),
                        ),
                    )
                    .await?;
                Ok::<_, anyhow::Error>(())
            }
            .await
        };
        self.archive
            .receipt(
                chat.clone(),
                vec![id.clone()],
                if result.is_ok() { "sent" } else { "failed" }.into(),
            )
            .await?;
        self.changed();
        // An ACK may beat a transport timeout. Never replace confirmed state
        // with a failure just because the awaiting request returned late.
        let current = self
            .archive
            .outgoing_view(&chat, &id)
            .await?
            .unwrap_or(view);
        if confirmed(&current.status) {
            Ok(current)
        } else {
            result?;
            Ok(current)
        }
    }
}

/// Media sends show in their chat as soon as they are staged and upload side by side, but
/// go out in the order they were started, as one send after another would.
#[derive(Default)]
pub(crate) struct SendOrder {
    next: std::sync::atomic::AtomicU64,
    state: std::sync::Mutex<(u64, std::collections::BTreeSet<u64>)>,
    serving: tokio::sync::watch::Sender<u64>,
}

/// A place in [`SendOrder`]; dropping it, sent or not, lets the next send go.
pub(crate) struct Turn<'a> {
    order: &'a SendOrder,
    ticket: u64,
}

impl SendOrder {
    pub(crate) fn ticket(&self) -> Turn<'_> {
        let ticket = self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Turn {
            order: self,
            ticket,
        }
    }
}

impl Turn<'_> {
    /// Waits until every earlier send has gone out or given up.
    pub(crate) async fn wait(&self) {
        let mut serving = self.order.serving.subscribe();
        let _ = serving.wait_for(|at| *at >= self.ticket).await;
    }
}

impl Drop for Turn<'_> {
    fn drop(&mut self) {
        let mut state = self
            .order
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let (serving, done) = &mut *state;
        done.insert(self.ticket);
        while done.remove(serving) {
            *serving += 1;
        }
        let at = *serving;
        drop(state);
        self.order.serving.send_replace(at);
    }
}

impl Service {
    /// Runs a send in the background and answers with its message as soon as `work` has
    /// staged it (sending, in its chat), or with the result when it never staged one.
    pub(crate) async fn send_early<F, Fut>(self: &std::sync::Arc<Self>, work: F) -> Result<Message>
    where
        F: FnOnce(std::sync::Arc<Self>, tokio::sync::oneshot::Sender<Message>) -> Fut,
        Fut: std::future::Future<Output = Result<Message>> + Send + 'static,
    {
        let (staged, early) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(work(self.clone(), staged));
        match early.await {
            Ok(message) => {
                tokio::spawn(async move {
                    if let Ok(Err(error)) = task.await {
                        log::warn!("A message shown as sending could not be sent: {error:#}");
                    }
                });
                Ok(message)
            }
            Err(_) => task.await?,
        }
    }
}
