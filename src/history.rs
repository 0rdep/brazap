use crate::{
    model::{Message, MessageCursor, MessagePage},
    search::decode,
    storage::{
        Archive, MESSAGE_SELECT, MessageRow, db_error, message_payload, project_message, searchable,
    },
};
use anyhow::{Context, Result};
use diesel::{
    prelude::*,
    sql_types::{BigInt, Binary, Text},
};
use whatsapp_rust::{prelude::wa, waproto::buffa::Message as ProtoMessage};

pub(crate) const CHAT_PAGE_SIZE: usize = 30;

fn timeline_select() -> String {
    // ORDER BY can otherwise tempt SQLite into scanning the account-wide
    // search index, especially for an alias with few or no stored messages.
    MESSAGE_SELECT.replacen(
        "FROM brazap_messages m",
        "FROM brazap_messages m INDEXED BY brazap_messages_timeline",
        1,
    )
}

#[derive(QueryableByName)]
struct RawRow {
    #[diesel(sql_type=BigInt)]
    cursor: i64,
    #[diesel(sql_type=Text)]
    payload: String,
    #[diesel(sql_type=Text)]
    metadata: String,
    #[diesel(sql_type=Binary)]
    encoded: Vec<u8>,
}
#[derive(QueryableByName)]
struct Chunk {
    #[diesel(sql_type=BigInt)]
    cursor: i64,
    #[diesel(sql_type=Binary)]
    data: Vec<u8>,
}

impl Archive {
    pub async fn message_page(
        &self,
        chat: String,
        limit: usize,
        cursor: Option<MessageCursor>,
    ) -> Result<MessagePage> {
        self.allow_chat(&chat).await?;
        let account = self.account as i64;
        let limit = limit.clamp(1, 500);
        let identities = self.identities().await?;
        let aliases = identities.aliases_json(&chat);
        let sources = identities.aliases(&chat);
        let scope = identities.timeline_scope(&chat);
        let select = timeline_select();
        let cursor = cursor.unwrap_or(MessageCursor {
            timestamp: i64::MAX,
            id: String::new(),
            sender: String::new(),
        });
        let rows:Vec<MessageRow>=self.backend.shared().read(move |conn| {
            let mut rows=Vec::new();
            for source in sources {
            rows.extend(diesel::sql_query(format!("{select} WHERE m.account=? AND {scope} AND json_extract(m.payload,'$.kind')!='protocol' AND (m.timestamp,m.id,m.sender)<(?,?,?) ORDER BY m.timestamp DESC,m.id DESC,m.sender DESC LIMIT ?"))
                .bind::<BigInt,_>(account).bind::<Text,_>(source).bind::<Text,_>(&aliases).bind::<BigInt,_>(cursor.timestamp).bind::<Text,_>(&cursor.id).bind::<Text,_>(&cursor.sender).bind::<BigInt,_>((limit+1) as i64).load::<MessageRow>(conn).map_err(db_error)?);
            } Ok(rows)
        }).await?;
        let mut messages = rows
            .into_iter()
            .map(|row| decode(row).map(|m| identities.message(m)))
            .collect::<Result<Vec<_>>>()?;
        messages
            .sort_by(|a, b| (b.timestamp, &b.id, &b.sender).cmp(&(a.timestamp, &a.id, &a.sender)));
        let has_older = messages.len() > limit;
        messages.truncate(limit);
        messages.reverse();
        Ok(MessagePage {
            messages,
            has_older,
            has_newer: false,
            notice: None,
        })
    }

    pub async fn newer_message_page(
        &self,
        chat: String,
        limit: usize,
        cursor: MessageCursor,
    ) -> Result<MessagePage> {
        self.allow_chat(&chat).await?;
        let account = self.account as i64;
        let limit = limit.clamp(1, 500);
        let identities = self.identities().await?;
        let aliases = identities.aliases_json(&chat);
        let sources = identities.aliases(&chat);
        let scope = identities.timeline_scope(&chat);
        let select = timeline_select();
        let rows: Vec<MessageRow> = self.backend.shared().read(move |conn| {
            let mut rows = Vec::new();
            for source in sources {
                rows.extend(diesel::sql_query(format!("{select} WHERE m.account=? AND {scope} AND json_extract(m.payload,'$.kind')!='protocol' AND (m.timestamp,m.id,m.sender)>(?,?,?) ORDER BY m.timestamp,m.id,m.sender LIMIT ?"))
                    .bind::<BigInt,_>(account).bind::<Text,_>(source).bind::<Text,_>(&aliases)
                    .bind::<BigInt,_>(cursor.timestamp).bind::<Text,_>(&cursor.id).bind::<Text,_>(&cursor.sender)
                    .bind::<BigInt,_>((limit+1) as i64).load::<MessageRow>(conn).map_err(db_error)?);
            }
            Ok(rows)
        }).await?;
        let mut messages = rows
            .into_iter()
            .map(|row| decode(row).map(|m| identities.message(m)))
            .collect::<Result<Vec<_>>>()?;
        messages
            .sort_by(|a, b| (a.timestamp, &a.id, &a.sender).cmp(&(b.timestamp, &b.id, &b.sender)));
        let has_newer = messages.len() > limit;
        messages.truncate(limit);
        Ok(MessagePage {
            messages,
            has_newer,
            ..Default::default()
        })
    }

