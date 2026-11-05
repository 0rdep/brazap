use std::sync::Arc;

use anyhow::{Context, Result};
use diesel::{
    prelude::*,
    sql_types::{BigInt, Binary, Text},
};
use whatsapp_rust::waproto::buffa::Message as ProtoMessage;
use whatsapp_rust::{
    prelude::{MessageExt, SqliteStore, wa},
    store::error::StoreError,
};

use crate::model::{Chat, Message, now};

const CHAT_SCHEMA: &str = include_str!("../migrations/004_chat_controls.sql");
const SCHEMA: &str = include_str!("../migrations/001_archive.sql");
const MESSAGE_SCHEMA: &str = include_str!("../migrations/003_messages.sql");
const VIEW_SCHEMA: &str = include_str!("../migrations/002_views.sql");
const ACTION_SCHEMA: &str = include_str!("../migrations/005_message_actions.sql");
const NOTIFICATION_SCHEMA: &str = include_str!("../migrations/006_notifications.sql");
pub(crate) const MESSAGE_SELECT: &str = include_str!("../queries/messages.sql");

#[derive(Clone)]
pub struct Archive {
    pub(crate) database: whatsapp_rust::store::SqliteDatabase,
    pub backend: Arc<SqliteStore>,
    pub account: i32,
}

#[derive(Clone)]
pub struct StoredMessage {
    pub view: Message,
    pub encoded: Vec<u8>,
    pub name: String,
    pub metadata: String,
}

#[derive(QueryableByName)]
pub(crate) struct MessageRow {
    #[diesel(sql_type = Text)]
    pub payload: String,
    #[diesel(sql_type = diesel::sql_types::Nullable<BigInt>)]
    pub deleted_at: Option<i64>,
    #[diesel(sql_type = BigInt)]
    pub retain_deleted: i64,
    #[diesel(sql_type = Text)]
    pub status: String,
    #[diesel(sql_type = BigInt)]
    pub has_media: i64,
    #[diesel(sql_type = Text)]
    pub sender_name: String,
    #[diesel(sql_type = Text)]
    pub sender_phone: String,
    #[diesel(sql_type = BigInt)]
    pub sender_photo_version: i64,
    #[diesel(sql_type = Text)]
    pub reactions: String,
}

#[derive(QueryableByName)]
struct CountRow {
    #[diesel(sql_type=BigInt)]
    count: i64,
}

#[derive(QueryableByName)]
pub struct BytesRow {
    #[diesel(sql_type = Binary)]
    pub data: Vec<u8>,
}

#[derive(QueryableByName)]
pub struct RecoveryRow {
    #[diesel(sql_type = Text)]
    pub chat: String,
    #[diesel(sql_type = Text)]
    pub sender: String,
    #[diesel(sql_type = Text)]
    pub id: String,
    #[diesel(sql_type = Binary)]
    pub data: Vec<u8>,
}

pub(crate) fn db_error(error: diesel::result::Error) -> StoreError {
    StoreError::Database(Box::new(error))
}

