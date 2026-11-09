//! A phone JID and its current WhatsApp LID address the same conversation.
//! Keep original archive keys intact; combine aliases only in application views.
use crate::{
    model::{Chat, Message},
    storage::{Archive, db_error, display_identity, meaningful_name},
};
use anyhow::Result;
use diesel::{
    prelude::*,
    sql_types::{BigInt, Text},
};
use std::collections::{HashMap, HashSet};

#[derive(Default)]
pub(crate) struct Identities {
    canonical: HashMap<String, String>,
    peers: HashMap<String, Vec<String>>,
    saved_names: HashMap<String, String>,
    known_names: HashMap<String, String>,
    locked: HashSet<String>,
}
#[derive(QueryableByName)]
struct Mapping {
    #[diesel(sql_type=Text)]
    lid: String,
    #[diesel(sql_type=Text)]
    phone_number: String,
}
#[derive(QueryableByName)]
struct Lock {
    #[diesel(sql_type=Text)]
    jid: String,
    #[diesel(sql_type=BigInt)]
    locked: i64,
}
#[derive(QueryableByName)]
struct SavedContact {
    #[diesel(sql_type=Text)]
    jid: String,
    #[diesel(sql_type=Text)]
    name: String,
}
#[derive(QueryableByName)]
struct SourceChat {
    #[diesel(sql_type=Text)]
    jid: String,
}
#[derive(QueryableByName)]
pub(crate) struct ChatSettings {
    #[diesel(sql_type=Text)]
    jid: String,
    #[diesel(sql_type=BigInt)]
    pinned: i64,
    #[diesel(sql_type=BigInt)]
    archived: i64,
    #[diesel(sql_type=BigInt)]
    muted: i64,
    #[diesel(sql_type=BigInt)]
    mute_until_ms: i64,
    #[diesel(sql_type=BigInt)]
    pin_changed_at: i64,
    #[diesel(sql_type=BigInt)]
    archive_changed_at: i64,
    #[diesel(sql_type=BigInt)]
    mute_changed_at: i64,
}
#[derive(QueryableByName)]
pub(crate) struct MessageSource {
    #[diesel(sql_type=Text)]
    pub chat: String,
    #[diesel(sql_type=Text)]
    pub sender: String,
}

fn normalize(jid: &str) -> String {
    if jid == "self" {
        return jid.into();
    }
    jid.parse::<whatsapp_rust::Jid>()
        .map(|j| j.to_non_ad().to_string())
        .unwrap_or_else(|_| jid.into())
}
fn address(value: &str, server: &str) -> Option<String> {
    let jid = if value.contains('@') {
        normalize(value)
    } else {
        format!("{value}@{server}")
    };
    let (user, host) = jid.split_once('@')?;
    (!user.is_empty() && host == server && user.bytes().all(|b| b.is_ascii_digit())).then_some(jid)
}

impl Identities {
    pub(crate) fn is_locked(&self, jid: &str) -> bool {
        self.locked.contains(&self.canonical(jid))
    }
    pub(crate) fn canonical(&self, jid: &str) -> String {
        let jid = normalize(jid);
        self.canonical.get(&jid).cloned().unwrap_or(jid)
    }
    pub(crate) fn aliases(&self, jid: &str) -> Vec<String> {
        let canonical = self.canonical(jid);
        let mut aliases = self
            .peers
            .get(&canonical)
            .cloned()
            .unwrap_or_else(|| vec![canonical]);
        if !aliases.iter().any(|a| a == jid) {
            aliases.push(jid.into());
        }
        aliases
    }
    pub(crate) fn aliases_json(&self, jid: &str) -> String {
        serde_json::to_string(&self.aliases(jid)).expect("String aliases serialize")
    }
    pub(crate) fn person(&self, jid: &str, fallback: &str) -> (String, String) {
        let jid = self.canonical(jid);
        let name = self
            .saved_names
            .get(&jid)
            .or_else(|| self.known_names.get(&jid))
            .map(String::as_str)
            .unwrap_or(fallback);
        let phone = self
            .aliases(&jid)
            .into_iter()
            .find(|value| value.ends_with("@s.whatsapp.net"))
            .unwrap_or_default();
        display_identity(&jid, name, &phone)
    }

