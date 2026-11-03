//! Fetch older history gradually after startup sync. UI and background requests
//! share their durable correlation record and send lock; neither marks chats read.

use crate::{
    model::{MessageCursor, now},
    service::Service,
    storage::{Archive, db_error},
};
use anyhow::Result;
use diesel::{
    prelude::*,
    sql_types::{BigInt, Text},
};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::Notify;
use whatsapp_rust::Client;

const PAGE_SIZE: i32 = 100;
const QUIET: Duration = Duration::from_secs(10);
const RETRY: i64 = 15 * 60;
const EMPTY_RETRY: i64 = 24 * 60 * 60;

pub(crate) struct Queue {
    wake: Arc<Notify>,
    last_sync: Mutex<Instant>,
    active: AtomicBool,
    queued: AtomicUsize,
    pages: AtomicU64,
    requests: AtomicU64,
}
impl Default for Queue {
    fn default() -> Self {
        Self {
            wake: Arc::new(Notify::new()),
            last_sync: Mutex::new(Instant::now()),
            active: AtomicBool::new(false),
            queued: AtomicUsize::new(0),
            pages: AtomicU64::new(0),
            requests: AtomicU64::new(0),
        }
    }
}
impl Queue {
    pub(crate) fn synced(&self) {
        *self.last_sync.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
        self.wake.notify_one();
    }
    pub(crate) fn wake(&self) {
        self.wake.notify_one();
    }
    pub(crate) fn report(&self) -> Value {
        json!({"active":self.active.load(Ordering::Relaxed),"queued_chats":self.queued.load(Ordering::Relaxed),"pages_received":self.pages.load(Ordering::Relaxed),"requests_sent":self.requests.load(Ordering::Relaxed)})
    }
}

#[derive(QueryableByName)]
struct Boundary {
    #[diesel(sql_type=BigInt)]
    timestamp: i64,
    #[diesel(sql_type=Text)]
    id: String,
    #[diesel(sql_type=Text)]
    sender: String,
}
#[derive(QueryableByName)]
struct Request {
    #[diesel(sql_type=Text)]
    request_id: String,
    #[diesel(sql_type=Text)]
    anchor_id: String,
    #[diesel(sql_type=Text)]
    anchor_sender: String,
    #[diesel(sql_type=BigInt)]
    anchor_timestamp: i64,
    #[diesel(sql_type=BigInt)]
    requested_at: i64,
    #[diesel(sql_type=BigInt)]
    complete: i64,
    #[diesel(sql_type=BigInt)]
    next_auto_at: i64,
}
impl Request {
    fn matches(&self, cursor: &MessageCursor) -> bool {
        self.anchor_id == cursor.id
            && self.anchor_sender == cursor.sender
            && self.anchor_timestamp == cursor.timestamp
    }
}

