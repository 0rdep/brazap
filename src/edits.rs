//! Editing text messages and media captions, and applying edits from WhatsApp.
//!
//! A MESSAGE_EDIT protocol message stays in `brazap_messages` with its raw
//! protobuf, like any other message, so the archive can be reprocessed.
//! `brazap_edits` holds the latest edit of each message, derived from those
//! rows. Reads overlay it on the original, whose payload and `encoded` bytes
//! never change, and the search index follows the edited text.
use std::sync::Arc;

use anyhow::{Context, Result, bail, ensure};
use diesel::{
    prelude::*,
    sql_types::{BigInt, Binary, Text},
};
use whatsapp_rust::{
    prelude::{MessageBuilderExt, MessageExt, wa},
    wacore::msg_secret::{EDIT_PROCESSING_WINDOW_SECS, ResolvedMessageSecret},
    waproto::buffa::{Message as ProtoMessage, MessageField},
};

use crate::{
    model::{EditUpdate, Message, now},
    service::Service,
    storage::{Archive, db_error},
};

/// How long after sending WhatsApp's apps offer to edit a message. Receivers
/// accept edits made within `EDIT_PROCESSING_WINDOW_SECS` (20 minutes) of the
/// original, so this leaves room for clock differences.
pub(crate) const SEND_WINDOW_SECS: i64 = 15 * 60;

/// Project a MESSAGE_EDIT protocol message. Returns whether `base` was one.
pub(crate) fn project(view: &mut Message, base: &wa::Message) -> bool {
    // Secret-encrypted add-ons (edits, poll and event changes) the library
    // could not decrypt are never content of their own.
    if base.secret_encrypted_message.is_set() {
        view.kind = "protocol".into();
        return true;
    }
    let Some(protocol) = base.protocol_message.as_option() else {
        return false;
    };
    if protocol.r#type != Some(wa::message::protocol_message::Type::MESSAGE_EDIT) {
        return false;
    }
    view.kind = "protocol".into();
    let Some(edited) = protocol.edited_message.as_option() else {
        return true;
    };
    let target_id = protocol
        .key
        .as_option()
        .and_then(|key| key.id.clone())
        .unwrap_or_default();
    let text = edited
        .text_content()
        .or_else(|| edited.get_caption())
        .unwrap_or("")
        .to_owned();
    // Mentions and link previews come from the new content, as WhatsApp shows them.
    let mut content = Message {
        chat: view.chat.clone(),
        sender: view.sender.clone(),
        from_me: view.from_me,
        timestamp: view.timestamp,
        text: text.clone(),
        kind: "text".into(),
        ..Default::default()
    };
    crate::content::project(&mut content, edited);
    let update = EditUpdate {
        target_id,
        text,
        mentions: content.mentions,
        link_preview: content.link_preview,
        timestamp_ms: protocol
            .timestamp_ms
            .unwrap_or(view.timestamp.saturating_mul(1000)),
    };
    if update.target_id == view.id {
        // A message stored with its own latest edit: show that content.
        let edited_at = protocol
            .timestamp_ms
            .map(|ms| ms / 1000)
            .filter(|seconds| *seconds > 0);
        view.kind = "text".into();
        view.text = update.text.clone();
        crate::content::project(view, edited);
        view.apply_edit(&update, edited_at);
    } else {
        view.edit_update = Some(update);
    }
    true
}

#[derive(QueryableByName)]
struct Target {
    #[diesel(sql_type = Text)]
    chat: String,
    #[diesel(sql_type = Text)]
    sender: String,
    #[diesel(sql_type = BigInt)]
    timestamp: i64,
    #[diesel(sql_type = Text)]
    payload: String,
}

#[derive(QueryableByName)]
struct EditRow {
    #[diesel(sql_type = Text)]
    chat: String,
    #[diesel(sql_type = Text)]
    sender: String,
    #[diesel(sql_type = Text)]
    edit_id: String,
    #[diesel(sql_type = BigInt)]
    edited_at: i64,
    #[diesel(sql_type = Text)]
    content: String,
}

fn json_error(error: serde_json::Error) -> diesel::result::Error {
    diesel::result::Error::SerializationError(Box::new(error))
}

