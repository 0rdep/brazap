//! Local read state commits before network sync. Boundaries and exact message
//! references keep delayed events and same-second arrivals from reopening reads.
use crate::{
    service::Service,
    storage::{Archive, db_error},
};
use anyhow::Result;
use diesel::{
    prelude::*,
    sql_types::{BigInt, Text},
};
use std::{collections::BTreeMap, sync::Arc};
use whatsapp_rust::prelude::wa;

#[derive(Clone, QueryableByName, serde::Serialize, serde::Deserialize)]
pub(crate) struct ReadTarget {
    #[diesel(sql_type=Text)]
    pub sender: String,
    #[diesel(sql_type=Text)]
    pub id: String,
    #[diesel(sql_type=BigInt)]
    pub timestamp: i64,
}
#[derive(QueryableByName)]
struct State {
    #[diesel(sql_type=BigInt)]
    through: i64,
    #[diesel(sql_type=BigInt)]
    clock: i64,
    #[diesel(sql_type=BigInt)]
    manual: i64,
}
#[derive(QueryableByName)]
struct Count {
    #[diesel(sql_type=BigInt)]
    value: i64,
}

impl Archive {
    pub(crate) async fn chat_unread(&self, chat: &str) -> Result<i64> {
        let identities = self.identities().await?;
        if identities.is_locked(chat) {
            return Ok(0);
        }
        let account = self.account as i64;
        let aliases = identities.aliases_json(chat);
        let count: Count = self.backend.shared().read(move |conn| {
            diesel::sql_query("SELECT COALESCE(SUM(unread),0) AS value FROM brazap_chats WHERE account=? AND jid IN (SELECT value FROM json_each(?))")
                .bind::<BigInt,_>(account).bind::<Text,_>(aliases).get_result(conn).map_err(db_error)
        }).await?;
        Ok(count.value)
    }

    pub(crate) async fn incoming_is_read(&self, message: &crate::model::Message) -> Result<bool> {
        let account = self.account as i64;
        let aliases = self.identities().await?.aliases_json(&message.chat);
        let (id, sender, timestamp) = (
            message.id.clone(),
            message.sender.clone(),
            message.timestamp,
        );
        let row:Count=self.backend.shared().read(move|conn| {
            diesel::sql_query("SELECT (EXISTS(SELECT 1 FROM brazap_chat_reads WHERE account=? AND chat IN (SELECT value FROM json_each(?)) AND through_timestamp>?) OR EXISTS(SELECT 1 FROM brazap_read_messages WHERE account=? AND chat IN (SELECT value FROM json_each(?)) AND id=? AND (sender='' OR sender=?))) AS value")
                .bind::<BigInt,_>(account).bind::<Text,_>(&aliases).bind::<BigInt,_>(timestamp).bind::<BigInt,_>(account).bind::<Text,_>(&aliases).bind::<Text,_>(id).bind::<Text,_>(sender).get_result(conn).map_err(db_error)
        }).await?;
        Ok(row.value != 0)
    }