impl Archive {
    async fn history_boundary(&self, chat: String) -> Result<Option<MessageCursor>> {
        let account = self.account as i64;
        let chat = self.identities().await?.aliases_json(&chat);
        let row:Option<Boundary>=self.backend.shared().read(move|conn| {
            diesel::sql_query("SELECT timestamp,id,sender FROM brazap_messages WHERE account=? AND chat IN (SELECT value FROM json_each(?)) AND timestamp>0 AND id!='' AND json_extract(payload,'$.kind')!='protocol' ORDER BY timestamp,id,sender LIMIT 1")
                .bind::<BigInt,_>(account).bind::<Text,_>(chat).get_result(conn).optional().map_err(db_error)
        }).await?;
        Ok(row.map(|r| MessageCursor {
            timestamp: r.timestamp,
            id: r.id,
            sender: r.sender,
        }))
    }
    async fn has_older_history(&self, chat: &str, cursor: &MessageCursor) -> Result<bool> {
        Ok(self
            .history_boundary(chat.into())
            .await?
            .is_some_and(|old| {
                (old.timestamp, old.id.as_str(), old.sender.as_str())
                    < (cursor.timestamp, cursor.id.as_str(), cursor.sender.as_str())
            }))
    }
    async fn history_request(&self, chat: String) -> Result<Option<Request>> {
        let account = self.account as i64;
        let aliases = serde_json::to_string(&self.chat_aliases(&chat).await?)?;
        Ok(self.backend.shared().read(move|conn| {
            diesel::sql_query("SELECT request_id,anchor_id,anchor_sender,anchor_timestamp,requested_at,complete,next_auto_at FROM brazap_history_requests WHERE account=? AND chat IN (SELECT value FROM json_each(?)) ORDER BY requested_at DESC LIMIT 1")
                .bind::<BigInt,_>(account).bind::<Text,_>(aliases).get_result(conn).optional().map_err(db_error)
        }).await?)
    }
    async fn start_history_request(
        &self,
        chat: String,
        cursor: MessageCursor,
        id: String,
    ) -> Result<()> {
        let account = self.account as i64;
        self.backend.shared().run(move|conn| {
            conn.transaction::<_,diesel::result::Error,_>(|conn| {
            diesel::sql_query("INSERT INTO brazap_history_requests(account,chat,request_id,anchor_id,anchor_sender,anchor_timestamp,requested_at) VALUES(?,?,?,?,?,?,?) ON CONFLICT(account,chat) DO UPDATE SET request_id=excluded.request_id,anchor_id=excluded.anchor_id,anchor_sender=excluded.anchor_sender,anchor_timestamp=excluded.anchor_timestamp,requested_at=excluded.requested_at,response_at=0,complete=0,next_auto_at=0")
                .bind::<BigInt,_>(account).bind::<Text,_>(&chat).bind::<Text,_>(id).bind::<Text,_>(cursor.id).bind::<Text,_>(cursor.sender).bind::<BigInt,_>(cursor.timestamp).bind::<BigInt,_>(now()).execute(conn)?;
            diesel::sql_query("INSERT INTO brazap_history_boundaries SELECT * FROM brazap_history_requests WHERE account=? AND chat=? ON CONFLICT(account,chat,anchor_id,anchor_sender,anchor_timestamp) DO UPDATE SET request_id=excluded.request_id,requested_at=excluded.requested_at,response_at=0,complete=0,next_auto_at=0")
                .bind::<BigInt,_>(account).bind::<Text,_>(chat).execute(conn)?;
            Ok(())
            }).map_err(db_error)
        }).await?;
        Ok(())
    }
    async fn history_boundary_request(
        &self,
        chat: &str,
        cursor: &MessageCursor,
    ) -> Result<Option<Request>> {
        let identities = self.identities().await?;
        let chats = identities.aliases_json(chat);
        let senders = identities.aliases_json(&cursor.sender);
        let (account, id, timestamp) = (self.account as i64, cursor.id.clone(), cursor.timestamp);
        Ok(self.backend.shared().read(move|conn| {
            diesel::sql_query("SELECT request_id,anchor_id,anchor_sender,anchor_timestamp,requested_at,complete,next_auto_at FROM brazap_history_boundaries WHERE account=? AND chat IN (SELECT value FROM json_each(?)) AND anchor_id=? AND anchor_sender IN (SELECT value FROM json_each(?)) AND anchor_timestamp=? ORDER BY complete DESC,requested_at DESC LIMIT 1")
                .bind::<BigInt,_>(account).bind::<Text,_>(chats).bind::<Text,_>(id).bind::<Text,_>(senders).bind::<BigInt,_>(timestamp).get_result(conn).optional().map_err(db_error)
        }).await?)
    }
    pub(crate) async fn history_response(
        &self,
        id: Option<&str>,
        progress: Option<u32>,
    ) -> Result<()> {
        let Some(id) = id.map(str::to_owned) else {
            return Ok(());
        };
        let account = self.account as i64;
        self.backend.shared().run(move|conn| {
            conn.transaction::<_,diesel::result::Error,_>(|conn| {
            // A response without chunk progress is a single completed payload.
            // Keep completion monotonic if duplicate/out-of-order chunks arrive.
            diesel::sql_query("UPDATE brazap_history_requests SET response_at=?,complete=MAX(complete,?) WHERE account=? AND request_id=?")
                .bind::<BigInt,_>(now()).bind::<BigInt,_>(i64::from(progress.is_none_or(|p|p>=100))).bind::<BigInt,_>(account).bind::<Text,_>(&id).execute(conn)?;
            diesel::sql_query("UPDATE brazap_history_boundaries SET response_at=?,complete=MAX(complete,?) WHERE account=? AND request_id=?")
                .bind::<BigInt,_>(now()).bind::<BigInt,_>(i64::from(progress.is_none_or(|p|p>=100))).bind::<BigInt,_>(account).bind::<Text,_>(id).execute(conn)?;
            Ok(())
            }).map_err(db_error)
        }).await?;
        Ok(())
    }
    async fn defer_auto_history(
        &self,
        chat: String,
        cursor: MessageCursor,
        seconds: i64,
        failed: bool,
    ) -> Result<()> {
        let account = self.account as i64;
        self.backend.shared().run(move|conn| {
            conn.transaction::<_,diesel::result::Error,_>(|conn| {
            diesel::sql_query("UPDATE brazap_history_requests SET next_auto_at=?,requested_at=CASE WHEN ? THEN 0 ELSE requested_at END WHERE account=? AND chat=? AND anchor_id=? AND anchor_sender=? AND anchor_timestamp=?")
                .bind::<BigInt,_>(now()+seconds).bind::<BigInt,_>(i64::from(failed)).bind::<BigInt,_>(account).bind::<Text,_>(&chat).bind::<Text,_>(&cursor.id).bind::<Text,_>(&cursor.sender).bind::<BigInt,_>(cursor.timestamp).execute(conn)?;
            diesel::sql_query("UPDATE brazap_history_boundaries SET next_auto_at=? WHERE account=? AND chat=? AND anchor_id=? AND anchor_sender=? AND anchor_timestamp=?")
                .bind::<BigInt,_>(now()+seconds).bind::<BigInt,_>(account).bind::<Text,_>(chat).bind::<Text,_>(cursor.id).bind::<Text,_>(cursor.sender).bind::<BigInt,_>(cursor.timestamp).execute(conn)?;
            Ok(())
            }).map_err(db_error)
        }).await?;
        Ok(())
    }
}