    pub(crate) fn message(&self, mut message: Message) -> Message {
        message.chat = self.canonical(&message.chat);
        if !message.from_me {
            let sender = self.canonical(&message.sender);
            if let Some(name) = self.saved_names.get(&sender) {
                message.sender_name = name.clone();
            } else if !meaningful_name(&sender, &message.sender_name)
                || (!message.sender_phone.is_empty() && message.sender_name == message.sender_phone)
            {
                if let Some(name) = self.known_names.get(&sender) {
                    message.sender_name = name.clone();
                }
            }
            if message.sender_phone.is_empty() {
                if let Some(phone) = self
                    .aliases(&sender)
                    .into_iter()
                    .find(|jid| jid.ends_with("@s.whatsapp.net"))
                {
                    message.sender_phone = display_identity(&sender, "", &phone).1;
                }
            }
            (message.sender_name, message.sender_phone) =
                display_identity(&sender, &message.sender_name, &message.sender_phone);
        }
        message
    }

    /// Two bound alias arrays select one stored representative per logical
    /// message before LIMIT, so pagination cannot fill up with duplicates.
    pub(crate) fn message_scope(&self, jid: &str) -> String {
        let author = if jid.ends_with("@g.us") {
            "d.sender=m.sender"
        } else {
            "COALESCE(json_extract(d.payload,'$.from_me'),0)=COALESCE(json_extract(m.payload,'$.from_me'),0)"
        };
        format!(
            "m.chat IN (SELECT value FROM json_each(?)) AND m.rowid=(SELECT d.rowid FROM brazap_messages d WHERE d.account=m.account AND d.chat IN (SELECT value FROM json_each(?)) AND d.id=m.id AND {author} ORDER BY CASE json_extract(d.payload,'$.kind') WHEN 'protocol' THEN -1 WHEN 'placeholder' THEN 0 WHEN 'unsupported' THEN 0 ELSE 1 END DESC, CASE d.status WHEN 'read' THEN 4 WHEN 'delivered' THEN 3 WHEN 'sent' THEN 2 ELSE 0 END DESC,d.rowid LIMIT 1)"
        )
    }

    /// Query each address independently so SQLite can walk its timeline index
    /// and stop at LIMIT, instead of sorting an entire combined conversation.
    pub(crate) fn timeline_scope(&self, jid: &str) -> String {
        if self.aliases(jid).len() == 1 {
            return "m.chat=? AND ? IS NOT NULL".into();
        }
        self.message_scope(jid).replacen(
            "m.chat IN (SELECT value FROM json_each(?))",
            "m.chat=?",
            1,
        )
    }