/// Keep the newest edit of a message; replays of the same edit change nothing.
fn upsert(
    conn: &mut SqliteConnection,
    account: i64,
    (chat, sender, id): (&str, &str, &str),
    edit_id: &str,
    edited_at: i64,
    content: &str,
) -> QueryResult<usize> {
    diesel::sql_query("INSERT INTO brazap_edits(account,chat,sender,id,edit_id,edited_at,content) VALUES(?,?,?,?,?,?,?) ON CONFLICT(account,chat,sender,id) DO UPDATE SET edit_id=excluded.edit_id,edited_at=excluded.edited_at,content=excluded.content WHERE (excluded.edited_at,COALESCE(json_extract(excluded.content,'$.timestamp_ms'),0),excluded.edit_id)>=(brazap_edits.edited_at,COALESCE(json_extract(brazap_edits.content,'$.timestamp_ms'),0),brazap_edits.edit_id)")
        .bind::<BigInt, _>(account).bind::<Text, _>(chat).bind::<Text, _>(sender).bind::<Text, _>(id)
        .bind::<Text, _>(edit_id).bind::<BigInt, _>(edited_at).bind::<Text, _>(content)
        .execute(conn)
}

/// Overlay the stored edit of `m` (by its exact identity) and remember the
/// edited preview for the chat list. Returns whether there was one.
fn apply_stored(conn: &mut SqliteConnection, account: i64, m: &mut Message) -> QueryResult<bool> {
    let row: Option<EditRow> = diesel::sql_query("SELECT chat,sender,edit_id,edited_at,content FROM brazap_edits WHERE account=? AND chat=? AND sender=? AND id=?")
        .bind::<BigInt, _>(account).bind::<Text, _>(&m.chat).bind::<Text, _>(&m.sender).bind::<Text, _>(&m.id)
        .get_result(conn).optional()?;
    let Some(edit) = row.and_then(|row| {
        serde_json::from_str::<EditUpdate>(&row.content)
            .ok()
            .map(|edit| (edit, row.edited_at))
    }) else {
        return Ok(false);
    };
    m.apply_edit(&edit.0, Some(edit.1));
    diesel::sql_query(
        "UPDATE brazap_edits SET preview=? WHERE account=? AND chat=? AND sender=? AND id=?",
    )
    .bind::<Text, _>(m.preview())
    .bind::<BigInt, _>(account)
    .bind::<Text, _>(&m.chat)
    .bind::<Text, _>(&m.sender)
    .bind::<Text, _>(&m.id)
    .execute(conn)?;
    Ok(true)
}

/// Record the edit an incoming or outgoing MESSAGE_EDIT carries. When its
/// target is archived, the edit is filed under the target's identity and the
/// search index is updated; otherwise it waits for the target (see `overlay`).
/// `aliases` and `senders` are the edit's chat and author aliases.
pub(crate) fn store(
    conn: &mut SqliteConnection,
    account: i64,
    m: &Message,
    aliases: &str,
    senders: &str,
) -> QueryResult<()> {
    let Some(edit) = &m.edit_update else {
        return Ok(());
    };
    if edit.target_id.is_empty() || edit.target_id == m.id {
        return Ok(());
    }
    // Only the author edits a message: in a group the target has the edit's
    // sender; in a direct chat, the same direction.
    let target: Option<Target> = diesel::sql_query("SELECT chat,sender,timestamp,payload FROM brazap_messages WHERE account=? AND chat IN (SELECT value FROM json_each(?)) AND (? NOT LIKE '%@g.us' OR sender IN (SELECT value FROM json_each(?))) AND id=? AND json_extract(payload,'$.from_me')=? AND json_extract(payload,'$.kind')!='protocol' ORDER BY rowid LIMIT 1")
        .bind::<BigInt, _>(account).bind::<Text, _>(aliases).bind::<Text, _>(&m.chat).bind::<Text, _>(senders)
        .bind::<Text, _>(&edit.target_id).bind::<BigInt, _>(i64::from(m.from_me))
        .get_result(conn).optional()?;
    // WhatsApp drops edits made too long after the original.
    if target
        .as_ref()
        .is_some_and(|t| m.timestamp >= t.timestamp + EDIT_PROCESSING_WINDOW_SECS)
    {
        return Ok(());
    }
    let (chat, sender) = target
        .as_ref()
        .map_or((m.chat.as_str(), m.sender.as_str()), |t| {
            (t.chat.as_str(), t.sender.as_str())
        });
    let content = serde_json::to_string(edit).map_err(json_error)?;
    upsert(
        conn,
        account,
        (chat, sender, &edit.target_id),
        &m.id,
        m.timestamp,
        &content,
    )?;
    if let Some(target) = target {
        let mut view: Message = serde_json::from_str(&target.payload).map_err(json_error)?;
        view.chat = target.chat;
        view.sender = target.sender;
        if apply_stored(conn, account, &mut view)? {
            crate::storage::index_message(conn, account, &view)?;
        }
    }
    Ok(())
}