    pub async fn message_window(
        &self,
        chat: String,
        id: String,
        sender: String,
    ) -> Result<MessagePage> {
        self.allow_chat(&chat).await?;
        let account = self.account as i64;
        let identities = self.identities().await?;
        let aliases = identities.aliases_json(&chat);
        let authors = identities.aliases_json(&sender);
        let scope = identities.message_scope(&chat);
        let target: MessageRow =
            self.backend
                .shared()
                .read({
                    let aliases=aliases.clone();let authors=authors.clone();let scope=scope.clone();
                    move |conn| {
                        let exact:Option<MessageRow>=diesel::sql_query(format!(
                        "{MESSAGE_SELECT} WHERE m.account=? AND {scope} AND m.id=? AND m.sender IN (SELECT value FROM json_each(?))"
                    ))
                    .bind::<BigInt, _>(account)
                    .bind::<Text, _>(&aliases)
                    .bind::<Text, _>(&aliases)
                    .bind::<Text, _>(&id)
                    .bind::<Text, _>(authors)
                    .get_result(conn)
                    .optional().map_err(db_error)?;
                        if let Some(target) = exact {
                            return Ok(target);
                        }
                        // Quoted keys can carry a PN while the archive uses its LID,
                        // or our own JID while outgoing rows use "self". Resolve a
                        // unique ID within this chat, never an arbitrary match.
                        let mut candidates: Vec<MessageRow> = diesel::sql_query(format!(
                            "{MESSAGE_SELECT} WHERE m.account=? AND {scope} AND m.id=? LIMIT 2"
                        ))
                        .bind::<BigInt, _>(account)
                        .bind::<Text, _>(&aliases)
                        .bind::<Text, _>(&aliases)
                        .bind::<Text, _>(id)
                        .load(conn)
                        .map_err(db_error)?;
                        if candidates.len() == 1 {
                            Ok(candidates.remove(0))
                        } else {
                            Err(db_error(diesel::result::Error::NotFound))
                        }
                    }
                })
                .await
                .context("The original message is not available in the saved history")?;
        let pivot = identities.message(decode(target)?);
        let mut page = self
            .message_page(chat.clone(), 50, Some(pivot.cursor()))
            .await?;
        let sources = identities.aliases(&chat);
        let scope = identities.timeline_scope(&chat);
        let select = timeline_select();
        let rows:Vec<MessageRow>=self.backend.shared().read(move |conn| {
            let mut rows=Vec::new();
            for source in sources {
                rows.extend(diesel::sql_query(format!("{select} WHERE m.account=? AND {scope} AND json_extract(m.payload,'$.kind')!='protocol' AND (m.timestamp,m.id,m.sender)>=(?,?,?) ORDER BY m.timestamp,m.id,m.sender LIMIT 52"))
                    .bind::<BigInt,_>(account).bind::<Text,_>(source).bind::<Text,_>(&aliases).bind::<BigInt,_>(pivot.timestamp).bind::<Text,_>(&pivot.id).bind::<Text,_>(&pivot.sender).load::<MessageRow>(conn).map_err(db_error)?);
            } Ok(rows)
        }).await?;
        let mut after = rows
            .into_iter()
            .map(|row| decode(row).map(|m| identities.message(m)))
            .collect::<Result<Vec<_>>>()?;
        after.sort_by(|a, b| (a.timestamp, &a.id, &a.sender).cmp(&(b.timestamp, &b.id, &b.sender)));
        page.has_newer = after.len() > 51;
        after.truncate(51);
        page.messages.extend(after);
        Ok(page)
    }