    pub(crate) fn chats(&self, chats: Vec<Chat>, settings: Vec<ChatSettings>) -> Vec<Chat> {
        let mut groups = HashMap::<String, Vec<Chat>>::new();
        let mut seen = HashSet::new();
        for chat in chats {
            if self.is_locked(&chat.jid) {
                continue;
            }
            if seen.insert(chat.jid.clone()) {
                groups
                    .entry(self.canonical(&chat.jid))
                    .or_default()
                    .push(chat);
            }
        }
        let mut controls = HashMap::<String, Vec<ChatSettings>>::new();
        for setting in settings {
            controls
                .entry(self.canonical(&setting.jid))
                .or_default()
                .push(setting);
        }
        let mut result = Vec::with_capacity(groups.len());
        for (canonical, rows) in groups {
            let mut chat = rows
                .iter()
                .find(|c| c.jid == canonical)
                .unwrap_or(&rows[0])
                .clone();
            if !meaningful_name(&canonical, &chat.name) {
                chat.name = rows
                    .iter()
                    .find(|c| meaningful_name(&c.jid, &c.name))
                    .map(|c| c.name.clone())
                    .unwrap_or_default();
            }
            if let Some(latest) = rows.iter().max_by_key(|c| c.updated_at) {
                chat.updated_at = latest.updated_at;
                chat.preview = latest.preview.clone();
                chat.preview_kind = latest.preview_kind.clone();
                chat.preview_status = latest.preview_status.clone();
                chat.preview_from_me = latest.preview_from_me;
            }
            chat.unread = rows.iter().map(|c| c.unread).sum();
            chat.photo_version = rows.iter().map(|c| c.photo_version).max().unwrap_or(0);
            chat.jid = canonical.clone();
            chat.aliases = crate::model::ChatAliases(self.aliases(&canonical));
            let peer_settings = controls.remove(&canonical).unwrap_or_default();
            if let Some(s) = peer_settings
                .iter()
                .max_by_key(|s| (s.pin_changed_at, s.jid == canonical))
            {
                chat.pinned = s.pinned != 0;
            }
            if let Some(s) = peer_settings
                .iter()
                .max_by_key(|s| (s.archive_changed_at, s.jid == canonical))
            {
                chat.archived = s.archived != 0;
            }
            if let Some(s) = peer_settings
                .iter()
                .max_by_key(|s| (s.mute_changed_at, s.jid == canonical))
            {
                chat.muted = s.muted != 0
                    && (s.mute_until_ms == -1 || s.mute_until_ms > crate::model::now() * 1000);
            }
            let phone = chat
                .aliases
                .iter()
                .find(|a| a.ends_with("@s.whatsapp.net"))
                .cloned()
                .unwrap_or(chat.phone);
            if let Some(name) = self
                .saved_names
                .get(&canonical)
                .filter(|_| !canonical.ends_with("@g.us"))
            {
                chat.name = name.clone();
            }
            if !meaningful_name(&canonical, &chat.name) {
                if let Some(name) = self.known_names.get(&canonical) {
                    chat.name = name.clone();
                }
            }
            (chat.name, chat.phone) = display_identity(&canonical, &chat.name, &phone);
            result.push(chat);
        }
        result.sort_by(|a, b| {
            b.pinned
                .cmp(&a.pinned)
                .then_with(|| b.updated_at.cmp(&a.updated_at))
                .then_with(|| a.jid.cmp(&b.jid))
        });
        result
    }
}

