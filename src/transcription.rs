//! Local speech-to-text for voice messages.
//!
//! transcribe.cpp (ggml) runs one speech model on this computer: OpenAI's
//! multilingual Whisper (*base* by default, *small* or *large-v3 turbo*) or
//! NVIDIA's Parakeet TDT v3 for 25 European languages, chosen in Settings
//! ([`MODELS`]). The selected model downloads the first time it is needed (or
//! when it is chosen) into the derived cache and is verified by size and
//! SHA-256; clearing the cache only means downloading it again. Only the model
//! in use is kept: the others are removed once it is ready. Transcripts are
//! stored in SQLite (`brazap_transcripts`) next to the message they describe
//! and are attached to [`Message::transcript`] whenever messages are read, so
//! the desktop window and MCP clients see the same text.
//!
//! One worker transcribes at a time with at most one model loaded, memory-mapped
//! and unloaded after [`UNLOAD_AFTER`] without work (see `engine`). Voice
//! messages on screen come first, then fresh incoming voice messages. Locked
//! chats are never transcribed.
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use diesel::{
    OptionalExtension, RunQueryDsl,
    sql_types::{BigInt, Text},
};
use serde::{Deserialize, Serialize};

use crate::{
    model::Message,
    service::Service,
    storage::{Archive, db_error, project_message},
};

mod catalog;
#[cfg(feature = "transcription")]
mod engine;
mod files;

pub use catalog::{DEFAULT_MODEL, MODELS, PARAKEET_V3_LANGUAGES, SpeechModel};

/// Longer recordings are transcribed up to this point and marked partial.
#[cfg_attr(not(feature = "transcription"), allow(dead_code))]
const MAX_SECONDS: usize = 15 * 60;
#[cfg_attr(not(feature = "transcription"), allow(dead_code))]
const SAMPLE_RATE: usize = 16_000;
/// The model stays loaded (memory-mapped) while voice messages keep arriving.
const UNLOAD_AFTER: Duration = Duration::from_secs(5 * 60);
/// A download or connection failure is reported for this long before the same
/// message may be queued again.
const RETRY_AFTER: Duration = Duration::from_secs(60);
const BACKGROUND_LIMIT: usize = 200;

/// A voice message's text. Stored results are `done` or `failed`; `queued` and
/// `running` describe work in progress and are never stored.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Transcript {
    pub status: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
    /// The detected language, as a Whisper language code (`pt`, `en`, …).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub language: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The recording was longer than the limit; only its beginning is included.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub partial: bool,
    /// The [`SpeechModel::name`] that produced a stored result.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub model: String,
}

impl Transcript {
    fn pending(status: &str) -> Self {
        Self {
            status: status.into(),
            ..Default::default()
        }
    }
    fn failed(error: impl Into<String>) -> Self {
        Self {
            status: "failed".into(),
            error: Some(error.into()),
            ..Default::default()
        }
    }
    pub fn finished(&self) -> bool {
        matches!(self.status.as_str(), "done" | "failed")
    }
}

/// The speech engine's state, shown in Settings and on pending voice messages.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Engine {
    /// `idle`, `downloading`, `loading`, `ready`, `error` or `unsupported`.
    pub status: String,
    /// Model download progress, 0–100.
    pub progress: Option<u32>,
    pub error: Option<String>,
    /// The [`SpeechModel::id`] this status describes: the selected model.
    pub model: String,
    /// Whether the selected model's file is already on this computer.
    pub downloaded: bool,
    /// Every model of the catalog, for the model picker.
    pub models: Vec<ModelState>,
}
impl Default for Engine {
    fn default() -> Self {
        Self {
            status: if cfg!(feature = "transcription") {
                "idle".into()
            } else {
                "unsupported".into()
            },
            progress: None,
            error: None,
            model: String::new(),
            downloaded: false,
            models: Vec::new(),
        }
    }
}

/// One catalog model's file on this computer.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelState {
    /// A [`SpeechModel::id`].
    pub id: String,
    pub downloaded: bool,
    /// Download progress (0–100) while it downloads.
    pub progress: Option<u32>,
    /// It becomes the selected model once downloaded.
    pub pending: bool,
    /// Why its last download failed.
    pub error: Option<String>,
}

/// Model downloads in progress, and the last failure of each model.
#[derive(Default)]
struct Downloads {
    active: HashMap<&'static str, Download>,
    failed: HashMap<&'static str, String>,
}
struct Download {
    progress: u32,
    cancel: Arc<AtomicBool>,
    /// Chosen in Settings: selected once it is ready.
    select: bool,
}