impl Archive {
    pub async fn open(path: &std::path::Path) -> Result<Self> {
        let database = whatsapp_rust::store::SqliteDatabase::open(
            path.to_str().context("Database path is not UTF-8")?,
            whatsapp_rust::store::SqliteDatabaseConfig {
                synchronous: whatsapp_rust::store::Synchronous::Full,
                ..Default::default()
            },
        )
        .await?;
        let backend = Arc::new(database.store(1));
        use diesel::connection::SimpleConnection;
        backend
            .shared()
            .run(|conn| {
                conn.batch_execute(SCHEMA).map_err(db_error)?;
                conn.batch_execute(VIEW_SCHEMA).map_err(db_error)?;
                conn.batch_execute(MESSAGE_SCHEMA).map_err(db_error)?;
                conn.batch_execute(CHAT_SCHEMA).map_err(db_error)?;
                conn.batch_execute(ACTION_SCHEMA).map_err(db_error)?;
                conn.batch_execute(NOTIFICATION_SCHEMA).map_err(db_error)?;
                conn.batch_execute(include_str!("../migrations/007_deletions.sql"))
                    .map_err(db_error)?;
                conn.batch_execute(include_str!("../migrations/008_history_requests.sql"))
                    .map_err(db_error)?;
                conn.batch_execute(include_str!("../migrations/009_message_identity.sql"))
                    .map_err(db_error)?;
                conn.batch_execute(include_str!("../migrations/010_saved_contacts.sql"))
                    .map_err(db_error)?;
                conn.batch_execute(include_str!("../migrations/011_stickers.sql"))
                    .map_err(db_error)?;
                conn.batch_execute(include_str!("../migrations/012_identity_links.sql"))
                    .map_err(db_error)?;
                conn.batch_execute(include_str!("../migrations/013_chat_sync_policy.sql"))
                    .map_err(db_error)?;
                conn.batch_execute(include_str!("../migrations/014_history_boundaries.sql"))
                    .map_err(db_error)?;
                conn.batch_execute(include_str!("../migrations/015_live_calls.sql"))
                    .map_err(db_error)?;
                conn.batch_execute(include_str!("../migrations/016_read_state.sql"))
                    .map_err(db_error)
            })
            .await?;
        Ok(Self {
            database,
            backend,
            account: 1,
        })
    }

    pub async fn store_messages(&self, rows: Vec<StoredMessage>, live: bool) -> Result<()> {
        let account = self.account;
        let identities = self.identities().await?;
        let rows: Vec<_> = rows
            .into_iter()
            .filter(|row| !identities.is_locked(&row.view.chat))
            .map(|row| {
                let aliases = identities.aliases_json(&row.view.chat);
                let senders = identities.aliases_json(&row.view.sender);
                (row, aliases, senders)
            })
            .collect();
        self.backend.shared().run(move |conn| {
            conn.transaction::<_, diesel::result::Error, _>(|conn| {
                for (row,aliases,senders) in rows {
                    let m = row.view;
                    crate::stickers::index(conn, account as i64, &m, &row.encoded)?;
                    store_deletion(conn, account as i64, &m.chat, m.timestamp, &row.encoded)?;
                    let payload = message_payload(&m).map_err(|error| diesel::result::Error::SerializationError(Box::new(error)))?;
                    let existed:CountRow=diesel::sql_query("SELECT COUNT(*) AS count FROM brazap_messages WHERE account=? AND chat IN (SELECT value FROM json_each(?)) AND (? NOT LIKE '%@g.us' OR sender IN (SELECT value FROM json_each(?))) AND id=? AND json_extract(payload,'$.from_me')=?")
                        .bind::<BigInt,_>(account as i64).bind::<Text,_>(&aliases).bind::<Text,_>(&m.chat).bind::<Text,_>(&senders).bind::<Text,_>(&m.id).bind::<BigInt,_>(i64::from(m.from_me)).get_result(conn)?;
                    diesel::sql_query(
                        "INSERT INTO brazap_messages(account, chat, sender, id, timestamp, status, payload, encoded, metadata) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(account,chat,sender,id) DO UPDATE SET status=CASE WHEN (CASE excluded.status WHEN 'read' THEN 4 WHEN 'delivered' THEN 3 WHEN 'sent' THEN 2 ELSE 0 END)>(CASE brazap_messages.status WHEN 'read' THEN 4 WHEN 'delivered' THEN 3 WHEN 'sent' THEN 2 ELSE 0 END) THEN excluded.status ELSE brazap_messages.status END"
                    ).bind::<BigInt,_>(account as i64).bind::<Text,_>(&m.chat)
                        .bind::<Text,_>(&m.sender).bind::<Text,_>(&m.id).bind::<BigInt,_>(m.timestamp)
                        .bind::<Text,_>(&m.status).bind::<Text,_>(payload).bind::<Binary,_>(row.encoded)
                        .bind::<Text,_>(row.metadata).execute(conn)?;
                    crate::message_actions::store_reaction(conn, account as i64, &m)?;
                    if !m.visible() {continue;}
                    diesel::sql_query("INSERT INTO brazap_message_search(account,chat,sender,id,text) VALUES(?,?,?,?,?) ON CONFLICT(account,chat,sender,id) DO UPDATE SET text=excluded.text")
                        .bind::<BigInt,_>(account as i64).bind::<Text,_>(&m.chat).bind::<Text,_>(&m.sender).bind::<Text,_>(&m.id).bind::<Text,_>(searchable(&m)).execute(conn)?;
                    let read:CountRow=diesel::sql_query("SELECT (EXISTS(SELECT 1 FROM brazap_chat_reads WHERE account=? AND chat IN (SELECT value FROM json_each(?)) AND through_timestamp>?) OR EXISTS(SELECT 1 FROM brazap_read_messages WHERE account=? AND chat IN (SELECT value FROM json_each(?)) AND id=? AND (sender='' OR sender IN (SELECT value FROM json_each(?))))) AS count")
                        .bind::<BigInt,_>(account as i64).bind::<Text,_>(&aliases).bind::<BigInt,_>(m.timestamp).bind::<BigInt,_>(account as i64).bind::<Text,_>(&aliases).bind::<Text,_>(&m.id).bind::<Text,_>(&senders).get_result(conn)?;
                    let unread = i64::from(existed.count == 0 && live && !m.from_me && read.count==0);
                    let preview = m.preview();
                    diesel::sql_query(
                        "INSERT INTO brazap_chats(account,jid,name,preview,updated_at,unread) VALUES(?,?,?,?,?,?) ON CONFLICT(account,jid) DO UPDATE SET name=CASE WHEN excluded.name!='' THEN excluded.name ELSE brazap_chats.name END, preview=CASE WHEN excluded.updated_at>=brazap_chats.updated_at THEN excluded.preview ELSE brazap_chats.preview END, updated_at=MAX(brazap_chats.updated_at,excluded.updated_at), unread=brazap_chats.unread+excluded.unread"
                    ).bind::<BigInt,_>(account as i64).bind::<Text,_>(&m.chat).bind::<Text,_>(row.name)
                        .bind::<Text,_>(preview).bind::<BigInt,_>(m.timestamp).bind::<BigInt,_>(unread).execute(conn)?;
                }
                Ok(())
            }).map_err(db_error)
        }).await?;
        Ok(())
    }