impl Archive {
    pub(crate) async fn identities(&self) -> Result<Identities> {
        let account = self.account as i64;
        let (rows,sources,saved,locks,names)=self.backend.shared().read(move|conn| {
            let rows=diesel::sql_query("SELECT lid,phone_number FROM (SELECT lid,phone_number,updated_at FROM lid_pn_mapping WHERE device_id=? UNION ALL SELECT lid,phone_number,updated_at FROM brazap_identity_links WHERE account=?) ORDER BY updated_at DESC,lid")
                .bind::<BigInt,_>(account).bind::<BigInt,_>(account).load::<Mapping>(conn).map_err(db_error)?;
            let sources=diesel::sql_query("SELECT jid FROM brazap_chats WHERE account=?")
                .bind::<BigInt,_>(account).load::<SourceChat>(conn).map_err(db_error)?;
            let saved=diesel::sql_query("SELECT jid,name FROM brazap_saved_contacts WHERE account=? AND trim(name)!='' ORDER BY rowid")
                .bind::<BigInt,_>(account).load::<SavedContact>(conn).map_err(db_error)?;
            let locks=diesel::sql_query("SELECT jid,locked FROM brazap_chat_locks WHERE account=? ORDER BY changed_at,locked")
                .bind::<BigInt,_>(account).load::<Lock>(conn).map_err(db_error)?;
            let names=diesel::sql_query("SELECT jid,name FROM brazap_chats WHERE account=? UNION ALL SELECT jid,name FROM brazap_contacts WHERE account=?")
                .bind::<BigInt,_>(account).bind::<BigInt,_>(account).load::<SavedContact>(conn).map_err(db_error)?;
            Ok((rows,sources,saved,locks,names))
        }).await?;
        let mut identities = Identities::default();
        let mut used = HashSet::new();
        // Match the SDK's newest reverse PN mapping. Never merge by name or
        // attach an older competing LID just because it once used this number.
        for row in rows {
            let (Some(lid), Some(pn)) = (
                address(&row.lid, "lid"),
                address(&row.phone_number, "s.whatsapp.net"),
            ) else {
                continue;
            };
            if used.contains(&pn) || identities.canonical.contains_key(&lid) {
                continue;
            }
            used.insert(pn.clone());
            identities.canonical.insert(pn.clone(), lid.clone());
            identities.canonical.insert(lid.clone(), lid.clone());
            identities.peers.insert(lid.clone(), vec![lid, pn]);
        }
        for source in sources {
            let canonical = identities.canonical(&source.jid);
            if canonical != source.jid {
                let peers = identities
                    .peers
                    .entry(canonical.clone())
                    .or_insert_with(|| vec![canonical]);
                if !peers.contains(&source.jid) {
                    peers.push(source.jid);
                }
            }
        }
        for lock in locks {
            let jid = identities.canonical(&lock.jid);
            if lock.locked != 0 {
                identities.locked.insert(jid);
            } else {
                identities.locked.remove(&jid);
            }
        }
        for contact in names {
            if meaningful_name(&contact.jid, &contact.name) {
                let jid = identities.canonical(&contact.jid);
                identities.known_names.insert(jid, contact.name);
            }
        }
        for contact in saved {
            let jid = identities.canonical(&contact.jid);
            if meaningful_name(&jid, &contact.name) {
                identities.saved_names.insert(jid, contact.name);
            }
        }
        Ok(identities)
    }
    pub(crate) async fn preserve_identity_links(&self) -> Result<()> {
        self.backend
            .shared()
            .run(|conn| {
                use diesel::connection::SimpleConnection;
                conn.batch_execute(include_str!("../migrations/012_identity_links.sql"))
                    .map_err(db_error)
            })
            .await?;
        Ok(())
    }

    pub(crate) async fn alias_settings(&self) -> Result<Vec<ChatSettings>> {
        let account = self.account as i64;
        Ok(self.backend.shared().read(move|conn| {
            diesel::sql_query("SELECT jid,pinned,archived,muted,mute_until_ms,pin_changed_at,archive_changed_at,mute_changed_at FROM brazap_chat_settings WHERE account=?")
                .bind::<BigInt,_>(account).load(conn).map_err(db_error)
        }).await?)
    }
    pub(crate) async fn canonical_chat(&self, jid: &str) -> Result<String> {
        Ok(self.identities().await?.canonical(jid))
    }
    pub(crate) async fn chat_aliases(&self, jid: &str) -> Result<Vec<String>> {
        Ok(self.identities().await?.aliases(jid))
    }
    pub(crate) async fn message_source(
        &self,
        chat: &str,
        sender: &str,
        id: &str,
    ) -> Result<MessageSource> {
        let identities = self.identities().await?;
        anyhow::ensure!(
            chat != crate::stories::CHAT || !identities.is_locked(sender),
            "This conversation is locked on WhatsApp and is excluded from Brazap."
        );
        let chats = identities.aliases_json(chat);
        let senders = identities.aliases_json(sender);
        let account = self.account as i64;
        let id = id.to_owned();

        Ok(self.backend.shared().read(move|conn| {
            diesel::sql_query("SELECT chat,sender FROM brazap_messages WHERE account=? AND chat IN (SELECT value FROM json_each(?)) AND sender IN (SELECT value FROM json_each(?)) AND id=? ORDER BY CASE json_extract(payload,'$.kind') WHEN 'protocol' THEN -1 WHEN 'placeholder' THEN 0 WHEN 'unsupported' THEN 0 ELSE 1 END DESC,CASE status WHEN 'read' THEN 4 WHEN 'delivered' THEN 3 WHEN 'sent' THEN 2 ELSE 0 END DESC,rowid LIMIT 1")
                .bind::<BigInt,_>(account).bind::<Text,_>(chats).bind::<Text,_>(senders).bind::<Text,_>(id).get_result(conn).map_err(db_error)
        }).await?)
    }
}