/// A message as stored in `brazap_messages` (not an alias of it).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Key {
    chat: String,
    sender: String,
    id: String,
}

#[derive(Default)]
struct Queue {
    visible: VecDeque<Key>,
    background: VecDeque<Key>,
    running: Option<Key>,
    failures: HashMap<Key, (Instant, String)>,
    /// Finished messages to transcribe again; their stored result stays until
    /// the new one replaces it.
    redo: HashSet<Key>,
}

impl Queue {
    fn pop(&mut self) -> Option<Key> {
        let next = self
            .visible
            .pop_front()
            .or_else(|| self.background.pop_front())?;
        self.background.retain(|key| key != &next);
        self.running = Some(next.clone());
        Some(next)
    }
    fn status(&mut self, key: &Key) -> Option<Transcript> {
        if let Some((at, error)) = self.failures.get(key) {
            if at.elapsed() < RETRY_AFTER {
                return Some(Transcript::failed(error.clone()));
            }
            self.failures.remove(key);
        }
        if self.running.as_ref() == Some(key) {
            Some(Transcript::pending("running"))
        } else if self.visible.contains(key) || self.background.contains(key) {
            Some(Transcript::pending("queued"))
        } else {
            None
        }
    }
}

#[derive(Default)]
pub(crate) struct Transcriber {
    queue: std::sync::Mutex<Queue>,
    wake: tokio::sync::Notify,
    finished: tokio::sync::Notify,
    engine: std::sync::Mutex<Engine>,
    downloads: std::sync::Mutex<Downloads>,
}

impl Transcriber {
    pub(crate) fn engine(&self) -> Engine {
        self.engine
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
    fn queue(&self) -> std::sync::MutexGuard<'_, Queue> {
        self.queue.lock().unwrap_or_else(|e| e.into_inner())
    }
    fn downloads(&self) -> std::sync::MutexGuard<'_, Downloads> {
        self.downloads.lock().unwrap_or_else(|e| e.into_inner())
    }
    fn set_engine(&self, service: &Service, engine: Engine) {
        let mut current = self.engine.lock().unwrap_or_else(|e| e.into_inner());
        if *current != engine {
            *current = engine;
            service.changed();
        }
    }
}

#[derive(diesel::QueryableByName)]
struct TranscriptRow {
    #[diesel(sql_type = Text)]
    status: String,
    #[diesel(sql_type = Text)]
    text: String,
    #[diesel(sql_type = Text)]
    language: String,
    #[diesel(sql_type = BigInt)]
    partial: i64,
    #[diesel(sql_type = diesel::sql_types::Nullable<Text>)]
    error: Option<String>,
    #[diesel(sql_type = Text)]
    model: String,
}

impl Archive {
    async fn transcript(&self, key: &Key) -> Result<Option<Transcript>> {
        let (account, chat, sender, id) = (
            self.account as i64,
            key.chat.clone(),
            key.sender.clone(),
            key.id.clone(),
        );
        let row: Option<TranscriptRow> = self.sql().read(move |conn| {
            diesel::sql_query("SELECT status,text,language,partial,error,model FROM brazap_transcripts WHERE account=? AND chat=? AND sender=? AND id=?")
                .bind::<BigInt, _>(account).bind::<Text, _>(chat).bind::<Text, _>(sender).bind::<Text, _>(id)
                .get_result(conn).optional().map_err(db_error)
        }).await?;
        Ok(row.map(|row| Transcript {
            status: row.status,
            text: row.text,
            language: row.language,
            error: row.error,
            partial: row.partial != 0,
            model: row.model,
        }))
    }