    pub async fn chats(&self) -> Result<Vec<Chat>> {
        let account = self.account as i64;
        let chats: Vec<Chat> = self
            .backend
            .shared()
            .read(move |conn| {
                diesel::sql_query(include_str!("../queries/chats.sql"))
                    .bind::<BigInt, _>(account)
                    .load(conn)
                    .map_err(db_error)
            })
            .await?;
        Ok(self
            .identities()
            .await?
            .chats(chats, self.alias_settings().await?))
    }

    pub async fn messages(
        &self,
        chat: String,
        limit: usize,
        before: Option<i64>,
    ) -> Result<Vec<Message>> {
        let cursor = before.map(|timestamp| crate::model::MessageCursor {
            timestamp,
            id: String::new(),
            sender: String::new(),
        });
        Ok(self.message_page(chat, limit, cursor).await?.messages)
    }

    pub async fn name_chat(
        &self,
        jid: String,
        name: String,
        unread: i64,
        timestamp: i64,
    ) -> Result<()> {
        let account = self.account as i64;
        self.backend.shared().run(move |conn| {
            diesel::sql_query("INSERT INTO brazap_chats(account,jid,name,preview,updated_at,unread) VALUES(?,?,?,'',?,?) ON CONFLICT(account,jid) DO UPDATE SET name=CASE WHEN excluded.name!='' THEN excluded.name ELSE brazap_chats.name END,updated_at=MAX(brazap_chats.updated_at,excluded.updated_at)")
                .bind::<BigInt,_>(account).bind::<Text,_>(jid).bind::<Text,_>(name)
                .bind::<BigInt,_>(timestamp).bind::<BigInt,_>(unread).execute(conn).map_err(db_error)?;
            Ok(())
        }).await?;
        Ok(())
    }

