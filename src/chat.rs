use crate::{
    model::{MediaPage, MessageCursor, now},
    search::decode,
    storage::{Archive, MESSAGE_SELECT, MessageRow, db_error},
};
use anyhow::{Result, ensure};
use diesel::{
    prelude::*,
    sql_types::{BigInt, Binary, Text},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatAction {
    Mute,
    Pin,
    Archive,
}

#[derive(QueryableByName)]
struct EventRow {
    #[diesel(sql_type=BigInt)]
    cursor: i64,
    #[diesel(sql_type=Text)]
    payload: String,
}

#[derive(QueryableByName)]
struct HistoryRow {
    #[diesel(sql_type=BigInt)]
    cursor: i64,
    #[diesel(sql_type=Binary)]
    data: Vec<u8>,
}

impl Archive {
    pub(crate) async fn restore_contact_names(&self) -> Result<()> {
        if self.preference("contact_names_v1".into()).await?.is_some() {
            return Ok(());
        }
        let mut cursor = 0;
        loop {
            let account = self.account as i64;
            let rows:Vec<EventRow>=self.backend.shared().read(move |conn| {
                diesel::sql_query("SELECT rowid AS cursor,payload FROM brazap_events WHERE account=? AND rowid>? AND kind IN ('ContactUpdate','ContactRemoved') ORDER BY rowid LIMIT 256")
                    .bind::<BigInt,_>(account).bind::<BigInt,_>(cursor).load(conn).map_err(db_error)
            }).await?;
            if rows.is_empty() {
                break;
            }
            for row in rows {
                cursor = row.cursor;
                let value: serde_json::Value = serde_json::from_str(&row.payload)?;
                if let Some(contact) = value.get("ContactUpdate")
                    && let Some(jid) = contact["jid"].as_str()
                {
                    let name = contact["action"]["full_name"]
                        .as_str()
                        .filter(|n| !n.trim().is_empty())
                        .or_else(|| contact["action"]["first_name"].as_str())
                        .unwrap_or("");
                    self.contact(
                        jid.into(),
                        name.into(),
                        serde_json::to_string(contact)?,
                        true,
                    )
                    .await?;
                }
                if let Some(contact) = value.get("ContactRemoved")
                    && let Some(jid) = contact["jid"].as_str()
                {
                    let account = self.account as i64;
                    let jid = jid.to_owned();
                    self.backend
                        .shared()
                        .run(move |conn| {
                            diesel::sql_query(
                                "DELETE FROM brazap_saved_contacts WHERE account=? AND jid=?",
                            )
                            .bind::<BigInt, _>(account)
                            .bind::<Text, _>(jid)
                            .execute(conn)
                            .map_err(db_error)?;
                            Ok(())
                        })
                        .await?;
                }
            }
        }
        self.set_preference("contact_names_v1".into(), "1".into())
            .await
    }

    pub async fn set_chat_action(
        &self,
        jid: String,
        action: ChatAction,
        enabled: bool,
        mute_until: i64,
        at: i64,
    ) -> Result<()> {
        let account = self.account as i64;
        let aliases = self.chat_aliases(&jid).await?;
        let (column, clock) = match action {
            ChatAction::Mute => ("muted", "mute_changed_at"),
            ChatAction::Pin => ("pinned", "pin_changed_at"),
            ChatAction::Archive => ("archived", "archive_changed_at"),
        };
        let sql = format!(
            "INSERT INTO brazap_chat_settings(account,jid,{column},mute_until_ms,{clock}) VALUES(?,?,?,?,?) ON CONFLICT(account,jid) DO UPDATE SET {column}=excluded.{column},{} {clock}=excluded.{clock} WHERE excluded.{clock}>=brazap_chat_settings.{clock}",
            if matches!(action, ChatAction::Mute) {
                "mute_until_ms=excluded.mute_until_ms,"
            } else {
                ""
            }
        );
        self.backend
            .shared()
            .run(move |conn| {
                for jid in aliases {
                    diesel::sql_query(&sql)
                        .bind::<BigInt, _>(account)
                        .bind::<Text, _>(jid)
                        .bind::<BigInt, _>(i64::from(enabled))
                        .bind::<BigInt, _>(mute_until)
                        .bind::<BigInt, _>(at)
                        .execute(conn)
                        .map_err(db_error)?;
                }
                Ok(())
            })
            .await?;
        Ok(())
    }

    pub async fn media_page(
        &self,
        chat: String,
        kind: String,
        cursor: Option<MessageCursor>,
    ) -> Result<MediaPage> {
        ensure!(
            ["media", "files", "audio"].contains(&kind.as_str()),
            "Unknown media category"
        );
        self.allow_chat(&chat).await?;
        let account = self.account as i64;
        let identities = self.identities().await?;
        let aliases = identities.aliases_json(&chat);
        let sources = identities.aliases(&chat);
        let scope = identities.timeline_scope(&chat);
        let cursor = cursor.unwrap_or(MessageCursor {
            timestamp: i64::MAX,
            id: String::new(),
            sender: String::new(),
        });
        let rows:Vec<MessageRow>=self.backend.shared().read(move |conn|{
            let mut rows=Vec::new();
            for source in sources {
            rows.extend(diesel::sql_query(format!("{MESSAGE_SELECT} WHERE m.account=? AND {scope} AND CASE ? WHEN 'media' THEN json_extract(m.payload,'$.kind') IN ('image','video','sticker') WHEN 'files' THEN json_extract(m.payload,'$.kind')='file' ELSE json_extract(m.payload,'$.kind')='audio' END AND (m.timestamp,m.id,m.sender)<(?,?,?) ORDER BY m.timestamp DESC,m.id DESC,m.sender DESC LIMIT 37"))
                .bind::<BigInt,_>(account).bind::<Text,_>(source).bind::<Text,_>(&aliases).bind::<Text,_>(&kind).bind::<BigInt,_>(cursor.timestamp).bind::<Text,_>(&cursor.id).bind::<Text,_>(&cursor.sender).load::<MessageRow>(conn).map_err(db_error)?);
            } Ok(rows)
        }).await?;
        let mut items = rows
            .into_iter()
            .map(|row| decode(row).map(|m| identities.message(m)))
            .collect::<Result<Vec<_>>>()?;
        items.sort_by(|a, b| (b.timestamp, &b.id, &b.sender).cmp(&(a.timestamp, &a.id, &a.sender)));
        let more = items.len() > 36;
        items.truncate(36);
        let next = if more {
            items.last().map(|m| m.cursor())
        } else {
            None
        };
        Ok(MediaPage { items, next })
    }

    pub async fn restore_chat_settings(&self) -> Result<()> {
        if self.preference("chat_settings_v4".into()).await?.is_some() {
            return Ok(());
        }
        let mut cursor = 0;
        loop {
            let account = self.account as i64;
            let rows:Vec<EventRow>=self.backend.shared().read(move |conn|{
                diesel::sql_query("SELECT rowid AS cursor,payload FROM brazap_events WHERE account=? AND rowid>? AND kind IN ('MuteUpdate','PinUpdate','ArchiveUpdate','LockChatUpdate') ORDER BY rowid LIMIT 128")
                    .bind::<BigInt,_>(account).bind::<BigInt,_>(cursor).load(conn).map_err(db_error)
            }).await?;
            if rows.is_empty() {
                break;
            }
            cursor = rows.last().unwrap().cursor;
            for row in rows {
                let value: serde_json::Value = serde_json::from_str(&row.payload)?;
                if let Some(v) = value.get("LockChatUpdate") {
                    if let (Some(jid), Some(locked)) =
                        (v["jid"].as_str(), v["action"]["locked"].as_bool())
                    {
                        let at = v
                            .get("action_timestamp")
                            .or_else(|| v.get("timestamp"))
                            .and_then(|v| v.as_str())
                            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                            .map(|t| t.timestamp_millis())
                            .unwrap_or(0);
                        self.set_chat_locked(jid.into(), locked, at).await?;
                    }
                }
                for (key, action, flag) in [
                    ("MuteUpdate", ChatAction::Mute, "muted"),
                    ("PinUpdate", ChatAction::Pin, "pinned"),
                    ("ArchiveUpdate", ChatAction::Archive, "archived"),
                ] {
                    let Some(v) = value.get(key) else {
                        continue;
                    };
                    let Some(jid) = v.get("jid").and_then(|v| v.as_str()) else {
                        continue;
                    };
                    let at = v
                        .get("action_timestamp")
                        .or_else(|| v.get("timestamp"))
                        .and_then(|v| v.as_str())
                        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                        .map(|t| t.timestamp_millis())
                        .unwrap_or(0);
                    let enabled = v["action"][flag].as_bool().unwrap_or(false);
                    let until = v["action"]["mute_end_timestamp"]
                        .as_i64()
                        .or_else(|| {
                            v["action"]["mute_end_timestamp"]
                                .as_str()
                                .and_then(|s| s.parse().ok())
                        })
                        .unwrap_or(if enabled { -1 } else { 0 });
                    self.set_chat_action(jid.into(), action, enabled, until, at)
                        .await?;
                }
            }
        }
        let mut cursor = 0;
        loop {
            let account = self.account as i64;
            let row:Option<HistoryRow>=self.backend.shared().read(move |conn|{
                diesel::sql_query("SELECT rowid AS cursor,data FROM brazap_history WHERE account=? AND stage=1 AND rowid>? ORDER BY rowid LIMIT 1")
                    .bind::<BigInt,_>(account).bind::<BigInt,_>(cursor).get_result(conn).optional().map_err(db_error)
            }).await?;
            let Some(row) = row else {
                break;
            };
            cursor = row.cursor;
            let history =
                tokio::task::spawn_blocking(move || crate::history::decode_history(&row.data))
                    .await??;
            for chat in history.conversations {
                if let Some(locked) = chat.locked {
                    self.set_chat_locked(chat.id.clone(), locked, 0).await?;
                } else {
                    self.note_history_visibility(chat.id.clone()).await?;
                }
                if chat.end_of_history_transfer_type == Some(whatsapp_rust::prelude::wa::conversation::EndOfHistoryTransferType::COMPLETE_AND_NO_MORE_MESSAGE_REMAIN_ON_PRIMARY) {
                    self.complete_history(chat.id.clone(), "server_complete")
                        .await?;
                }
                if let Some(pinned) = chat.pinned {
                    self.set_chat_action(chat.id.clone(), ChatAction::Pin, pinned > 0, 0, 0)
                        .await?;
                }
                if let Some(archived) = chat.archived {
                    self.set_chat_action(chat.id.clone(), ChatAction::Archive, archived, 0, 0)
                        .await?;
                }
                // History timestamps may use an all-ones sentinel for an indefinite mute.
                if let Some(until) = chat.mute_end_time {
                    let until = if until == u64::MAX || until == u32::MAX as u64 {
                        -1
                    } else if until < 100_000_000_000 {
                        (until as i64).saturating_mul(1000)
                    } else {
                        i64::try_from(until).unwrap_or(i64::MAX)
                    };
                    self.set_chat_action(
                        chat.id,
                        ChatAction::Mute,
                        until == -1 || until > now() * 1000,
                        until,
                        0,
                    )
                    .await?;
                }
            }
        }
        self.set_preference("chat_settings_v4".into(), "1".into())
            .await
    }
}

impl crate::service::Service {
    pub async fn chat_action(&self, chat: String, action: ChatAction, enabled: bool) -> Result<()> {
        self.archive.allow_chat(&chat).await?;
        let jid = chat.parse::<whatsapp_rust::Jid>()?;
        if self.config.demo {
            self.archive
                .set_chat_action(
                    chat,
                    action,
                    enabled,
                    if enabled { -1 } else { 0 },
                    now() * 1000,
                )
                .await?;
            self.changed();
            return Ok(());
        }
        let client = self.connected().await?;
        match (action, enabled) {
            (ChatAction::Mute, true) => client.chat_actions().mute_chat(&jid).await?,
            (ChatAction::Mute, false) => client.chat_actions().unmute_chat(&jid).await?,
            (ChatAction::Pin, true) => client.chat_actions().pin_chat(&jid).await?,
            (ChatAction::Pin, false) => client.chat_actions().unpin_chat(&jid).await?,
            (ChatAction::Archive, true) => client.chat_actions().archive_chat(&jid, None).await?,
            (ChatAction::Archive, false) => {
                client.chat_actions().unarchive_chat(&jid, None).await?
            }
        }
        self.archive
            .set_chat_action(
                chat,
                action,
                enabled,
                if enabled { -1 } else { 0 },
                now() * 1000,
            )
            .await?;
        self.changed();
        Ok(())
    }
}
