use std::io::Read;

use anyhow::Result;
use diesel::{
    prelude::*,
    sql_types::{BigInt, Binary, Nullable, Text},
};
use whatsapp_rust::{prelude::wa, waproto::buffa::Message as ProtoMessage};

use crate::{
    model::Call,
    storage::{Archive, display_identity},
};

fn db_error(error: diesel::result::Error) -> whatsapp_rust::store::error::StoreError {
    whatsapp_rust::store::error::StoreError::Database(Box::new(error))
}

#[derive(QueryableByName)]
struct CallRow {
    #[diesel(sql_type=Text)]
    payload: String,
    #[diesel(sql_type=Text)]
    name: String,
    #[diesel(sql_type=Text)]
    phone: String,
    #[diesel(sql_type=BigInt)]
    photo_version: i64,
}

#[derive(QueryableByName)]
struct ValueRow {
    #[diesel(sql_type=Text)]
    value: String,
}

#[derive(QueryableByName)]
struct HistoryRow {
    #[diesel(sql_type=BigInt)]
    cursor: i64,
    #[diesel(sql_type=Binary)]
    data: Vec<u8>,
}

fn canonical(jid: &str) -> String {
    jid.parse::<whatsapp_rust::Jid>()
        .map(|jid| jid.to_non_ad().to_string())
        .unwrap_or_default()
}

impl Archive {
    pub async fn own_jids(&self) -> Result<Vec<String>> {
        use whatsapp_rust::store::traits::DeviceStore;
        Ok(self
            .backend
            .load()
            .await?
            .map(|device| {
                [device.pn, device.lid]
                    .into_iter()
                    .flatten()
                    .map(|jid| jid.to_non_ad().to_string())
                    .collect()
            })
            .unwrap_or_default())
    }

