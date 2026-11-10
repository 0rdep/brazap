use crate::{
    config::Config,
    storage::{Archive, db_error},
};
use anyhow::Result;
use diesel::{
    prelude::*,
    sql_types::{BigInt, Text},
};
use serde_json::{Value, json};

#[derive(QueryableByName)]
struct Usage {
    #[diesel(sql_type=Text)]
    kind: String,
    #[diesel(sql_type=BigInt)]
    count: i64,
    #[diesel(sql_type=BigInt)]
    bytes: i64,
}

#[derive(QueryableByName)]
struct PragmaValue {
    #[diesel(sql_type=BigInt)]
    value: i64,
}

impl Archive {
    pub async fn storage_usage(&self, config: &Config) -> Result<Value> {
        let account = self.account as i64;
        let items=self.backend.shared().read(move|conn| {
            diesel::sql_query("SELECT 'messages' kind, COUNT(*) count, COALESCE(SUM(length(encoded)+length(payload)+length(metadata)),0) bytes FROM brazap_messages WHERE account=? UNION ALL SELECT COALESCE(json_extract(m.payload,'$.kind'),'file') kind, COUNT(*) count, COALESCE(SUM(length(b.data)),0) bytes FROM brazap_media b LEFT JOIN brazap_messages m ON m.account=b.account AND m.chat=b.chat AND m.sender=b.sender AND m.id=b.id WHERE b.account=? GROUP BY kind UNION ALL SELECT 'cache' kind,COUNT(*) count,COALESCE(SUM(length(data)),0) bytes FROM brazap_assets WHERE account=? UNION ALL SELECT 'history' kind,COUNT(*) count,COALESCE(SUM(length(data)+length(metadata)),0) bytes FROM brazap_history WHERE account=?")
                .bind::<BigInt,_>(account).bind::<BigInt,_>(account).bind::<BigInt,_>(account).bind::<BigInt,_>(account).load::<Usage>(conn).map_err(db_error)
        }).await?;
        let file_size = |p: &std::path::Path| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
        let db = config.database();
        let reclaimable = match (
            self.pragma("freelist_count").await,
            self.pragma("page_size").await,
        ) {
            (Ok(free), Ok(page)) => free.max(0) * page.max(0),
            _ => 0,
        };
        // Search index, table indexes, page overhead, stickers and the library's
        // own protocol tables are not listed above; show them so the rows add up.
        let database = file_size(&db);
        let listed = items.iter().map(|u| u.bytes.max(0) as u64).sum::<u64>();
        let mut items = items
            .into_iter()
            .map(|u| json!({"kind":u.kind,"count":u.count,"bytes":u.bytes}))
            .collect::<Vec<_>>();
        items.push(json!({"kind":"other","count":0,"bytes":database.saturating_sub(listed).saturating_sub(reclaimable.max(0) as u64)}));
        Ok(json!({"items":items,
            "database_bytes":database,"wal_bytes":file_size(&db.with_extension("db-wal")),"reclaimable_bytes":reclaimable,
            "data_dir":config.data_dir.to_string_lossy(),"logs_dir":config.logs_dir().to_string_lossy(),"cache_dir":config.cache_dir().to_string_lossy()}))
    }
    async fn pragma(&self, name: &'static str) -> Result<i64> {
        Ok(self
            .backend
            .shared()
            .read(move |conn| {
                diesel::sql_query(format!("SELECT {name} AS value FROM pragma_{name}"))
                    .get_result::<PragmaValue>(conn)
                    .map(|row| row.value)
                    .map_err(db_error)
            })
            .await?)
    }

    /// Return free database pages to the filesystem. SQLite never shrinks the
    /// file on its own, so after large deletions (media retention, metadata
    /// compaction) the first call rebuilds the file once and switches it to
    /// incremental auto-vacuum; later calls only trim the free list.
    pub async fn reclaim_space(&self, config: &Config) -> Result<u64> {
        use diesel::connection::SimpleConnection;
        const MIB: u64 = 1024 * 1024;
        let page = self.pragma("page_size").await?.max(0) as u64;
        let free = self.pragma("freelist_count").await?.max(0) as u64;
        let total = self.pragma("page_count").await?.max(0) as u64;
        let (reclaimable, file) = (free * page, total * page);
        if self.pragma("auto_vacuum").await? == 0 {
            if reclaimable < (128 * MIB).max(file / 8) {
                return Ok(0);
            }
            // The rebuild writes a second copy of the live data.
            let available = fs2::available_space(&config.data_dir)?;
            if available < file + 512 * MIB {
                log::warn!(
                    "Not compacting the database: {} MiB free is less than the {} MiB needed",
                    available / MIB,
                    (file + 512 * MIB) / MIB
                );
                return Ok(0);
            }
            log::info!(
                "Compacting the database to release {} MiB",
                reclaimable / MIB
            );
            self.backend
                .shared()
                .run(|conn| {
                    conn.batch_execute("PRAGMA auto_vacuum=INCREMENTAL; VACUUM;")
                        .map_err(db_error)
                })
                .await?;
        } else {
            for _ in 0..4096 {
                if self.pragma("freelist_count").await? == 0 {
                    break;
                }
                self.backend
                    .shared()
                    .run(|conn| {
                        conn.batch_execute("PRAGMA incremental_vacuum(8192)")
                            .map_err(db_error)
                    })
                    .await?;
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }
        self.backend
            .shared()
            .run(|conn| {
                conn.batch_execute("PRAGMA wal_checkpoint(TRUNCATE)")
                    .map_err(db_error)
            })
            .await?;
        Ok(reclaimable)
    }

    pub async fn clear_derived_cache(&self) -> Result<()> {
        let account = self.account as i64;
        self.backend
            .shared()
            .run(move |conn| {
                diesel::sql_query("DELETE FROM brazap_assets WHERE account=?")
                    .bind::<BigInt, _>(account)
                    .execute(conn)
                    .map_err(db_error)?;
                Ok(())
            })
            .await?;
        Ok(())
    }
}