    /// Stores a result, replacing an earlier one (a re-transcription) and its
    /// text in the message's search entry.
    async fn save_transcript(&self, key: &Key, transcript: &Transcript) -> Result<()> {
        let (account, chat, sender, id, value) = (
            self.account as i64,
            key.chat.clone(),
            key.sender.clone(),
            key.id.clone(),
            transcript.clone(),
        );
        let search = searchable(&value);
        self.sql().run(move |conn| {
            use diesel::Connection;
            #[derive(diesel::QueryableByName)]
            struct Previous {
                #[diesel(sql_type = Text)]
                search: String,
            }
            conn.transaction::<_, diesel::result::Error, _>(|conn| {
                let previous: Option<Previous> = diesel::sql_query("SELECT search FROM brazap_transcripts WHERE account=? AND chat=? AND sender=? AND id=?")
                    .bind::<BigInt, _>(account).bind::<Text, _>(&chat).bind::<Text, _>(&sender).bind::<Text, _>(&id)
                    .get_result(conn).optional()?;
                diesel::sql_query("INSERT INTO brazap_transcripts(account,chat,sender,id,status,text,language,partial,error,model,created_at,search) VALUES(?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(account,chat,sender,id) DO UPDATE SET status=excluded.status,text=excluded.text,language=excluded.language,partial=excluded.partial,error=excluded.error,model=excluded.model,created_at=excluded.created_at,search=excluded.search")
                    .bind::<BigInt, _>(account).bind::<Text, _>(&chat).bind::<Text, _>(&sender).bind::<Text, _>(&id)
                    .bind::<Text, _>(value.status).bind::<Text, _>(value.text).bind::<Text, _>(value.language)
                    .bind::<BigInt, _>(value.partial as i64).bind::<diesel::sql_types::Nullable<Text>, _>(value.error)
                    .bind::<Text, _>(value.model).bind::<BigInt, _>(chrono::Utc::now().timestamp_millis())
                    .bind::<Text, _>(&search)
                    .execute(conn)?;
                let previous = previous.map(|p| p.search).unwrap_or_default();
                index_transcript(conn, account, &chat, &sender, &id, &previous, &search)
            }).map_err(db_error)
        }).await?;
        Ok(())
    }

    async fn forget_failed_transcript(&self, key: &Key) -> Result<()> {
        let (account, chat, sender, id) = (
            self.account as i64,
            key.chat.clone(),
            key.sender.clone(),
            key.id.clone(),
        );
        self.sql().run(move |conn| {
            diesel::sql_query("DELETE FROM brazap_transcripts WHERE account=? AND chat=? AND sender=? AND id=? AND status='failed'")
                .bind::<BigInt, _>(account).bind::<Text, _>(chat).bind::<Text, _>(sender).bind::<Text, _>(id)
                .execute(conn).map_err(db_error)?;
            Ok(())
        }).await?;
        Ok(())
    }
}

/// Lowercase like `storage::searchable`, which the search query is compared with.
fn searchable(transcript: &Transcript) -> String {
    if transcript.status == "done" {
        transcript.text.trim().to_lowercase()
    } else {
        String::new()
    }
}

/// Replaces a message's transcript in its search text. The transcript is always
/// the text's suffix (`' ' || search`), both here and when `storage::index_message`
/// rebuilds the row, so the `previous` one is removed before `search` is
/// appended. The FTS triggers on `brazap_message_search` update the trigram index.
fn index_transcript(
    conn: &mut diesel::SqliteConnection,
    account: i64,
    chat: &str,
    sender: &str,
    id: &str,
    previous: &str,
    search: &str,
) -> diesel::QueryResult<()> {
    if previous == search {
        // Appends a transcript the row does not carry yet (profiles indexed
        // before the `search` column existed); otherwise nothing changes.
        if !search.is_empty() {
            diesel::sql_query("UPDATE brazap_message_search SET text=text || ' ' || ? WHERE account=? AND chat=? AND sender=? AND id=? AND substr(text, -length(?)-1)!=' ' || ?")
                .bind::<Text, _>(search).bind::<BigInt, _>(account).bind::<Text, _>(chat)
                .bind::<Text, _>(sender).bind::<Text, _>(id).bind::<Text, _>(search).bind::<Text, _>(search)
                .execute(conn)?;
        }
        return Ok(());
    }
    if !previous.is_empty() {
        diesel::sql_query("UPDATE brazap_message_search SET text=substr(text, 1, length(text)-length(?)-1) WHERE account=? AND chat=? AND sender=? AND id=? AND substr(text, -length(?)-1)=' ' || ?")
            .bind::<Text, _>(previous).bind::<BigInt, _>(account).bind::<Text, _>(chat)
            .bind::<Text, _>(sender).bind::<Text, _>(id).bind::<Text, _>(previous).bind::<Text, _>(previous)
            .execute(conn)?;
    }
    if !search.is_empty() {
        diesel::sql_query("UPDATE brazap_message_search SET text=text || ' ' || ? WHERE account=? AND chat=? AND sender=? AND id=?")
            .bind::<Text, _>(search).bind::<BigInt, _>(account).bind::<Text, _>(chat)
            .bind::<Text, _>(sender).bind::<Text, _>(id)
            .execute(conn)?;
    }
    Ok(())
}

#[derive(diesel::QueryableByName)]
struct UnindexedRow {
    #[diesel(sql_type = BigInt)]
    account: i64,
    #[diesel(sql_type = Text)]
    chat: String,
    #[diesel(sql_type = Text)]
    sender: String,
    #[diesel(sql_type = Text)]
    id: String,
    #[diesel(sql_type = Text)]
    text: String,
}

