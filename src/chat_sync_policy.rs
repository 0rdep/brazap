use crate::{
    model::now,
    storage::{Archive, db_error},
};
use anyhow::{Result, ensure};
use diesel::{
    prelude::*,
    sql_types::{BigInt, Text},
};

#[derive(QueryableByName)]
struct Count {
    #[diesel(sql_type=BigInt)]
    value: i64,
}

impl Archive {
    pub(crate) async fn set_chat_locked(&self, jid: String, locked: bool, at: i64) -> Result<()> {
        let account = self.account as i64;
        self.backend.shared().run(move |conn| {
            diesel::sql_query("INSERT INTO brazap_chat_locks(account,jid,locked,changed_at) VALUES(?,?,?,?) ON CONFLICT(account,jid) DO UPDATE SET locked=excluded.locked,changed_at=excluded.changed_at WHERE excluded.changed_at>=brazap_chat_locks.changed_at")
                .bind::<BigInt,_>(account).bind::<Text,_>(jid).bind::<BigInt,_>(i64::from(locked)).bind::<BigInt,_>(at).execute(conn).map_err(db_error)?;
            Ok(())
        }).await?;
        Ok(())
    }
    pub(crate) async fn allow_chat(&self, chat: &str) -> Result<()> {
        ensure!(
            !self.identities().await?.is_locked(chat),
            "This conversation is locked on WhatsApp and is excluded from Brazap."
        );
        Ok(())
    }
    pub(crate) async fn history_is_complete(&self, chat: &str) -> Result<bool> {
        let aliases = self.chat_aliases(chat).await?;
        let aliases = serde_json::to_string(&aliases)?;
        let account = self.account as i64;
        let count:Count=self.backend.shared().read(move|conn| {
            diesel::sql_query("SELECT COUNT(*) AS value FROM brazap_history_complete WHERE account=? AND chat IN (SELECT value FROM json_each(?))")
                .bind::<BigInt,_>(account).bind::<Text,_>(aliases).get_result(conn).map_err(db_error)
        }).await?;
        Ok(count.value > 0)
    }
    pub(crate) async fn was_auto_prefetched(&self, chat: &str) -> Result<bool> {
        let aliases = serde_json::to_string(&self.chat_aliases(chat).await?)?;
        let account = self.account as i64;
        let count: Count=self.backend.shared().read(move |conn| {
            diesel::sql_query("SELECT COUNT(*) AS value FROM brazap_history_prefetched WHERE account=? AND chat IN (SELECT value FROM json_each(?))")
                .bind::<BigInt,_>(account).bind::<Text,_>(aliases).get_result(conn).map_err(db_error)
        }).await?;
        Ok(count.value > 0)
    }
    pub(crate) async fn mark_auto_prefetched(&self, chat: &str) -> Result<()> {
        let account = self.account as i64;
        let chat = chat.to_owned();
        self.backend.shared().run(move|conn| {
            diesel::sql_query("INSERT OR IGNORE INTO brazap_history_prefetched(account,chat,completed_at) VALUES(?,?,?)")
                .bind::<BigInt,_>(account).bind::<Text,_>(chat).bind::<BigInt,_>(now()).execute(conn).map_err(db_error)?;
            Ok(())
        }).await?;
        Ok(())
    }

    pub(crate) async fn complete_history(&self, chat: String, reason: &str) -> Result<()> {
        let (account, reason) = (self.account as i64, reason.to_owned());
        self.backend.shared().run(move|conn| {
            diesel::sql_query("INSERT INTO brazap_history_complete(account,chat,completed_at,reason) VALUES(?,?,?,?) ON CONFLICT(account,chat) DO UPDATE SET completed_at=excluded.completed_at,reason=excluded.reason")
                .bind::<BigInt,_>(account).bind::<Text,_>(chat).bind::<BigInt,_>(now()).bind::<Text,_>(reason).execute(conn).map_err(db_error)?;
            Ok(())
        }).await?;
        Ok(())
    }
}