    pub async fn contact(
        &self,
        jid: String,
        name: String,
        payload: String,
        saved: bool,
    ) -> Result<()> {
        let account = self.account as i64;
        self.backend.shared().run(move |conn| {
            if saved {
                diesel::sql_query("INSERT INTO brazap_saved_contacts(account,jid,name,payload) VALUES(?,?,?,?) ON CONFLICT(account,jid) DO UPDATE SET name=excluded.name,payload=excluded.payload")
                    .bind::<BigInt,_>(account).bind::<Text,_>(&jid).bind::<Text,_>(&name)
                    .bind::<Text,_>(&payload).execute(conn).map_err(db_error)?;
            }
            diesel::sql_query("INSERT INTO brazap_contacts(account,jid,name,payload) VALUES(?,?,?,?) ON CONFLICT(account,jid) DO UPDATE SET name=excluded.name,payload=excluded.payload")
                .bind::<BigInt,_>(account).bind::<Text,_>(jid).bind::<Text,_>(name)
                .bind::<Text,_>(payload).execute(conn).map_err(db_error)?;
            Ok(())
        }).await?;
        Ok(())
    }

    pub async fn event(&self, kind: String, payload: String) -> Result<()> {
        let account = self.account as i64;
        self.backend
            .shared()
            .run(move |conn| {
                diesel::sql_query(
                    "INSERT INTO brazap_events(account,kind,identity,payload) VALUES(?,?,?,?)",
                )
                .bind::<BigInt, _>(account)
                .bind::<Text, _>(kind)
                .bind::<Text, _>(uuid::Uuid::new_v4().to_string())
                .bind::<Text, _>(payload)
                .execute(conn)
                .map_err(db_error)?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    pub async fn mark_read(&self, chat: String) -> Result<()> {
        self.apply_chat_read(
            &chat,
            None,
            Vec::new(),
            chrono::Utc::now().timestamp_millis(),
            false,
        )
        .await
        .map(|_| ())
    }

    pub async fn receipt(&self, chat: String, ids: Vec<String>, status: String) -> Result<()> {
        let account = self.account as i64;
        let aliases = self.identities().await?.aliases_json(&chat);
        self.backend.shared().run(move |conn| {
            conn.transaction::<_,diesel::result::Error,_>(|conn| {
                for id in ids {
                    diesel::sql_query("INSERT INTO brazap_receipts(account,chat,id,status) VALUES(?,?,?,?) ON CONFLICT(account,chat,id) DO UPDATE SET status=CASE WHEN (CASE excluded.status WHEN 'read' THEN 4 WHEN 'delivered' THEN 3 WHEN 'sent' THEN 2 WHEN 'failed' THEN 1 ELSE 0 END) >= (CASE brazap_receipts.status WHEN 'read' THEN 4 WHEN 'delivered' THEN 3 WHEN 'sent' THEN 2 WHEN 'failed' THEN 1 ELSE 0 END) THEN excluded.status ELSE brazap_receipts.status END")
                        .bind::<BigInt,_>(account).bind::<Text,_>(&chat).bind::<Text,_>(&id).bind::<Text,_>(&status).execute(conn)?;
                    diesel::sql_query("UPDATE brazap_messages SET status=? WHERE account=? AND chat IN (SELECT value FROM json_each(?)) AND id=? AND json_extract(payload,'$.from_me')=1 AND (CASE status WHEN 'read' THEN 4 WHEN 'delivered' THEN 3 WHEN 'sent' THEN 2 ELSE 0 END) <= ?")
                        .bind::<Text,_>(&status).bind::<BigInt,_>(account).bind::<Text,_>(&aliases).bind::<Text,_>(&id).bind::<BigInt,_>(status_rank(&status)).execute(conn)?;
                }
                Ok(())
            }).map_err(db_error)
        }).await?;
        Ok(())
    }

    pub async fn put_media(&self, message: &Message, bytes: Vec<u8>) -> Result<()> {
        let source = self
            .message_source(&message.chat, &message.sender, &message.id)
            .await?;
        let aliases = self.identities().await?.aliases_json(&message.chat);
        let (account, chat, sender, id) = (
            self.account as i64,
            source.chat,
            source.sender,
            message.id.clone(),
        );
        self.backend.shared().run(move |conn| {
            diesel::sql_query("INSERT INTO brazap_media(account,chat,sender,id,data) VALUES(?,?,?,?,?) ON CONFLICT(account,chat,sender,id) DO UPDATE SET data=excluded.data")
                .bind::<BigInt,_>(account).bind::<Text,_>(&chat).bind::<Text,_>(&sender)
                .bind::<Text,_>(&id).bind::<Binary,_>(bytes).execute(conn).map_err(db_error)?;
            diesel::sql_query("UPDATE brazap_assets SET expires_at=0 WHERE account=? AND key=?")
                .bind::<BigInt,_>(account).bind::<Text,_>(format!("thumb:{chat}:{sender}:{id}")).execute(conn).map_err(db_error)?;
            for alias in serde_json::from_str::<Vec<String>>(&aliases).unwrap_or_default() {
                diesel::sql_query("UPDATE brazap_assets SET expires_at=0 WHERE account=? AND key=?")
                    .bind::<BigInt,_>(account).bind::<Text,_>(format!("thumb:{alias}:{sender}:{id}")).execute(conn).map_err(db_error)?;
            }
            Ok(())
        }).await?;
        Ok(())
    }

    pub async fn media(&self, chat: String, sender: String, id: String) -> Result<Option<Vec<u8>>> {
        let account = self.account as i64;
        let identities = self.identities().await?;
        let chat = identities.aliases_json(&chat);
        let sender = identities.aliases_json(&sender);
        let row: Option<BytesRow> = self.backend.shared().read(move |conn| {
            diesel::sql_query("SELECT data FROM brazap_media WHERE account=? AND chat IN (SELECT value FROM json_each(?)) AND sender IN (SELECT value FROM json_each(?)) AND id=? LIMIT 1")
                .bind::<BigInt,_>(account).bind::<Text,_>(chat).bind::<Text,_>(sender)
                .bind::<Text,_>(id).get_result(conn).optional().map_err(db_error)
        }).await?;
        Ok(row.map(|r| r.data))
    }

    pub async fn raw_message(
        &self,
        chat: String,
        sender: String,
        id: String,
    ) -> Result<wa::Message> {
        self.allow_chat(&chat).await?;
        let source = self.message_source(&chat, &sender, &id).await?;
        let chat = source.chat;
        let sender = source.sender;
        let account = self.account as i64;
        let row: BytesRow = self.backend.shared().read(move |conn| {
            diesel::sql_query("SELECT encoded AS data FROM brazap_messages WHERE account=? AND chat=? AND sender=? AND id=?")
                .bind::<BigInt,_>(account).bind::<Text,_>(chat).bind::<Text,_>(sender)
                .bind::<Text,_>(id).get_result(conn).map_err(db_error)
        }).await?;
        Ok(wa::Message::decode_from_slice(&row.data)?)
    }

    pub async fn capture(
        &self,
        key: &whatsapp_rust::HistorySyncKey,
        stage: i64,
        data: Vec<u8>,
        metadata: String,
    ) -> Result<()> {
        let (account, chat, sender, id) = (
            self.account as i64,
            key.chat.clone(),
            key.sender.clone(),
            key.id.clone(),
        );
        self.backend.shared().run(move |conn| {
            diesel::sql_query("INSERT OR IGNORE INTO brazap_history(account,chat,sender,id,stage,data,metadata,processed) VALUES(?,?,?,?,?,?,?,0)")
                .bind::<BigInt,_>(account).bind::<Text,_>(&chat).bind::<Text,_>(&sender).bind::<Text,_>(&id)
                .bind::<BigInt,_>(stage).bind::<Binary,_>(&data).bind::<Text,_>(metadata).execute(conn).map_err(db_error)?;
            let existing: BytesRow = diesel::sql_query("SELECT data FROM brazap_history WHERE account=? AND chat=? AND sender=? AND id=? AND stage=?")
                .bind::<BigInt,_>(account).bind::<Text,_>(chat).bind::<Text,_>(sender).bind::<Text,_>(id)
                .bind::<BigInt,_>(stage).get_result(conn).map_err(db_error)?;
            if existing.data != data { return Err(StoreError::Validation("History replay changed stored bytes".into())); }
            Ok(())
        }).await?;
        Ok(())
    }

    pub async fn pending_history(&self) -> Result<Vec<RecoveryRow>> {
        let account = self.account as i64;
        Ok(self.backend.shared().read(move |conn| {
            diesel::sql_query("SELECT chat,sender,id,data FROM brazap_history WHERE account=? AND stage=0 AND processed=0 ORDER BY rowid LIMIT 32")
                .bind::<BigInt,_>(account).load(conn).map_err(db_error)
        }).await?)
    }

    pub async fn finish_history(&self, key: &whatsapp_rust::HistorySyncKey) -> Result<()> {
        let (account, chat, sender, id) = (
            self.account as i64,
            key.chat.clone(),
            key.sender.clone(),
            key.id.clone(),
        );
        self.backend.shared().run(move |conn| {
            diesel::sql_query("UPDATE brazap_history SET processed=1 WHERE account=? AND chat=? AND sender=? AND id=?")
                .bind::<BigInt,_>(account).bind::<Text,_>(chat).bind::<Text,_>(sender).bind::<Text,_>(id).execute(conn).map_err(db_error)?;
            Ok(())
        }).await?;
        Ok(())
    }
}

pub(crate) fn meaningful_name(jid: &str, name: &str) -> bool {
    use whatsapp_rust::wacore_binary::JidExt;
    let name = name.trim();
    if name.is_empty()
        || matches!(
            name.to_lowercase().as_str(),
            "unknown contact"
                | "unknown"
                | "unnamed group"
                | "contato desconhecido"
                | "contacto desconocido"
        )
    {
        return false;
    }
    let bare = jid
        .parse::<whatsapp_rust::Jid>()
        .ok()
        .is_some_and(|jid| name == jid.user());
    name != jid && !bare && name.parse::<whatsapp_rust::Jid>().is_err()
}

pub fn display_identity(jid: &str, name: &str, mapped_phone: &str) -> (String, String) {
    use whatsapp_rust::wacore_binary::JidExt;
    let parsed = jid.parse::<whatsapp_rust::Jid>().ok();
    let phone = if !mapped_phone.is_empty() {
        mapped_phone
            .split('@')
            .next()
            .unwrap_or("")
            .trim_start_matches('+')
            .to_owned()
    } else {
        parsed
            .as_ref()
            .filter(|jid| jid.is_pn())
            .map(|jid| jid.user().to_owned())
            .unwrap_or_default()
    };
    let phone = if !phone.is_empty() && phone.bytes().all(|c| c.is_ascii_digit()) {
        format!("+{phone}")
    } else {
        String::new()
    };
    let name = name.trim();
    let meaningful = meaningful_name(jid, name);
    let display = if meaningful {
        name.to_owned()
    } else if jid == "status@broadcast" {
        "WhatsApp".into()
    } else if !phone.is_empty() {
        phone.clone()
    } else if jid.ends_with("@g.us") {
        "Unnamed group".into()
    } else {
        "Unknown contact".into()
    };
    (display, phone)
}

pub fn project_message(
    chat: String,
    sender: String,
    id: String,
    from_me: bool,
    timestamp: i64,
    message: &wa::Message,
    name: String,
    metadata: String,
) -> StoredMessage {
    let mut view = Message {
        id,
        chat,
        sender: if from_me { "self".into() } else { sender },
        from_me,
        timestamp,
        text: message
            .text_content()
            .or_else(|| message.get_caption())
            .unwrap_or("")
            .to_owned(),
        kind: "text".into(),
        status: if from_me {
            "sent".into()
        } else {
            "received".into()
        },
        ..Default::default()
    };
    crate::content::project(&mut view, message);
    if !view.is_call() {
        crate::content::project_call_stub(&mut view, &metadata);
    }
    StoredMessage {
        view,
        encoded: message.encode_to_vec(),
        name,
        metadata,
    }
}

pub fn outgoing(chat: String, id: String, message: &wa::Message) -> StoredMessage {
    project_message(
        chat,
        "self".into(),
        id,
        true,
        now(),
        message,
        String::new(),
        "{}".into(),
    )
}

pub(crate) fn message_payload(message: &Message) -> serde_json::Result<String> {
    let mut value = serde_json::to_value(message)?;
    value["preview"] = serde_json::Value::String(message.preview());
    Ok(serde_json::to_string(&value)?)
}
pub(crate) fn searchable(message: &Message) -> String {
    format!(
        "{} {} {} {} {}",
        message.title,
        message.text,
        message.footer,
        message.file_name.as_deref().unwrap_or(""),
        message
            .actions
            .iter()
            .map(|a| a.label.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    )
    .to_lowercase()
}
pub(crate) fn status_rank(status: &str) -> i64 {
    match status {
        "read" => 4,
        "delivered" => 3,
        "sent" => 2,
        "failed" => 1,
        _ => 0,
    }
}

fn store_deletion(
    conn: &mut diesel::SqliteConnection,
    account: i64,
    chat: &str,
    time: i64,
    bytes: &[u8],
) -> diesel::QueryResult<()> {
    let Ok(proto) = wa::Message::decode(&mut &*bytes) else {
        return Ok(());
    };
    let inner = crate::content::media_message(&proto);
    let Some(protocol) = inner.protocol_message.as_option() else {
        return Ok(());
    };
    if protocol.r#type != Some(wa::message::protocol_message::Type::REVOKE) {
        return Ok(());
    }
    if let Some(key) = protocol.key.as_option() {
        if let Some(id) = key.id.as_deref().filter(|s| !s.is_empty()) {
            diesel::sql_query("INSERT OR IGNORE INTO brazap_deletions(account,chat,id,deleted_at) VALUES(?,?,?,?)")
                .bind::<BigInt,_>(account).bind::<Text,_>(chat).bind::<Text,_>(id).bind::<BigInt,_>(time).execute(conn)?;
        }
    }
    Ok(())
}
impl Archive {
    pub async fn backfill_deletions(&self) -> Result<()> {
        if self.preference("deletions_v1".into()).await?.is_some() {
            return Ok(());
        }
        #[derive(QueryableByName)]
        struct Row {
            #[diesel(sql_type=Text)]
            chat: String,
            #[diesel(sql_type=BigInt)]
            timestamp: i64,
            #[diesel(sql_type=Binary)]
            encoded: Vec<u8>,
        }
        let account = self.account as i64;
        self.backend.shared().run(move|conn| {
            let rows=diesel::sql_query("SELECT chat,timestamp,encoded FROM brazap_messages WHERE account=? AND json_extract(payload,'$.kind')='protocol'").bind::<BigInt,_>(account).load::<Row>(conn).map_err(db_error)?;
            conn.transaction::<_,diesel::result::Error,_>(|conn| {for row in rows {store_deletion(conn,account,&row.chat,row.timestamp,&row.encoded)?;} Ok(())}).map_err(db_error)
        }).await?;
        self.set_preference("deletions_v1".into(), "1".into()).await
    }
}