/// Runs on every start after `021_transcripts.sql`. Development profiles created
/// that table before it had a `search` column; add it and index their transcripts.
pub(crate) fn upgrade_schema(
    conn: &mut diesel::SqliteConnection,
) -> Result<(), whatsapp_rust::store::error::StoreError> {
    #[derive(diesel::QueryableByName)]
    struct Count {
        #[diesel(sql_type = BigInt)]
        count: i64,
    }
    let has_search: Count = diesel::sql_query(
        "SELECT COUNT(*) AS count FROM pragma_table_info('brazap_transcripts') WHERE name='search'",
    )
    .get_result(conn)
    .map_err(db_error)?;
    if has_search.count > 0 {
        return Ok(());
    }
    use diesel::Connection;
    conn.transaction::<_, diesel::result::Error, _>(|conn| {
        diesel::sql_query(
            "ALTER TABLE brazap_transcripts ADD COLUMN search TEXT NOT NULL DEFAULT ''",
        )
        .execute(conn)?;
        let rows: Vec<UnindexedRow> = diesel::sql_query(
            "SELECT account,chat,sender,id,text FROM brazap_transcripts WHERE status='done' AND text!=''",
        )
        .load(conn)?;
        for row in rows {
            let search = row.text.trim().to_lowercase();
            diesel::sql_query("UPDATE brazap_transcripts SET search=? WHERE account=? AND chat=? AND sender=? AND id=?")
                .bind::<Text, _>(&search).bind::<BigInt, _>(row.account).bind::<Text, _>(&row.chat)
                .bind::<Text, _>(&row.sender).bind::<Text, _>(&row.id)
                .execute(conn)?;
            index_transcript(
                conn,
                row.account,
                &row.chat,
                &row.sender,
                &row.id,
                &search,
                &search,
            )?;
        }
        Ok(())
    })
    .map_err(db_error)
}

/// One requested voice message and its current transcript.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Item {
    pub sender: String,
    pub id: String,
    #[serde(default)]
    pub transcript: Option<Transcript>,
}

impl Service {
    async fn transcript_key(&self, chat: &str, sender: &str, id: &str) -> Result<Key> {
        let source = self.archive.message_source(chat, sender, id).await?;
        Ok(Key {
            chat: source.chat,
            sender: source.sender,
            id: id.to_owned(),
        })
    }

    /// Returns the transcripts of these voice messages in `chat`, queuing the
    /// ones not transcribed yet. `visible` requests replace earlier visible ones,
    /// so scrolling or switching chats moves the newest view to the front.
    /// Messages also listed in `retry` forget earlier failures first; with
    /// `redo` they are transcribed again even when finished, with the selected
    /// model, and the new result replaces the stored one.
    pub async fn transcribe(
        &self,
        chat: String,
        items: Vec<(String, String)>,
        visible: bool,
        retry: &[(String, String)],
        redo: bool,
    ) -> Result<Vec<Item>> {
        self.archive.allow_chat(&chat).await?;
        let enabled = self.settings.read().await.transcription.enabled;
        let mut results = Vec::new();
        let mut wanted = Vec::new();
        for (sender, id) in items.into_iter().take(100) {
            let Ok(key) = self.transcript_key(&chat, &sender, &id).await else {
                results.push(Item {
                    sender,
                    id,
                    transcript: Some(Transcript::failed(
                        "Could not transcribe this voice message",
                    )),
                });
                continue;
            };
            if retry.iter().any(|(s, i)| *s == sender && *i == id) {
                if redo && enabled {
                    self.transcriber.queue().redo.insert(key.clone());
                } else if !redo {
                    self.archive.forget_failed_transcript(&key).await?;
                }
                self.transcriber.queue().failures.remove(&key);
            }
            // Work in progress (or a recent failure) describes a message being
            // transcribed again better than its stored result.
            let (mut transcript, forced) = {
                let mut queue = self.transcriber.queue();
                (queue.status(&key), enabled && queue.redo.contains(&key))
            };
            if transcript.is_none() && !forced {
                transcript = self.archive.transcript(&key).await?;
            }
            if transcript.is_none() && enabled {
                wanted.push(key);
                transcript = Some(Transcript::pending("queued"));
            } else if visible && transcript.as_ref().is_some_and(|t| t.status == "queued") {
                // Polling keeps queued messages in the visible queue.
                wanted.push(key);
            }
            results.push(Item {
                sender,
                id,
                transcript,
            });
        }
        if enabled {
            let mut queue = self.transcriber.queue();
            if visible {
                queue.visible = wanted.into_iter().collect();
            } else {
                for key in wanted {
                    if queue.background.len() >= BACKGROUND_LIMIT {
                        queue.background.pop_front();
                    }
                    if !queue.background.contains(&key) && !queue.visible.contains(&key) {
                        queue.background.push_back(key);
                    }
                }
            }
            drop(queue);
            self.transcriber.wake.notify_one();
        }
        Ok(results)
    }