    pub async fn rebuild_projections(&self) -> Result<()> {
        if self
            .preference("message_projection_v4".into())
            .await?
            .is_some()
        {
            return Ok(());
        }
        let mut cursor = 0;
        loop {
            let account = self.account as i64;
            let rows:Vec<RawRow>=self.backend.shared().read(move |conn| {
                diesel::sql_query("SELECT rowid AS cursor,payload,encoded,metadata FROM brazap_messages WHERE account=? AND rowid>? ORDER BY rowid LIMIT 256").bind::<BigInt,_>(account).bind::<BigInt,_>(cursor).load(conn).map_err(db_error)
            }).await?;
            if rows.is_empty() {
                break;
            }
            cursor = rows.last().unwrap().cursor;
            let mut updates = Vec::new();
            for row in rows {
                let old: Message = serde_json::from_str(&row.payload)?;
                let raw = wa::Message::decode_from_slice(&row.encoded)?;
                let mut message = project_message(
                    old.chat,
                    old.sender,
                    old.id,
                    old.from_me,
                    old.timestamp,
                    &raw,
                    String::new(),
                    String::new(),
                )
                .view;
                let metadata: serde_json::Value =
                    serde_json::from_str(&row.metadata).unwrap_or_default();
                let receipt = metadata.get("status").and_then(|v| v.as_str());
                if message.from_me {
                    message.status = match receipt {
                        Some("READ" | "PLAYED") => "read",
                        Some("DELIVERY_ACK") => "delivered",
                        _ => "sent",
                    }
                    .into();
                }
                updates.push((row.cursor, message_payload(&message)?, message));
            }
            self.backend.shared().run(move |conn| {
                conn.transaction::<_,diesel::result::Error,_>(|conn| {
                    for (rowid,payload,m) in updates {
                        crate::message_actions::store_reaction(conn, account, &m)?;
                        diesel::sql_query("UPDATE brazap_messages SET payload=? WHERE rowid=?").bind::<Text,_>(payload).bind::<BigInt,_>(rowid).execute(conn)?;
                        if m.from_me && crate::storage::status_rank(&m.status)>2 {
                            diesel::sql_query("UPDATE brazap_messages SET status=? WHERE rowid=? AND status!='read'").bind::<Text,_>(&m.status).bind::<BigInt,_>(rowid).execute(conn)?;
                        }
                        if m.visible(){diesel::sql_query("INSERT INTO brazap_message_search(account,chat,sender,id,text) VALUES(?,?,?,?,?) ON CONFLICT(account,chat,sender,id) DO UPDATE SET text=excluded.text").bind::<BigInt,_>(account).bind::<Text,_>(m.chat.clone()).bind::<Text,_>(m.sender.clone()).bind::<Text,_>(m.id.clone()).bind::<Text,_>(searchable(&m)).execute(conn)?;}
                        else {diesel::sql_query("DELETE FROM brazap_message_search WHERE account=? AND chat=? AND sender=? AND id=?").bind::<BigInt,_>(account).bind::<Text,_>(&m.chat).bind::<Text,_>(&m.sender).bind::<Text,_>(&m.id).execute(conn)?;}
                    }Ok(())
                }).map_err(db_error)
            }).await?;
        }
        self.set_preference("message_projection_v4".into(), "1".into())
            .await
    }

    pub async fn recover_archived_history(&self) -> Result<()> {
        if self
            .preference("history_recovery_v3".into())
            .await?
            .is_some()
        {
            return Ok(());
        }
        let mut cursor = 0;
        loop {
            let account = self.account as i64;
            let chunk:Option<Chunk>=self.backend.shared().read(move |conn|{
                diesel::sql_query("SELECT rowid AS cursor,data FROM brazap_history WHERE account=? AND stage=1 AND rowid>? ORDER BY rowid LIMIT 1").bind::<BigInt,_>(account).bind::<BigInt,_>(cursor).get_result(conn).optional().map_err(db_error)
            }).await?;
            let Some(chunk) = chunk else {
                break;
            };
            cursor = chunk.cursor;
            let history =
                tokio::task::spawn_blocking(move || decode_history(&chunk.data)).await??;
            crate::service::ingest_history(self, &history).await?;
        }
        self.set_preference("history_recovery_v3".into(), "1".into())
            .await
    }
}

pub(crate) fn decode_history(bytes: &[u8]) -> Result<wa::HistorySync> {
    use std::io::Read;
    let mut decoded = Vec::new();
    flate2::read::ZlibDecoder::new(bytes)
        .take(128 * 1024 * 1024 + 1)
        .read_to_end(&mut decoded)?;
    anyhow::ensure!(
        decoded.len() <= 128 * 1024 * 1024,
        "History chunk exceeds 128 MiB"
    );
    whatsapp_rust::waproto::codec::history_sync_decode(&decoded).context("Decode stored history")
}

impl crate::service::Service {
    pub async fn older_messages(&self, chat: String, cursor: MessageCursor) -> Result<MessagePage> {
        let chat = self.archive.canonical_chat(&chat).await?;
        let page = self
            .archive
            .message_page(chat.clone(), CHAT_PAGE_SIZE, Some(cursor.clone()))
            .await?;
        if !page.messages.is_empty() {
            return Ok(page);
        }
        if self.config.demo {
            return Ok(page);
        }
        let client = self.connected().await?;
        let result = self.fetch_older_history(&client, &chat, &cursor).await?;
        let mut page = self
            .archive
            .message_page(chat, CHAT_PAGE_SIZE, Some(cursor))
            .await?;
        if page.messages.is_empty() {
            page.notice=Some(match result {
                crate::history_prefetch::Fetched::Empty=>"The phone returned no older messages for this chat.",
                _=>"No older messages returned yet. Keep WhatsApp online on your phone and try again.",
            }.into());
        }
        Ok(page)
    }
}