    pub async fn calls(&self) -> Result<Vec<Call>> {
        let account = self.account as i64;
        let rows:Vec<CallRow>=self.backend.shared().read(move |conn| {
            diesel::sql_query("SELECT b.payload,COALESCE(NULLIF(n.name,''),NULLIF(c.name,''),'') AS name,COALESCE(l.phone_number,'') AS phone,COALESCE(a.updated_at,0) AS photo_version FROM brazap_calls b LEFT JOIN brazap_chats c ON c.account=b.account AND c.jid=b.peer LEFT JOIN brazap_contacts n ON n.account=b.account AND n.jid=b.peer LEFT JOIN lid_pn_mapping l ON l.device_id=b.account AND b.peer LIKE '%@lid' AND (l.lid=b.peer OR l.lid=substr(b.peer,1,instr(b.peer,'@')-1)) LEFT JOIN brazap_assets a ON a.account=b.account AND a.key='avatar:'||b.peer WHERE b.account=? ORDER BY b.timestamp DESC,b.id DESC LIMIT 500")
                .bind::<BigInt,_>(account).load(conn).map_err(db_error)
        }).await?;
        let identities = self.identities().await?;
        let calls = rows
            .into_iter()
            .map(|row| {
                let mut call: Call = serde_json::from_str(&row.payload)?;
                call.name = display_identity(&call.peer, &row.name, &row.phone).0;
                call.photo_version = row.photo_version;
                Ok(call)
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(calls
            .into_iter()
            .filter(|call| !identities.is_locked(&call.peer))
            .collect())
    }

    pub async fn store_call(
        &self,
        record: &wa::CallLogRecord,
        from_me: Option<bool>,
        creator: Option<String>,
        own: &[String],
    ) -> Result<()> {
        let creator = creator
            .or_else(|| record.call_creator_jid.clone())
            .unwrap_or_default();
        let creator = canonical(&creator);
        let from_me = from_me
            .or_else(|| (!own.is_empty() && !creator.is_empty()).then(|| own.contains(&creator)));
        let id = record
            .call_id
            .clone()
            .unwrap_or_else(|| blake3::hash(&record.encode_to_vec()).to_hex().to_string());
        let peer = record
            .group_jid
            .clone()
            .filter(|s| !s.is_empty())
            .or_else(|| (from_me == Some(false) && !creator.is_empty()).then(|| creator.clone()))
            .or_else(|| {
                record
                    .participants
                    .iter()
                    .filter_map(|p| p.user_jid.as_ref())
                    .map(|jid| canonical(jid))
                    .find(|jid| !own.contains(jid) && *jid != creator)
            })
            .or_else(|| (from_me != Some(true) && !own.contains(&creator)).then(|| creator.clone()))
            .unwrap_or_default();
        let value = serde_json::to_value(record.call_result)?;
        let outcome = value.as_str().unwrap_or("unknown").to_lowercase();
        let call = Call {
            id: id.clone(),
            peer,
            name: String::new(),
            timestamp: record.start_time,
            from_me,
            video: record.is_video.unwrap_or(false),
            duration: record.duration,
            outcome,
            photo_version: 0,
        };
        let payload = serde_json::to_string(&call)?;
        let bytes = record.encode_to_vec();
        let account = self.account as i64;
        self.backend.shared().run(move |conn| {
            diesel::sql_query("INSERT INTO brazap_calls(account,creator,id,peer,timestamp,payload,record) VALUES(?,?,?,?,?,?,?) ON CONFLICT(account,creator,id) DO UPDATE SET peer=CASE WHEN excluded.peer!='' THEN excluded.peer ELSE brazap_calls.peer END,timestamp=COALESCE(excluded.timestamp,brazap_calls.timestamp),payload=json_set(excluded.payload,'$.peer',COALESCE(NULLIF(excluded.peer,''),brazap_calls.peer),'$.from_me',json(CASE COALESCE(json_extract(excluded.payload,'$.from_me'),json_extract(brazap_calls.payload,'$.from_me')) WHEN 1 THEN 'true' WHEN 0 THEN 'false' ELSE 'null' END)),record=excluded.record")
                .bind::<BigInt,_>(account).bind::<Text,_>(creator).bind::<Text,_>(id).bind::<Text,_>(call.peer)
                .bind::<Nullable<BigInt>,_>(call.timestamp).bind::<Text,_>(payload).bind::<Binary,_>(bytes).execute(conn).map_err(db_error)?;
            Ok(())
        }).await?;
        Ok(())
    }

    pub async fn preference(&self, key: String) -> Result<Option<String>> {
        let account = self.account as i64;
        let row: Option<ValueRow> = self
            .backend
            .shared()
            .read(move |conn| {
                diesel::sql_query("SELECT value FROM brazap_preferences WHERE account=? AND key=?")
                    .bind::<BigInt, _>(account)
                    .bind::<Text, _>(key)
                    .get_result(conn)
                    .optional()
                    .map_err(db_error)
            })
            .await?;
        Ok(row.map(|r| r.value))
    }

    pub async fn set_preference(&self, key: String, value: String) -> Result<()> {
        let account = self.account as i64;
        self.backend.shared().run(move |conn| {
            diesel::sql_query("INSERT INTO brazap_preferences(account,key,value) VALUES(?,?,?) ON CONFLICT(account,key) DO UPDATE SET value=excluded.value")
                .bind::<BigInt,_>(account).bind::<Text,_>(key).bind::<Text,_>(value).execute(conn).map_err(db_error)?;
            Ok(())
        }).await?;
        Ok(())
    }

    pub async fn backfill_calls(&self) -> Result<()> {
        if self.preference("calls_backfilled".into()).await?.is_some() {
            return Ok(());
        }
        let own = self.own_jids().await?;
        let mut cursor = 0i64;
        loop {
            let account = self.account as i64;
            let row:Option<HistoryRow>=self.backend.shared().read(move |conn| {
                diesel::sql_query("SELECT rowid AS cursor,data FROM brazap_history WHERE account=? AND stage=1 AND rowid>? ORDER BY rowid LIMIT 1")
                    .bind::<BigInt,_>(account).bind::<BigInt,_>(cursor).get_result(conn).optional().map_err(db_error)
            }).await?;
            let Some(row) = row else {
                break;
            };
            cursor = row.cursor;
            let records = tokio::task::spawn_blocking(move || -> Result<Vec<wa::CallLogRecord>> {
                let mut decoded = Vec::new();
                flate2::read::ZlibDecoder::new(row.data.as_slice())
                    .take(128 * 1024 * 1024 + 1)
                    .read_to_end(&mut decoded)?;
                anyhow::ensure!(
                    decoded.len() <= 128 * 1024 * 1024,
                    "History chunk exceeds 128 MiB"
                );
                Ok(whatsapp_rust::waproto::codec::history_sync_decode(&decoded)?.call_log_records)
            })
            .await??;
            for record in records {
                self.store_call(&record, None, None, &own).await?;
            }
        }
        self.set_preference("calls_backfilled".into(), "1".into())
            .await
    }
}