    /// Waits until one voice message is transcribed, for MCP clients. With
    /// `redo`, a finished transcript is replaced by a fresh one.
    pub async fn transcript_of(
        &self,
        chat: String,
        sender: String,
        id: String,
        wait: Duration,
        redo: bool,
    ) -> Result<Transcript> {
        if let Ok(raw) = self
            .archive
            .raw_message(chat.clone(), sender.clone(), id.clone())
            .await
        {
            ensure!(
                !whatsapp_rust::prelude::MessageExt::is_view_once(&raw),
                crate::content::VIEW_ONCE_LOCKED
            );
        }
        let deadline = tokio::time::Instant::now() + wait;
        let mut first = true;
        let target = vec![(sender.clone(), id.clone())];
        loop {
            let finished = self.transcriber.finished.notified();
            let retry = if first { target.as_slice() } else { &[] };
            let item = self
                .transcribe(chat.clone(), target.clone(), false, retry, redo && first)
                .await?
                .pop()
                .context("No voice message with this id in the chat")?;
            first = false;
            let transcript = item.transcript.unwrap_or_else(|| {
                Transcript::failed("Voice transcription is turned off in Settings")
            });
            if transcript.finished() || transcript.error.is_some() {
                return Ok(transcript);
            }
            if tokio::time::timeout_at(deadline, finished).await.is_err() {
                return Ok(transcript);
            }
        }
    }