/// Apply an edit that arrived before its message (out of order or through
/// history sync) to the message being stored, so its search text and chat
/// preview use the latest version. The stored payload stays unedited.
pub(crate) fn overlay(
    conn: &mut SqliteConnection,
    account: i64,
    m: &mut Message,
    aliases: &str,
    senders: &str,
) -> QueryResult<()> {
    if m.edit_update.is_some() || m.edited {
        return Ok(());
    }
    let row: Option<EditRow> = diesel::sql_query("SELECT chat,sender,edit_id,edited_at,content FROM brazap_edits WHERE account=? AND chat IN (SELECT value FROM json_each(?)) AND sender IN (SELECT value FROM json_each(?)) AND id=? ORDER BY edited_at DESC,COALESCE(json_extract(content,'$.timestamp_ms'),0) DESC,edit_id DESC LIMIT 1")
        .bind::<BigInt, _>(account).bind::<Text, _>(aliases).bind::<Text, _>(senders).bind::<Text, _>(&m.id)
        .get_result(conn).optional()?;
    let Some(row) = row else {
        return Ok(());
    };
    if row.edited_at >= m.timestamp + EDIT_PROCESSING_WINDOW_SECS {
        diesel::sql_query(
            "DELETE FROM brazap_edits WHERE account=? AND chat=? AND sender=? AND id=?",
        )
        .bind::<BigInt, _>(account)
        .bind::<Text, _>(&row.chat)
        .bind::<Text, _>(&row.sender)
        .bind::<Text, _>(&m.id)
        .execute(conn)?;
        return Ok(());
    }
    if (row.chat.as_str(), row.sender.as_str()) != (m.chat.as_str(), m.sender.as_str()) {
        // File the edit under the message's own identity, which reads join on.
        diesel::sql_query(
            "DELETE FROM brazap_edits WHERE account=? AND chat=? AND sender=? AND id=?",
        )
        .bind::<BigInt, _>(account)
        .bind::<Text, _>(&row.chat)
        .bind::<Text, _>(&row.sender)
        .bind::<Text, _>(&m.id)
        .execute(conn)?;
        upsert(
            conn,
            account,
            (&m.chat, &m.sender, &m.id),
            &row.edit_id,
            row.edited_at,
            &row.content,
        )?;
    }
    apply_stored(conn, account, m)?;
    Ok(())
}

#[derive(serde::Deserialize)]
struct StoredEdit {
    at: i64,
    content: EditUpdate,
}

/// Overlay the `edit` column of `queries/messages.sql` on a loaded message.
pub(crate) fn apply_row(message: &mut Message, edit: Option<&str>) {
    if let Some(edit) = edit.and_then(|value| serde_json::from_str::<StoredEdit>(value).ok()) {
        message.apply_edit(&edit.content, Some(edit.at));
    }
}

/// The new content in the shape of the original: plain text for text
/// messages, and the same media message with a new caption for captions.
fn edited_content(original: &wa::Message, kind: &str, text: String) -> Result<wa::Message> {
    let base = original.get_base_message();
    let mut content = wa::Message::default();
    match kind {
        "text" => return Ok(wa::Message::text(text)),
        "image" => {
            let mut media = base
                .image_message
                .as_option()
                .context("This photo cannot be edited")?
                .clone();
            media.caption = Some(text);
            content.image_message = MessageField::some(media);
        }
        "video" => {
            let mut media = base
                .video_message
                .as_option()
                .context("This video cannot be edited")?
                .clone();
            media.caption = Some(text);
            content.video_message = MessageField::some(media);
        }
        "file" => {
            let mut media = base
                .document_message
                .as_option()
                .context("This document cannot be edited")?
                .clone();
            media.caption = Some(text);
            content.document_message = MessageField::some(media);
        }
        _ => bail!("Only text messages and captions can be edited"),
    }
    Ok(content)
}

/// The protocol message WhatsApp sends for an edit (see whatsapp-rust's
/// `build_edit_message`), for demo mode, which sends nothing.
fn local_edit(chat: &str, id: &str, content: wa::Message) -> wa::Message {
    let mut key = wa::MessageKey::default();
    key.remote_jid = Some(chat.into());
    key.from_me = Some(true);
    key.id = Some(id.into());
    let mut protocol = wa::message::ProtocolMessage::default();
    protocol.key = MessageField::some(key);
    protocol.r#type = Some(wa::message::protocol_message::Type::MESSAGE_EDIT);
    protocol.edited_message = MessageField::some(content);
    protocol.timestamp_ms = Some(chrono::Utc::now().timestamp_millis());
    let mut message = wa::Message::default();
    message.protocol_message = MessageField::some(protocol);
    message
}

