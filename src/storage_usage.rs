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

impl Archive {
    pub async fn storage_usage(&self, config: &Config) -> Result<Value> {
        let account = self.account as i64;
        let items=self.backend.shared().read(move|conn| {
            diesel::sql_query("SELECT 'messages' kind, COUNT(*) count, COALESCE(SUM(length(encoded)+length(payload)+length(metadata)),0) bytes FROM brazap_messages WHERE account=? UNION ALL SELECT COALESCE(json_extract(m.payload,'$.kind'),'file') kind, COUNT(*) count, COALESCE(SUM(length(b.data)),0) bytes FROM brazap_media b LEFT JOIN brazap_messages m ON m.account=b.account AND m.chat=b.chat AND m.sender=b.sender AND m.id=b.id WHERE b.account=? GROUP BY kind UNION ALL SELECT 'cache' kind,COUNT(*) count,COALESCE(SUM(length(data)),0) bytes FROM brazap_assets WHERE account=?")
                .bind::<BigInt,_>(account).bind::<BigInt,_>(account).bind::<BigInt,_>(account).load::<Usage>(conn).map_err(db_error)
        }).await?;
        let file_size = |p: &std::path::Path| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
        let db = config.database();
        Ok(
            json!({"items":items.into_iter().map(|u|json!({"kind":u.kind,"count":u.count,"bytes":u.bytes})).collect::<Vec<_>>(),
            "database_bytes":file_size(&db),"wal_bytes":file_size(&db.with_extension("db-wal")),
            "data_dir":config.data_dir.to_string_lossy(),"logs_dir":config.logs_dir().to_string_lossy(),"cache_dir":config.cache_dir().to_string_lossy()}),
        )
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