    pub(crate) async fn apply_chat_read(
        &self,
        chat: &str,
        through: Option<i64>,
        references: Vec<ReadTarget>,
        at_ms: i64,
        unread: bool,
    ) -> Result<Vec<ReadTarget>> {
        self.apply_read(chat, through, references, at_ms, unread, None)
            .await
    }
    async fn apply_read(
        &self,
        chat: &str,
        through: Option<i64>,
        references: Vec<ReadTarget>,
        at_ms: i64,
        unread: bool,
        queue: Option<bool>,
    ) -> Result<Vec<ReadTarget>> {
        self.allow_chat(chat).await?;
        let identities = self.identities().await?;
        let canonical = identities.canonical(chat);
        let aliases = identities.aliases_json(chat);
        let references: Vec<_> = references
            .into_iter()
            .flat_map(|target| {
                if target.sender.is_empty() {
                    vec![target]
                } else {
                    identities
                        .aliases(&target.sender)
                        .into_iter()
                        .map(|sender| ReadTarget {
                            sender,
                            ..target.clone()
                        })
                        .collect()
                }
            })
            .collect();
        let account = self.account as i64;
        let pending=self.backend.shared().run(move|conn| {
            conn.transaction::<_,diesel::result::Error,_>(|conn| {
                let pending:Vec<ReadTarget>=diesel::sql_query("SELECT m.sender,m.id,m.timestamp FROM brazap_messages m WHERE m.account=? AND m.chat IN (SELECT value FROM json_each(?)) AND json_extract(m.payload,'$.from_me')=0 AND json_extract(m.payload,'$.kind')!='protocol' AND NOT EXISTS(SELECT 1 FROM brazap_chat_reads r WHERE r.account=m.account AND r.chat IN (SELECT value FROM json_each(?)) AND m.timestamp<r.through_timestamp) AND NOT EXISTS(SELECT 1 FROM brazap_read_messages r WHERE r.account=m.account AND r.chat IN (SELECT value FROM json_each(?)) AND r.id=m.id AND (r.sender='' OR r.sender=m.sender)) GROUP BY m.id,m.sender ORDER BY m.timestamp DESC,m.id DESC LIMIT 500")
                    .bind::<BigInt,_>(account).bind::<Text,_>(&aliases).bind::<Text,_>(&aliases).bind::<Text,_>(&aliases).load(conn)?;
                let unknown_references=through==Some(0) && !references.is_empty();
                let old:State=diesel::sql_query("SELECT COALESCE(MAX(through_timestamp),0) AS through,COALESCE(MAX(changed_at_ms),0) AS clock,COALESCE((SELECT manual_unread FROM brazap_chat_reads WHERE account=? AND chat IN (SELECT value FROM json_each(?)) ORDER BY changed_at_ms DESC LIMIT 1),0) AS manual FROM brazap_chat_reads WHERE account=? AND chat IN (SELECT value FROM json_each(?))")
                    .bind::<BigInt,_>(account).bind::<Text,_>(&aliases).bind::<BigInt,_>(account).bind::<Text,_>(&aliases).get_result(conn)?;
                let latest:Count=diesel::sql_query("SELECT COALESCE(MAX(timestamp),0) AS value FROM brazap_messages WHERE account=? AND chat IN (SELECT value FROM json_each(?))")
                    .bind::<BigInt,_>(account).bind::<Text,_>(&aliases).get_result(conn)?;
                let boundary=through.unwrap_or(latest.value);
                let newest=old.through.max(if unread {old.through}else{boundary});
                let manual=if at_ms>=old.clock {i64::from(unread)} else {old.manual};
                // A local/full-range read covers all messages already archived at
                // its boundary; unseen messages in that second remain eligible.
                if !unread && references.is_empty() {
                    diesel::sql_query("INSERT OR IGNORE INTO brazap_read_messages(account,chat,sender,id) SELECT account,?,sender,id FROM brazap_messages WHERE account=? AND chat IN (SELECT value FROM json_each(?)) AND timestamp=? AND json_extract(payload,'$.from_me')=0")
                        .bind::<Text,_>(&canonical).bind::<BigInt,_>(account).bind::<Text,_>(&aliases).bind::<BigInt,_>(boundary).execute(conn)?;
                }
                for target in references {
                    diesel::sql_query("INSERT OR IGNORE INTO brazap_read_messages(account,chat,sender,id) VALUES(?,?,?,?)")
                        .bind::<BigInt,_>(account).bind::<Text,_>(&canonical).bind::<Text,_>(target.sender).bind::<Text,_>(target.id).execute(conn)?;
                }
                diesel::sql_query("INSERT INTO brazap_chat_reads(account,chat,through_timestamp,changed_at_ms,manual_unread) VALUES(?,?,?,?,?) ON CONFLICT(account,chat) DO UPDATE SET through_timestamp=excluded.through_timestamp,changed_at_ms=excluded.changed_at_ms,manual_unread=excluded.manual_unread")
                    .bind::<BigInt,_>(account).bind::<Text,_>(&canonical).bind::<BigInt,_>(newest).bind::<BigInt,_>(old.clock.max(at_ms)).bind::<BigInt,_>(manual).execute(conn)?;
                if unread || unknown_references {
                    if unread && manual!=0 {
                        let existing:Count=diesel::sql_query("SELECT COALESCE(SUM(unread),0) AS value FROM brazap_chats WHERE account=? AND jid IN (SELECT value FROM json_each(?))")
                            .bind::<BigInt,_>(account).bind::<Text,_>(&aliases).get_result(conn)?;
                        if existing.value==0 {
                            diesel::sql_query("INSERT INTO brazap_chats(account,jid,unread) VALUES(?,?,1) ON CONFLICT(account,jid) DO UPDATE SET unread=MAX(unread,1)")
                                .bind::<BigInt,_>(account).bind::<Text,_>(&canonical).execute(conn)?;
                        }
                    }
                    return Ok(pending);
                }
                let count:Count=diesel::sql_query("SELECT COUNT(*) AS value FROM (SELECT m.id,CASE WHEN m.chat LIKE '%@g.us' THEN m.sender ELSE '' END AS author FROM brazap_messages m WHERE m.account=? AND m.chat IN (SELECT value FROM json_each(?)) AND json_extract(m.payload,'$.from_me')=0 AND json_extract(m.payload,'$.kind')!='protocol' AND m.timestamp>=? AND NOT EXISTS(SELECT 1 FROM brazap_read_messages r WHERE r.account=m.account AND r.chat IN (SELECT value FROM json_each(?)) AND r.id=m.id AND (r.sender='' OR r.sender=m.sender)) GROUP BY m.id,author)")
                    .bind::<BigInt,_>(account).bind::<Text,_>(&aliases).bind::<BigInt,_>(newest).bind::<Text,_>(&aliases).get_result(conn)?;
                diesel::sql_query("UPDATE brazap_chats SET unread=0 WHERE account=? AND jid IN (SELECT value FROM json_each(?))")
                    .bind::<BigInt,_>(account).bind::<Text,_>(&aliases).execute(conn)?;
                diesel::sql_query("INSERT INTO brazap_chats(account,jid,unread) VALUES(?,?,?) ON CONFLICT(account,jid) DO UPDATE SET unread=excluded.unread")
                    .bind::<BigInt,_>(account).bind::<Text,_>(&canonical).bind::<BigInt,_>(count.value.max(manual)).execute(conn)?;
                if queue.is_some_and(|force|force || !pending.is_empty()) {
                    enqueue_read_sync(conn,account,&canonical,&pending)?;
                }
                Ok(pending)
            }).map_err(db_error)
        }).await?;
        Ok(pending)
    }