    /// Queues a fresh incoming voice message (not offline or history replays).
    pub(crate) fn queue_fresh_voice(self: &Arc<Self>, message: &Message) {
        if message.kind != "audio"
            || message.from_me
            || message.chat == crate::stories::CHAT
            || message.timestamp < crate::model::now() - 120
        {
            return;
        }
        let service = self.clone();
        let message = message.clone();
        tokio::spawn(async move {
            if !service.settings.read().await.transcription.enabled {
                return;
            }
            // Let the automatic attachment download finish instead of fetching twice.
            let media = format!("{}:{}:{}", message.chat, message.sender, message.id);
            for _ in 0..120 {
                if !service.media_job_running(&media) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            if let Err(error) = service
                .transcribe(
                    message.chat.clone(),
                    vec![(message.sender.clone(), message.id.clone())],
                    false,
                    &[],
                    false,
                )
                .await
            {
                log::debug!("Voice message not queued for transcription: {error:#}");
            }
        });
    }

    pub(crate) fn spawn_transcriber(self: &Arc<Self>) {
        let service = Arc::downgrade(self);
        let transcriber = self.transcriber.clone();
        let cache = self.config.cache_dir();
        tokio::spawn(async move {
            // Downloads can't resume, and set-aside files are no longer mapped.
            {
                let cache = cache.clone();
                let selected = match service.upgrade() {
                    Some(service) => {
                        SpeechModel::find(&service.settings.read().await.transcription.model)
                    }
                    None => return,
                };
                let _ = tokio::task::spawn_blocking(move || {
                    files::remove_partials(&cache);
                    if selected.downloaded(&cache) {
                        files::prune(&cache, &[selected.id]);
                    } else {
                        files::sweep(&cache);
                    }
                })
                .await;
            }
            let mut model: Option<Loaded> = None;
            let mut idle_since = Instant::now();
            loop {
                // A model replaced in Settings is released right away.
                if let Some(loaded) = &model
                    && let Some(service) = service.upgrade()
                {
                    let selected = service.settings.read().await.transcription.model.clone();
                    if SpeechModel::find(&selected) != loaded.0 {
                        model = None;
                        transcriber.set_engine(&service, Engine::default());
                        files::sweep(&cache);
                    }
                }
                let next = transcriber.queue().pop();
                let Some(key) = next else {
                    let unload = model.is_some();
                    tokio::select! {
                        _ = transcriber.wake.notified() => {}
                        _ = tokio::time::sleep(UNLOAD_AFTER.saturating_sub(idle_since.elapsed())), if unload => {
                            model = None;
                            files::sweep(&cache);
                            if let Some(service) = service.upgrade() {
                                transcriber.set_engine(&service, Engine::default());
                            }
                        }
                    }
                    continue;
                };
                let Some(service) = service.upgrade() else {
                    break;
                };
                if service.stopping.load(Ordering::Relaxed) {
                    break;
                }
                let outcome = service.transcribe_one(&key, &mut model).await;
                match outcome {
                    Ok(Some(transcript)) => {
                        if let Err(error) = service.archive.save_transcript(&key, &transcript).await
                        {
                            log::warn!("Could not store a transcript: {error:#}");
                        }
                    }
                    Ok(None) => {}
                    Err(error) => {
                        log::debug!("Voice transcription deferred: {error:#}");
                        // The outermost context is a short, translatable sentence.
                        transcriber
                            .queue()
                            .failures
                            .insert(key.clone(), (Instant::now(), error.to_string()));
                    }
                }
                // Cleared after the result is stored, so a poll never sees neither.
                {
                    let mut queue = transcriber.queue();
                    queue.running = None;
                    queue.redo.remove(&key);
                }
                idle_since = Instant::now();
                service.changed();
                transcriber.finished.notify_waiters();
                drop(service);
            }
        });
    }

    /// `Ok(Some)` is a result to store; `Ok(None)` skips the message; `Err` is
    /// a temporary failure (no connection, model download) worth retrying later.
    async fn transcribe_one(
        self: &Arc<Self>,
        key: &Key,
        model: &mut Option<Loaded>,
    ) -> Result<Option<Transcript>> {
        // A model chosen in Settings becomes the selected one once downloaded.
        while self
            .transcriber
            .downloads()
            .active
            .values()
            .any(|d| d.select)
        {
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        let settings = self.settings.read().await.transcription.clone();
        let redo = self.transcriber.queue().redo.contains(key);
        if !settings.enabled || (!redo && self.archive.transcript(key).await?.is_some()) {
            return Ok(None);
        }
        let selected = SpeechModel::find(&settings.model);
        // Locked chats are refused here as well as when queued.
        let Ok(proto) = self
            .archive
            .raw_message(key.chat.clone(), key.sender.clone(), key.id.clone())
            .await
        else {
            return Ok(None);
        };
        let view = project_message(
            key.chat.clone(),
            key.sender.clone(),
            key.id.clone(),
            false,
            0,
            &proto,
            String::new(),
            "{}".into(),
        )
        .view;
        if view.kind != "audio" {
            return Ok(None);
        }
        let bytes = self
            .download_media(&view, proto)
            .await
            .context("Could not download the voice message")?;
        let audio = match tokio::task::spawn_blocking(move || engine::decode(&bytes)).await? {
            Ok(audio) => audio,
            Err(error) => {
                log::debug!("Voice message could not be decoded for transcription: {error:#}");
                return Ok(Some(Transcript {
                    model: selected.name.into(),
                    ..Transcript::failed("This audio format can't be transcribed")
                }));
            }
        };
        let model = match model {
            Some((id, loaded)) if *id == selected => loaded.clone(),
            _ => {
                // Release the previous model before loading another one.
                *model = None;
                let loaded = self.load_model(selected).await?;
                *model = Some((selected, loaded.clone()));
                loaded
            }
        };
        let language = self.settings.read().await.appearance.language.clone();
        let preferred = language.split('-').next().unwrap_or("en").to_owned();
        let partial = audio.partial;
        let (text, language) = tokio::task::spawn_blocking(move || {
            // The samples are freed with this closure, before the result is stored.
            model.transcribe(&audio.samples, &preferred, selected)
        })
        .await?
        .context("Could not transcribe this voice message")?;
        Ok(Some(Transcript {
            status: "done".into(),
            text,
            language,
            error: None,
            partial,
            model: selected.name.into(),
        }))
    }

    async fn load_model(
        self: &Arc<Self>,
        selected: &'static SpeechModel,
    ) -> Result<Arc<engine::Model>> {
        let transcriber = self.transcriber.clone();
        let unsupported = engine::unsupported();
        if let Some(reason) = unsupported {
            transcriber.set_engine(
                self,
                Engine {
                    status: "unsupported".into(),
                    error: Some(reason.into()),
                    ..Default::default()
                },
            );
            // Nothing queued can succeed on this computer.
            let mut queue = transcriber.queue();
            queue.visible.clear();
            queue.background.clear();
            anyhow::bail!(reason);
        }
        let cache = self.config.cache_dir();
        let path = selected.path(&cache);
        let status = |status: &str| Engine {
            status: status.into(),
            model: selected.id.into(),
            ..Default::default()
        };
        if !selected.downloaded(&cache) {
            transcriber.set_engine(self, status("downloading"));
            if let Err(error) = self.download_model(selected).await {
                transcriber.set_engine(
                    self,
                    Engine {
                        error: Some("Could not download the speech model".into()),
                        ..status("error")
                    },
                );
                return Err(error.context("Could not download the speech model"));
            }
        }
        transcriber.set_engine(self, status("loading"));
        let file = path.clone();
        match tokio::task::spawn_blocking(move || engine::Model::load(&file, selected)).await? {
            Ok(model) => {
                transcriber.set_engine(self, status("ready"));
                // Only the model in use stays on disk; one being downloaded
                // keeps its unfinished file.
                let chosen = SpeechModel::find(&self.settings.read().await.transcription.model);
                let _ = tokio::task::spawn_blocking(move || {
                    files::prune(&cache, &[selected.id, chosen.id])
                })
                .await;
                Ok(Arc::new(model))
            }
            Err(error) => {
                // A damaged file downloads again next time.
                if engine::damaged(&error) {
                    let _ = files::delete(selected, &cache);
                }
                let error = error.context("Could not load the speech model");
                log::warn!("{error:#}");
                transcriber.set_engine(
                    self,
                    Engine {
                        error: Some("Could not load the speech model".into()),
                        ..status("error")
                    },
                );
                Err(error)
            }
        }
    }

    /// Downloads `model` (or waits for its download in progress) for the worker.
    async fn download_model(self: &Arc<Self>, model: &'static SpeechModel) -> Result<()> {
        self.start_model_download(model, false);
        while self.transcriber.downloads().active.contains_key(model.id) {
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        if model.downloaded(&self.config.cache_dir()) {
            return Ok(());
        }
        let error = self.transcriber.downloads().failed.get(model.id).cloned();
        Err(anyhow::anyhow!(
            error.unwrap_or_else(|| "The download was cancelled".into())
        ))
    }

    /// Starts downloading `model` unless it is already downloading. With
    /// `select`, it becomes the selected model once it is ready, and a model
    /// chosen earlier that is still downloading is cancelled.
    fn start_model_download(self: &Arc<Self>, model: &'static SpeechModel, select: bool) {
        let cancel = {
            let mut downloads = self.transcriber.downloads();
            downloads.failed.remove(model.id);
            if select {
                for (id, download) in downloads.active.iter_mut() {
                    if *id != model.id && download.select {
                        download.cancel.store(true, Ordering::Relaxed);
                        download.select = false;
                    }
                }
            }
            if let Some(download) = downloads.active.get_mut(model.id) {
                download.select |= select;
                return;
            }
            let cancel = Arc::new(AtomicBool::new(false));
            downloads.active.insert(
                model.id,
                Download {
                    progress: 0,
                    cancel: cancel.clone(),
                    select,
                },
            );
            cancel
        };
        self.changed();
        let service = self.clone();
        tokio::spawn(async move {
            let cache = service.config.cache_dir();
            let transcriber = service.transcriber.clone();
            let revision = service.revision_handle();
            let progress = transcriber.clone();
            let outcome = tokio::task::spawn_blocking(move || {
                files::download(model, &cache, &cancel, |percent| {
                    if let Some(download) = progress.downloads().active.get_mut(model.id)
                        && download.progress != percent
                    {
                        download.progress = percent;
                        revision.fetch_add(1, Ordering::Relaxed);
                    }
                })
            })
            .await
            .map_err(anyhow::Error::from)
            .and_then(|result| result);
            let select = {
                let mut downloads = transcriber.downloads();
                let entry = downloads.active.remove(model.id);
                match &outcome {
                    Ok(()) => {}
                    Err(error) if error.is::<files::Cancelled>() => {}
                    Err(error) => {
                        log::warn!(
                            "Could not download the speech model {}: {error:#}",
                            model.id
                        );
                        downloads
                            .failed
                            .insert(model.id, "Could not download the speech model".into());
                    }
                }
                entry.is_some_and(|entry| entry.select)
            };
            if outcome.is_ok() {
                log::info!("Downloaded the speech model {}", model.id);
                if select && let Err(error) = service.select_speech_model(model).await {
                    log::warn!("Could not select the speech model {}: {error:#}", model.id);
                }
            }
            service.changed();
            transcriber.wake.notify_one();
        });
    }

    /// Makes a downloaded model the selected one and removes the others.
    async fn select_speech_model(&self, model: &'static SpeechModel) -> Result<()> {
        let mut settings = self.settings.read().await.clone();
        if settings.transcription.model != model.id {
            settings.transcription.model = model.id.into();
            self.save_settings(settings).await?;
        }
        let cache = self.config.cache_dir();
        tokio::task::spawn_blocking(move || files::prune(&cache, &[model.id])).await?;
        // The worker releases the previous model.
        self.transcriber.wake.notify_one();
        Ok(())
    }

    /// The model picker's actions: `choose` selects a downloaded model or
    /// downloads one and selects it when ready, `cancel` stops a download, and
    /// `delete` removes a downloaded model that is not in use.
    pub async fn speech_model(self: &Arc<Self>, id: &str, action: &str) -> Result<()> {
        let model = SpeechModel::get(id).context("Unknown speech model")?;
        let cache = self.config.cache_dir();
        match action {
            "choose" => {
                if model.downloaded(&cache) {
                    // A model chosen earlier that is still downloading is dropped.
                    for download in self.transcriber.downloads().active.values_mut() {
                        if download.select {
                            download.cancel.store(true, Ordering::Relaxed);
                            download.select = false;
                        }
                    }
                    self.select_speech_model(model).await?;
                } else {
                    self.start_model_download(model, true);
                }
            }
            "cancel" => {
                if let Some(download) = self.transcriber.downloads().active.get(model.id) {
                    download.cancel.store(true, Ordering::Relaxed);
                }
            }
            "delete" => {
                anyhow::ensure!(
                    self.settings.read().await.transcription.model != model.id,
                    "The speech model in use can't be deleted"
                );
                if let Some(download) = self.transcriber.downloads().active.get(model.id) {
                    download.cancel.store(true, Ordering::Relaxed);
                }
                tokio::task::spawn_blocking(move || files::delete(model, &cache))
                    .await?
                    .context("Could not delete the speech model")?;
            }
            _ => anyhow::bail!("Unknown speech model action"),
        }
        self.changed();
        Ok(())
    }

    /// The engine's state for the model selected in Settings: work on a
    /// model that was replaced meanwhile is reported as idle.
    pub(crate) async fn transcription_engine(&self) -> Engine {
        let mut engine = self.transcriber.engine();
        let selected = SpeechModel::find(&self.settings.read().await.transcription.model);
        if engine.status != "unsupported" && engine.model != selected.id {
            engine = Engine::default();
        }
        let cache = self.config.cache_dir();
        let downloads = self.transcriber.downloads();
        engine.model = selected.id.into();
        engine.downloaded = selected.downloaded(&cache);
        if engine.status != "unsupported"
            && let Some(download) = downloads.active.get(selected.id)
        {
            engine.status = "downloading".into();
            engine.progress = Some(download.progress);
        }
        engine.models = MODELS
            .iter()
            .map(|model| {
                let download = downloads.active.get(model.id);
                ModelState {
                    id: model.id.into(),
                    downloaded: model.downloaded(&cache),
                    progress: download.map(|d| d.progress),
                    pending: download.is_some_and(|d| d.select),
                    error: downloads.failed.get(model.id).cloned(),
                }
            })
            .collect();
        engine
    }

    /// Drops queued work when transcription is turned off, and lets the worker
    /// release a model replaced in Settings.
    pub(crate) fn transcription_toggled(&self, enabled: bool) {
        if !enabled {
            let mut queue = self.transcriber.queue();
            queue.visible.clear();
            queue.background.clear();
            queue.redo.clear();
        }
        self.transcriber.wake.notify_one();
    }
}

/// The model the worker has loaded, and which one it is.
type Loaded = (&'static SpeechModel, Arc<engine::Model>);

/// Builds without the `transcription` feature report the engine unsupported.
#[cfg(not(feature = "transcription"))]
mod engine {
    use anyhow::{Result, bail};

    use super::SpeechModel;

    pub(super) struct Audio {
        pub samples: Vec<f32>,
        pub partial: bool,
    }
    pub(super) struct Model;
    pub(super) fn unsupported() -> Option<&'static str> {
        Some("This build of Brazap doesn't include voice transcription")
    }
    pub(super) fn decode(_: &[u8]) -> Result<Audio> {
        bail!("Voice transcription is not included in this build")
    }
    pub(super) fn damaged(_: &anyhow::Error) -> bool {
        false
    }
    impl Model {
        pub fn load(_: &std::path::Path, _: &SpeechModel) -> Result<Self> {
            bail!("Voice transcription is not included in this build")
        }
        pub fn transcribe(&self, _: &[f32], _: &str, _: &SpeechModel) -> Result<(String, String)> {
            bail!("Voice transcription is not included in this build")
        }
    }
}