impl Service {
    /// Edit the text or caption of one of your own recent messages.
    pub async fn edit_message(&self, chat: String, id: String, text: String) -> Result<Message> {
        ensure!(
            !text.trim().is_empty() && text.len() <= 65_536,
            "Enter the new text, up to 65,536 bytes"
        );
        let original = self
            .archive
            .outgoing_view(&chat, &id)
            .await?
            .context("You can only edit messages you sent")?;
        if let Some(reason) = original.edit_blocker(now()) {
            bail!(reason);
        }
        if original.text == text {
            return Ok(original);
        }
        let raw = self
            .archive
            .raw_message(original.chat.clone(), original.sender.clone(), id.clone())
            .await?;
        let content = edited_content(&raw, &original.kind, text)?;
        let (edit_id, proto) = if self.config.demo {
            (
                format!("demo-{}", uuid::Uuid::new_v4()),
                local_edit(&original.chat, &id, content),
            )
        } else {
            let client = self.connected().await?;
            let jid: whatsapp_rust::Jid = original.chat.parse()?;
            let target = whatsapp_rust::MessageRef::new(
                &jid,
                whatsapp_rust::MessageId::new(&id)?,
                None,
                true,
            )?;
            let sent = client
                .edit_message(whatsapp_rust::EditRequest::new(target, content))
                .await?;
            (
                sent.message_id.to_string(),
                Arc::unwrap_or_clone(sent.message),
            )
        };
        // Archive the edit as sent so the UI and MCP show the new text at once.
        let mut row = crate::storage::outgoing(original.chat.clone(), edit_id, &proto);
        row.view.status = "sent".into();
        self.archive.store_messages(vec![row], false).await?;
        self.changed();
        self.archive
            .outgoing_view(&original.chat, &id)
            .await?
            .context("The edited message is no longer saved")
    }
}

/// Supplies the library with a message's `messageSecret` from the archive,
/// so it can decrypt secret-encrypted edits of messages its own secret store
/// no longer has (or never saw, such as history-synced ones).
pub(crate) struct SecretResolver {
    pub archive: Archive,
}

#[async_trait::async_trait]
impl whatsapp_rust::OriginalMessageResolver for SecretResolver {
    async fn resolve_msg_secret(&self, chat: &str, sender: &str, msg_id: &str) -> Option<[u8; 32]> {
        self.resolve_msg_secret_with_metadata(chat, sender, msg_id)
            .await
            .map(|found| found.secret)
    }

    async fn resolve_msg_secret_with_metadata(
        &self,
        chat: &str,
        sender: &str,
        msg_id: &str,
    ) -> Option<ResolvedMessageSecret> {
        match self.archive.message_secret(chat, sender, msg_id).await {
            Ok(found) => found,
            Err(error) => {
                log::debug!("Could not read a message secret: {error:#}");
                None
            }
        }
    }
}

#[derive(QueryableByName)]
struct SecretRow {
    #[diesel(sql_type = Binary)]
    encoded: Vec<u8>,
    #[diesel(sql_type = BigInt)]
    timestamp: i64,
}

impl Archive {
    async fn message_secret(
        &self,
        chat: &str,
        sender: &str,
        id: &str,
    ) -> Result<Option<ResolvedMessageSecret>> {
        let identities = self.identities().await?;
        let chats = identities.aliases_json(chat);
        let mut senders = identities.aliases(sender);
        if self.own_jids().await?.iter().any(|own| own == sender) {
            senders.push("self".into());
        }
        let senders = serde_json::to_string(&senders)?;
        let (account, id) = (self.account as i64, id.to_owned());
        let row: Option<SecretRow> = self.sql().read(move |conn| {
            diesel::sql_query("SELECT encoded,timestamp FROM brazap_messages WHERE account=? AND chat IN (SELECT value FROM json_each(?)) AND sender IN (SELECT value FROM json_each(?)) AND id=? AND json_extract(payload,'$.kind')!='protocol' LIMIT 1")
                .bind::<BigInt, _>(account).bind::<Text, _>(chats).bind::<Text, _>(senders).bind::<Text, _>(id)
                .get_result(conn).optional().map_err(db_error)
        }).await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let message = wa::Message::decode_from_slice(&row.encoded)?;
        Ok(message
            .message_context_info
            .as_option()
            .and_then(|info| info.message_secret.as_deref())
            .and_then(|secret| <[u8; 32]>::try_from(secret).ok())
            .map(|secret| ResolvedMessageSecret {
                secret,
                message_ts: Some(row.timestamp).filter(|t| *t > 0),
            }))
    }
}