    pub(crate) async fn read_self(&self, chat: &str, ids: &[String], at_ms: i64) -> Result<()> {
        let account = self.account as i64;
        let aliases = self.identities().await?.aliases_json(chat);
        let ids_json = serde_json::to_string(ids)?;
        let through:Count=self.backend.shared().read(move|conn| {
            diesel::sql_query("SELECT COALESCE(MAX(timestamp),0) AS value FROM brazap_messages WHERE account=? AND chat IN (SELECT value FROM json_each(?)) AND id IN (SELECT value FROM json_each(?)) AND json_extract(payload,'$.from_me')=0")
                .bind::<BigInt,_>(account).bind::<Text,_>(aliases).bind::<Text,_>(ids_json).get_result(conn).map_err(db_error)
        }).await?;
        self.apply_chat_read(
            chat,
            Some(through.value),
            ids.iter()
                .map(|id| ReadTarget {
                    sender: String::new(),
                    id: id.clone(),
                    timestamp: 0,
                })
                .collect(),
            at_ms,
            false,
        )
        .await
        .map(|_| ())
    }
}

impl Service {
    pub async fn mark_chat_read(self: &Arc<Self>, chat: String) -> Result<()> {
        let chat = self.archive.canonical_chat(&chat).await?;
        self.archive.allow_chat(&chat).await?;
        let unread = self.archive.chat_unread(&chat).await?;
        // This transaction is the local outcome; transport failure cannot undo it.
        let targets = self
            .archive
            .apply_read(
                &chat,
                None,
                Vec::new(),
                chrono::Utc::now().timestamp_millis(),
                false,
                if self.config.demo {
                    None
                } else {
                    Some(unread > 0)
                },
            )
            .await?;
        self.changed();
        if self.config.demo || (unread == 0 && targets.is_empty()) {
            return Ok(());
        }
        self.flush_read_sync().await;
        Ok(())
    }