pub(crate) enum Fetched {
    Older,
    Empty,
    Pending,
}

impl Service {
    pub(crate) fn spawn_history_prefetch(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        let wake = self.history_prefetch.wake.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {_=wake.notified()=>{},_=tokio::time::sleep(Duration::from_secs(RETRY as u64))=>{}}
                let Some(service) = weak.upgrade() else {
                    break;
                };
                if service.stopping.load(Ordering::Relaxed) {
                    break;
                }
                if let Err(error) = service.prefetch_history().await {
                    log::debug!("Background history paused: {error}");
                }
            }
        });
    }

    async fn same_history_connection(&self, client: &Arc<Client>) -> bool {
        !self.stopping.load(Ordering::Relaxed)
            && self
                .connected()
                .await
                .is_ok_and(|current| Arc::ptr_eq(&current, client))
    }

    async fn settle_history(&self, client: &Arc<Client>) -> Result<bool> {
        loop {
            if !self.same_history_connection(client).await {
                return Ok(false);
            }
            // This public SDK barrier accounts for queued/running history work,
            // offline delivery, and reconnects (a disconnect is not completion).
            client
                .wait_for_startup_sync(Duration::from_secs(60))
                .await?;
            let remaining = QUIET.saturating_sub(
                self.history_prefetch
                    .last_sync
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .elapsed(),
            );
            if remaining.is_zero() {
                return Ok(true);
            }
            tokio::time::sleep(remaining).await;
        }
    }

    async fn prefetch_history(&self) -> Result<()> {
        if !self.chat_locks_ready.load(Ordering::Acquire) {
            return Ok(());
        }
        let client = self.connected().await?;
        if !self.settle_history(&client).await? {
            return Ok(());
        }
        let chats = self.archive.chats().await?;
        let eligible = |chat: &&crate::model::Chat| {
            chat.jid.ends_with("@g.us")
                || chat.jid.ends_with("@s.whatsapp.net")
                || chat.jid.ends_with("@lid")
        };
        // Pinned-first UI ordering must not turn "recent" into oldest-pinned-first.
        let mut recent: Vec<_> = chats
            .iter()
            .filter(eligible)
            .filter(|c| !c.pinned)
            .collect();
        recent.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| a.jid.cmp(&b.jid))
        });
        let mut queue: VecDeque<_> = chats
            .iter()
            .filter(eligible)
            .filter(|c| c.pinned)
            .chain(recent.into_iter().take(10))
            .map(|c| c.jid.clone())
            .collect();
        self.history_prefetch.active.store(true, Ordering::Relaxed);
        // Warm each eligible chat once. Further pages are explicitly requested
        // by scrolling history; restarting must not ping the phone again.
        let result = async {
            for _ in 0..128 {
                self.history_prefetch
                    .queued
                    .store(queue.len(), Ordering::Relaxed);
                let Some(chat) = queue.pop_front() else {
                    break;
                };
                if !self.settle_history(&client).await? {
                    break;
                }
                if self.archive.was_auto_prefetched(&chat).await?
                    || self.archive.history_request(chat.clone()).await?.is_some()
                    || self.archive.history_is_complete(&chat).await?
                    || self.archive.identities().await?.is_locked(&chat)
                {
                    continue;
                }
                let Some(cursor) = self.archive.history_boundary(chat.clone()).await? else {
                    continue;
                };
                if self
                    .archive
                    .history_request(chat.clone())
                    .await?
                    .is_some_and(|r| r.matches(&cursor) && r.next_auto_at > now())
                {
                    continue;
                }
                match self.fetch_older_history(&client, &chat, &cursor).await {
                    Ok(Fetched::Older) => {
                        self.history_prefetch.pages.fetch_add(1, Ordering::Relaxed);
                        self.changed();
                        self.archive.mark_auto_prefetched(&chat).await?;
                    }
                    Ok(Fetched::Empty) => {
                        self.archive.mark_auto_prefetched(&chat).await?;
                        self.archive
                            .defer_auto_history(chat, cursor, EMPTY_RETRY, false)
                            .await?
                    }
                    Ok(Fetched::Pending) => {
                        self.archive
                            .defer_auto_history(chat, cursor, RETRY, false)
                            .await?
                    }
                    Err(error) => {
                        log::debug!("Background history request deferred: {error}");
                        self.archive
                            .defer_auto_history(chat, cursor, RETRY, true)
                            .await?;
                    }
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            Ok(())
        }
        .await;
        self.history_prefetch.active.store(false, Ordering::Relaxed);
        self.history_prefetch.queued.store(0, Ordering::Relaxed);
        result
    }

    pub(crate) async fn fetch_older_history(
        &self,
        client: &Arc<Client>,
        chat: &str,
        cursor: &MessageCursor,
    ) -> Result<Fetched> {
        self.archive.allow_chat(chat).await?;
        if self.archive.has_older_history(chat, cursor).await? {
            self.archive.mark_auto_prefetched(chat).await?;
            return Ok(Fetched::Older);
        }
        if self.archive.history_is_complete(chat).await? {
            return Ok(Fetched::Empty);
        }
        let guard = self.history_lock.lock().await;
        if !self.same_history_connection(client).await {
            anyhow::bail!("Connection changed before requesting history");
        }
        if self.archive.has_older_history(chat, cursor).await? {
            self.archive.mark_auto_prefetched(chat).await?;
            return Ok(Fetched::Older);
        }
        let previous = self.archive.history_request(chat.into()).await?;
        // A preceding page may still be streaming even after its first messages
        // moved the oldest boundary. Do not replace its correlation record yet.
        if previous
            .as_ref()
            .is_some_and(|r| !r.matches(cursor) && r.complete == 0 && r.requested_at > now() - 60)
        {
            return Ok(Fetched::Pending);
        }
        // A page can be requested through a different phone/LID alias or after
        // newer requests replaced the per-chat cursor. Never repeat completed
        // boundaries, and give pending phone requests time to finish.
        let recent = self
            .archive
            .history_boundary_request(chat, cursor)
            .await?
            .filter(|r| {
                r.complete != 0 || r.requested_at > now() - RETRY || r.next_auto_at > now()
            });
        let request_id = if let Some(previous) = recent {
            if previous.complete != 0 {
                return Ok(if self.archive.has_older_history(chat, cursor).await? {
                    Fetched::Older
                } else {
                    self.archive
                        .complete_history(chat.into(), "no_older_messages")
                        .await?;
                    Fetched::Empty
                });
            }
            previous.request_id
        } else {
            let id = client.generate_message_id();
            // Commit the ID before sending: the reply can arrive before send returns.
            self.archive
                .start_history_request(chat.into(), cursor.clone(), id.to_string())
                .await?;
            let result = tokio::time::timeout(
                Duration::from_secs(30),
                client.fetch_message_history_with_id(
                    &chat.parse()?,
                    &cursor.id,
                    cursor.sender == "self",
                    cursor.timestamp.saturating_mul(1000),
                    PAGE_SIZE,
                    &id,
                ),
            )
            .await;
            match result {
                Ok(Ok(())) => {
                    self.history_prefetch
                        .requests
                        .fetch_add(1, Ordering::Relaxed);
                }
                other => {
                    self.archive
                        .defer_auto_history(chat.into(), cursor.clone(), RETRY, true)
                        .await?;
                    match other {
                        Ok(Err(e)) => return Err(e),
                        _ => anyhow::bail!("Phone history request timed out"),
                    }
                }
            }
            id.to_string()
        };
        drop(guard);
        for _ in 0..30 {
            if !self.same_history_connection(client).await {
                anyhow::bail!("Disconnected while loading older history");
            }
            if self.archive.has_older_history(chat, cursor).await? {
                self.archive.mark_auto_prefetched(chat).await?;
                return Ok(Fetched::Older);
            }
            if self
                .archive
                .history_request(chat.into())
                .await?
                .is_some_and(|r| r.request_id == request_id && r.complete != 0)
            {
                return Ok(if self.archive.has_older_history(chat, cursor).await? {
                    Fetched::Older
                } else {
                    self.archive
                        .complete_history(chat.into(), "no_older_messages")
                        .await?;
                    Fetched::Empty
                });
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        Ok(Fetched::Pending)
    }
}
