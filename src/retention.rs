//! Media retention: remove downloaded attachment bytes that were not opened
//! within the configured time. Messages, their raw protobuf and envelope are
//! never touched, so evicted media can be fetched again from the retained media
//! key while WhatsApp (or the phone, via media retry) still has the file.
use crate::{
    settings::RetentionSettings,
    storage::{Archive, db_error},
};
use anyhow::Result;
use diesel::{
    prelude::*,
    sql_types::{BigInt, Binary, Text},
};
use whatsapp_rust::{download::Downloadable, prelude::wa, waproto::buffa::Message as ProtoMessage};

const DAY_MS: i64 = 24 * 60 * 60 * 1000;
/// `accessed_at` is refreshed at most this often, so reading media stays cheap.
pub(crate) const TOUCH_INTERVAL_MS: i64 = DAY_MS;

#[derive(QueryableByName)]
struct Candidate {
    #[diesel(sql_type = Text)]
    chat: String,
    #[diesel(sql_type = Text)]
    sender: String,
    #[diesel(sql_type = Text)]
    id: String,
    #[diesel(sql_type = BigInt)]
    bytes: i64,
    #[diesel(sql_type = Binary)]
    encoded: Vec<u8>,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct EvictionReport {
    pub items: u64,
    pub bytes: u64,
}

/// Evicting is only safe when the message still carries what a later download
/// needs: the decryption key and a CDN reference.
fn redownloadable(message: &wa::Message) -> bool {
    let base = crate::content::media_message(message);
    let media: Option<&dyn Downloadable> = if let Some(m) = base.image_message.as_option() {
        Some(m)
    } else if let Some(m) = base.video_message.as_option() {
        Some(m)
    } else if let Some(m) = base.document_message.as_option() {
        Some(m)
    } else if let Some(m) = base.audio_message.as_option() {
        Some(m)
    } else if let Some(m) = base.sticker_message.as_option() {
        Some(m)
    } else {
        None
    };
    media.is_some_and(|media| {
        media.media_key().is_some_and(|key| !key.is_empty())
            && (media.direct_path().is_some_and(|path| !path.is_empty())
                || media.static_url().is_some_and(|url| !url.is_empty()))
    })
}

impl Archive {
    pub async fn evict_media(
        &self,
        retention: &RetentionSettings,
        now_ms: i64,
    ) -> Result<EvictionReport> {
        let account = self.account as i64;
        let identities = self.identities().await?;
        let mut report = EvictionReport::default();
        for (kind, days) in retention.by_kind() {
            if days == 0 {
                continue;
            }
            let cutoff = now_ms - i64::from(days) * DAY_MS;
            let candidates: Vec<Candidate> = self
                .backend
                .shared()
                .read(move |conn| {
                    // Unsent uploads have no server copy yet; keep their bytes.
                    diesel::sql_query(
                        "SELECT b.chat AS chat,b.sender AS sender,b.id AS id,length(b.data) AS bytes,m.encoded AS encoded \
                         FROM brazap_media b \
                         JOIN brazap_media_usage u ON u.account=b.account AND u.chat=b.chat AND u.sender=b.sender AND u.id=b.id \
                         JOIN brazap_messages m ON m.account=b.account AND m.chat=b.chat AND m.sender=b.sender AND m.id=b.id \
                         WHERE b.account=? AND json_extract(m.payload,'$.kind')=? AND u.accessed_at<? \
                           AND m.status NOT IN ('sending','failed')",
                    )
                    .bind::<BigInt, _>(account)
                    .bind::<Text, _>(kind)
                    .bind::<BigInt, _>(cutoff)
                    .load(conn)
                    .map_err(db_error)
                })
                .await?;
            let mut evict = Vec::new();
            for candidate in candidates {
                // Locked chats cannot download media, so their bytes are the only copy.
                if identities.is_locked(&candidate.chat) {
                    continue;
                }
                let Ok(message) = wa::Message::decode_from_slice(&candidate.encoded) else {
                    continue;
                };
                if redownloadable(&message) {
                    evict.push((
                        candidate.chat,
                        candidate.sender,
                        candidate.id,
                        candidate.bytes,
                    ));
                }
            }
            if evict.is_empty() {
                continue;
            }
            report.items += evict.len() as u64;
            report.bytes += evict.iter().map(|item| item.3.max(0) as u64).sum::<u64>();
            self.backend
                .shared()
                .run(move |conn| {
                    conn.transaction::<_, diesel::result::Error, _>(|conn| {
                        for (chat, sender, id, _) in evict {
                            for table in ["brazap_media", "brazap_media_usage"] {
                                diesel::sql_query(format!(
                                    "DELETE FROM {table} WHERE account=? AND chat=? AND sender=? AND id=?"
                                ))
                                .bind::<BigInt, _>(account)
                                .bind::<Text, _>(&chat)
                                .bind::<Text, _>(&sender)
                                .bind::<Text, _>(&id)
                                .execute(conn)?;
                            }
                            // The full-size preview is derived from the bytes just removed.
                            diesel::sql_query(
                                "DELETE FROM brazap_assets WHERE account=? AND key LIKE 'image:%' AND substr(key,-length(?)-1)=':'||?",
                            )
                            .bind::<BigInt, _>(account)
                            .bind::<Text, _>(&id)
                            .bind::<Text, _>(&id)
                            .execute(conn)?;
                        }
                        Ok(())
                    })
                    .map_err(db_error)
                })
                .await?;
        }
        self.backend
            .shared()
            .run(move |conn| {
                diesel::sql_query(
                    "DELETE FROM brazap_media_usage WHERE account=? AND NOT EXISTS \
                     (SELECT 1 FROM brazap_media b WHERE b.account=brazap_media_usage.account \
                      AND b.chat=brazap_media_usage.chat AND b.sender=brazap_media_usage.sender \
                      AND b.id=brazap_media_usage.id)",
                )
                .bind::<BigInt, _>(account)
                .execute(conn)
                .map_err(db_error)?;
                Ok(())
            })
            .await?;
        Ok(report)
    }
}

impl crate::service::Service {
    /// Background storage upkeep, kept off the startup path: compact old
    /// message envelopes once, then hourly apply media retention and return
    /// freed pages to the filesystem. Changing a retention setting takes
    /// effect on the next pass, which leaves time to undo a mistaken choice.
    pub(crate) fn spawn_storage_maintenance(self: &std::sync::Arc<Self>) {
        use std::sync::atomic::Ordering;
        let service = self.clone();
        tokio::spawn(async move {
            match service.archive.compact_stored_metadata().await {
                Ok(report) if report.rows > 0 => log::info!(
                    "Compacted message envelopes: {} of {} rows, {} MiB to {} MiB ({} kept their embedded body)",
                    report.rewritten,
                    report.rows,
                    report.bytes_before / (1024 * 1024),
                    report.bytes_after / (1024 * 1024),
                    report.body_kept
                ),
                Ok(_) => {}
                Err(error) => log::warn!("Could not compact message envelopes: {error:#}"),
            }
            tokio::time::sleep(std::time::Duration::from_secs(120)).await;
            while !service.stopping.load(Ordering::Relaxed) {
                let retention = service.settings.read().await.storage.retention.clone();
                match service
                    .archive
                    .evict_media(&retention, chrono::Utc::now().timestamp_millis())
                    .await
                {
                    Ok(report) if report.items > 0 => {
                        log::info!(
                            "Removed {} downloaded attachments ({} MiB) past their retention",
                            report.items,
                            report.bytes / (1024 * 1024)
                        );
                        service.changed();
                    }
                    Ok(_) => {}
                    Err(error) => log::warn!("Could not apply media retention: {error:#}"),
                }
                match service.archive.reclaim_space(&service.config).await {
                    Ok(bytes) if bytes > 0 => {
                        log::info!("Released {} MiB of database space", bytes / (1024 * 1024));
                        service.changed();
                    }
                    Ok(_) => {}
                    Err(error) => log::warn!("Could not release database space: {error:#}"),
                }
                tokio::time::sleep(std::time::Duration::from_secs(60 * 60)).await;
            }
        });
    }
}