    pub(crate) async fn flush_read_sync(self: &Arc<Self>) {
        if self.config.demo {
            return;
        }
        let Ok(client) = self.connected().await else {
            return;
        };
        let Ok(jobs) = self.archive.pending_read_sync().await else {
            return;
        };
        for job in jobs {
            let key = (job.chat.clone(), job.generation);
            if !self
                .read_jobs
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(key.clone())
            {
                continue;
            }
            let service = self.clone();
            let client = client.clone();
            tokio::spawn(async move {
                let _permit = service.read_slots.acquire().await;
                let mut complete = false;
                for attempt in 0..3 {
                    if service.stopping.load(std::sync::atomic::Ordering::Acquire) {
                        break;
                    }
                    if attempt > 0 {
                        tokio::time::sleep(std::time::Duration::from_secs(1 << attempt)).await;
                    }
                    if service.send_read_sync(&job, &client).await.is_ok() {
                        let _ = service.archive.finish_read_sync(&job).await;
                        complete = true;
                        break;
                    }
                }
                if !complete {
                    log::warn!("Read receipts remain queued for reconnection; local read was kept");
                }
                service
                    .read_jobs
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&key);
            });
        }
    }
    async fn send_read_sync(
        &self,
        job: &ReadSync,
        client: &Arc<whatsapp_rust::Client>,
    ) -> Result<()> {
        self.archive.allow_chat(&job.chat).await?;
        anyhow::ensure!(
            Arc::ptr_eq(client, &self.connected().await?),
            "The WhatsApp session changed"
        );
        let jid: whatsapp_rust::Jid = job.chat.parse()?;
        let targets: Vec<ReadTarget> = serde_json::from_str(&job.targets)?;
        if self.settings.read().await.privacy.send_read_receipts {
            let mut groups = BTreeMap::<String, Vec<String>>::new();
            for target in &targets {
                groups
                    .entry(target.sender.clone())
                    .or_default()
                    .push(target.id.clone());
            }
            for (sender, ids) in groups {
                let sender = if job.chat.ends_with("@g.us") {
                    Some(sender.parse()?)
                } else {
                    None
                };
                client
                    .mark_as_read(
                        &jid,
                        sender.as_ref(),
                        &ids.iter().map(String::as_str).collect::<Vec<_>>(),
                    )
                    .await?;
            }
        }
        let mut range = wa::sync_action_value::SyncActionMessageRange::default();
        range.last_message_timestamp = targets.iter().map(|t| t.timestamp).max();
        client
            .chat_actions()
            .mark_chat_as_read(&jid, true, Some(range))
            .await?;
        Ok(())
    }
}

#[derive(QueryableByName)]
struct ReadSync {
    #[diesel(sql_type=Text)]
    chat: String,
    #[diesel(sql_type=Text)]
    targets: String,
    #[diesel(sql_type=BigInt)]
    generation: i64,
}
impl Archive {
    pub(crate) async fn clear_read_sync(&self) -> Result<()> {
        let account = self.account as i64;
        self.backend
            .shared()
            .run(move |conn| {
                diesel::sql_query("DELETE FROM brazap_read_sync WHERE account=?")
                    .bind::<BigInt, _>(account)
                    .execute(conn)
                    .map_err(db_error)
            })
            .await?;
        Ok(())
    }
    async fn pending_read_sync(&self) -> Result<Vec<ReadSync>> {
        let account = self.account as i64;
        Ok(self.backend.shared().read(move|conn| {
            diesel::sql_query("SELECT chat,targets,generation FROM brazap_read_sync WHERE account=? LIMIT 500")
                .bind::<BigInt,_>(account).load(conn).map_err(db_error)
        }).await?)
    }
    async fn finish_read_sync(&self, job: &ReadSync) -> Result<()> {
        let (account, chat, generation) = (self.account as i64, job.chat.clone(), job.generation);
        self.backend
            .shared()
            .run(move |conn| {
                diesel::sql_query(
                    "DELETE FROM brazap_read_sync WHERE account=? AND chat=? AND generation=?",
                )
                .bind::<BigInt, _>(account)
                .bind::<Text, _>(chat)
                .bind::<BigInt, _>(generation)
                .execute(conn)
                .map_err(db_error)
            })
            .await?;
        Ok(())
    }
}

fn enqueue_read_sync(
    conn: &mut SqliteConnection,
    account: i64,
    chat: &str,
    targets: &[ReadTarget],
) -> diesel::QueryResult<()> {
    let mut targets = targets.to_vec();
    let old: Option<ReadSync> = diesel::sql_query(
        "SELECT chat,targets,generation FROM brazap_read_sync WHERE account=? AND chat=?",
    )
    .bind::<BigInt, _>(account)
    .bind::<Text, _>(chat)
    .get_result(conn)
    .optional()?;
    if let Some(old) = old
        && let Ok(mut saved) = serde_json::from_str::<Vec<ReadTarget>>(&old.targets)
    {
        targets.append(&mut saved);
    }
    targets.sort_by(|a, b| (&a.id, &a.sender).cmp(&(&b.id, &b.sender)));
    targets.dedup_by(|a, b| a.id == b.id && a.sender == b.sender);
    let targets = serde_json::to_string(&targets)
        .map_err(|e| diesel::result::Error::SerializationError(Box::new(e)))?;
    let generation = chrono::Utc::now()
        .timestamp_nanos_opt()
        .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());
    diesel::sql_query("INSERT INTO brazap_read_sync(account,chat,targets,generation) VALUES(?,?,?,?) ON CONFLICT(account,chat) DO UPDATE SET targets=excluded.targets,generation=MAX(generation+1,excluded.generation)")
        .bind::<BigInt,_>(account).bind::<Text,_>(chat).bind::<Text,_>(targets).bind::<BigInt,_>(generation).execute(conn)?;
    Ok(())
}
