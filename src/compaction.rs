//! Lossless compaction of the archived message envelope.
//!
//! `brazap_messages.encoded` is the authoritative raw message. `metadata` is the
//! JSON of the surrounding WhatsApp record, which used to include every unset
//! field as `null` and a second copy of the message body. Neither carries
//! information, so they are removed; every other envelope field is kept.
//!
//! The removal is reversible for future features that reprocess raw data:
//! [`rehydrate_metadata`] rebuilds the full record from `metadata` + `encoded`.
use crate::storage::{Archive, db_error};
use anyhow::Result;
use diesel::{
    prelude::*,
    sql_types::{BigInt, Binary, Text},
};
use serde_json::Value;
use whatsapp_rust::{prelude::wa, waproto::buffa::Message as ProtoMessage};

const BATCH: i64 = 200;
const MARKER: &str = "metadata_compaction_v1";

/// Remove object members whose value is `null` (an unset protobuf field).
/// Nulls inside arrays are positional and therefore kept.
fn drop_nulls(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.retain(|_, v| !v.is_null());
            map.values_mut().for_each(drop_nulls);
        }
        Value::Array(items) => items.iter_mut().for_each(drop_nulls),
        _ => {}
    }
}

/// Compact one record's metadata. `message` must be the decoded `encoded` bytes
/// stored with it. The embedded copy of the message is removed only when it is
/// exactly equal to what `message` serialises to; otherwise it is kept (minus
/// nulls), so the result never loses information.
pub(crate) fn compact_metadata(metadata: &str, message: &wa::Message) -> (String, bool) {
    let Ok(mut value) = serde_json::from_str::<Value>(metadata) else {
        return (metadata.to_owned(), false);
    };
    if !value.is_object() {
        return (metadata.to_owned(), false);
    }
    drop_nulls(&mut value);
    let mut removed = false;
    if let Some(embedded) = value.get("message")
        && let Ok(mut rebuilt) = serde_json::to_value(message)
    {
        drop_nulls(&mut rebuilt);
        if *embedded == rebuilt {
            if let Some(object) = value.as_object_mut() {
                object.remove("message");
            }
            removed = true;
        }
    }
    match serde_json::to_string(&value) {
        Ok(compact) => (compact, removed),
        Err(_) => (metadata.to_owned(), false),
    }
}

/// Rebuild the full WhatsApp record for a stored message: its compact envelope
/// plus the message body decoded from the raw `encoded` bytes. Unset (`null`)
/// fields stay omitted. Use this instead of reading `metadata` alone whenever a
/// feature needs fields from the message body.
pub fn rehydrate_metadata(metadata: &str, encoded: &[u8]) -> Result<Value> {
    let mut value: Value = serde_json::from_str(metadata)?;
    if let Some(object) = value.as_object_mut()
        && !object.contains_key("message")
    {
        let mut message = serde_json::to_value(wa::Message::decode_from_slice(encoded)?)?;
        drop_nulls(&mut message);
        object.insert("message".into(), message);
    }
    Ok(value)
}

#[derive(QueryableByName)]
struct Row {
    #[diesel(sql_type = BigInt)]
    cursor: i64,
    #[diesel(sql_type = Binary)]
    encoded: Vec<u8>,
    #[diesel(sql_type = Text)]
    metadata: String,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct CompactionReport {
    pub rows: u64,
    pub rewritten: u64,
    pub body_kept: u64,
    pub bytes_before: u64,
    pub bytes_after: u64,
}

impl Archive {
    /// Compact existing rows in small, resumable batches. Messages are
    /// never deleted and `encoded` is never touched.
    pub async fn compact_stored_metadata(&self) -> Result<CompactionReport> {
        let mut report = CompactionReport::default();
        if self.preference(MARKER.into()).await?.is_some() {
            return Ok(report);
        }
        let account = self.account as i64;
        let mut cursor: i64 = self
            .preference(format!("{MARKER}_cursor"))
            .await?
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        loop {
            let rows: Vec<Row> = self
                .backend
                .shared()
                .read(move |conn| {
                    diesel::sql_query("SELECT rowid AS cursor,encoded,metadata FROM brazap_messages WHERE account=? AND rowid>? ORDER BY rowid LIMIT ?")
                        .bind::<BigInt, _>(account)
                        .bind::<BigInt, _>(cursor)
                        .bind::<BigInt, _>(BATCH)
                        .load(conn)
                        .map_err(db_error)
                })
                .await?;
            let Some(last) = rows.last().map(|row| row.cursor) else {
                break;
            };
            let (updates, batch) = tokio::task::spawn_blocking(move || {
                let mut batch = CompactionReport::default();
                let mut updates = Vec::new();
                for row in rows {
                    batch.rows += 1;
                    batch.bytes_before += row.metadata.len() as u64;
                    let compact = wa::Message::decode_from_slice(&row.encoded)
                        .ok()
                        .map(|message| compact_metadata(&row.metadata, &message));
                    match compact {
                        Some((compact, removed)) if compact.len() < row.metadata.len() => {
                            batch.rewritten += 1;
                            batch.body_kept += u64::from(!removed);
                            batch.bytes_after += compact.len() as u64;
                            updates.push((row.cursor, row.metadata, compact));
                        }
                        _ => batch.bytes_after += row.metadata.len() as u64,
                    }
                }
                (updates, batch)
            })
            .await?;
            if !updates.is_empty() {
                self.backend
                    .shared()
                    .run(move |conn| {
                        conn.transaction::<_, diesel::result::Error, _>(|conn| {
                            for (rowid, before, after) in updates {
                                // The old value guards against a concurrent rewrite.
                                diesel::sql_query(
                                    "UPDATE brazap_messages SET metadata=? WHERE rowid=? AND metadata=?",
                                )
                                .bind::<Text, _>(after)
                                .bind::<BigInt, _>(rowid)
                                .bind::<Text, _>(before)
                                .execute(conn)?;
                            }
                            Ok(())
                        })
                        .map_err(db_error)
                    })
                    .await?;
            }
            cursor = last;
            report.rows += batch.rows;
            report.rewritten += batch.rewritten;
            report.body_kept += batch.body_kept;
            report.bytes_before += batch.bytes_before;
            report.bytes_after += batch.bytes_after;
            self.set_preference(format!("{MARKER}_cursor"), cursor.to_string())
                .await?;
            // Leave room for live message storage between batches.
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        self.set_preference(MARKER.into(), "1".into()).await?;
        Ok(report)
    }
}
