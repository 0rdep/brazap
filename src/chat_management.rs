use crate::{
    model::{Chat, now},
    service::Service,
    storage::{Archive, db_error},
};
use anyhow::{Context, Result};
use diesel::{
    prelude::*,
    sql_types::{BigInt, Text},
};

#[derive(QueryableByName)]
struct Deleted {
    #[diesel(sql_type=Text)]
    jid: String,
}
#[derive(QueryableByName)]
struct Boundary {
    #[diesel(sql_type=BigInt)]
    timestamp: i64,
}

impl Archive {
    pub(crate) async fn filter_deleted_chats(&self, chats: Vec<Chat>) -> Result<Vec<Chat>> {
        let account = self.account as i64;
        let deleted: Vec<Deleted> = self
            .backend
            .shared()
            .read(move |conn| {
                diesel::sql_query("SELECT jid FROM brazap_deleted_chats WHERE account=?")
                    .bind::<BigInt, _>(account)
                    .load(conn)
                    .map_err(db_error)
            })
            .await?;
        Ok(chats
            .into_iter()
            .filter(|c| {
                !deleted
                    .iter()
                    .any(|d| c.matches_jid(&d.jid) && c.preview_kind.is_empty())
            })
            .collect())
    }

    pub(crate) async fn chat_boundary(&self, chat: &str) -> Result<i64> {
        let account = self.account as i64;
        let aliases = self.identities().await?.aliases_json(chat);
        let boundary:Boundary=self.backend.shared().read(move |conn|{
            diesel::sql_query("SELECT COALESCE(MAX(timestamp),0) AS timestamp FROM (SELECT timestamp FROM brazap_messages WHERE account=? AND chat IN (SELECT value FROM json_each(?)) UNION ALL SELECT updated_at AS timestamp FROM brazap_chats WHERE account=? AND jid IN (SELECT value FROM json_each(?)))")
                .bind::<BigInt,_>(account).bind::<Text,_>(&aliases).bind::<BigInt,_>(account).bind::<Text,_>(&aliases).get_result(conn).map_err(db_error)
        }).await?;
        Ok(boundary.timestamp)
    }

    pub(crate) async fn delete_chat_through(&self, chat: &str, through: i64) -> Result<()> {
        let account = self.account as i64;
        let aliases = self.identities().await?.aliases_json(chat);
        self.backend.shared().run(move |conn| {
            conn.transaction::<_,diesel::result::Error,_>(|conn| {
                diesel::sql_query("INSERT INTO brazap_deleted_chats(account,jid,through_timestamp) SELECT ?,value,? FROM json_each(?) WHERE true ON CONFLICT(account,jid) DO UPDATE SET through_timestamp=MAX(through_timestamp,excluded.through_timestamp)")
                    .bind::<BigInt,_>(account).bind::<BigInt,_>(through).bind::<Text,_>(&aliases).execute(conn)?;
                // Delete retained message content and its projections together. The
                // boundary prevents old history chunks from recreating deleted rows.
                diesel::sql_query("INSERT OR IGNORE INTO brazap_deleted_message_ids(account,chat,sender,id) SELECT account,chat,sender,id FROM brazap_messages WHERE account=? AND chat IN (SELECT value FROM json_each(?)) AND timestamp<=?")
                    .bind::<BigInt,_>(account).bind::<Text,_>(&aliases).bind::<BigInt,_>(through).execute(conn)?;
                for table in ["brazap_media","brazap_message_search","brazap_receipts","brazap_read_messages"] {
                    diesel::sql_query(format!("DELETE FROM {table} WHERE account=? AND chat IN (SELECT value FROM json_each(?)) AND id IN (SELECT id FROM brazap_messages WHERE account=? AND chat IN (SELECT value FROM json_each(?)) AND timestamp<=?)"))
                        .bind::<BigInt,_>(account).bind::<Text,_>(&aliases).bind::<BigInt,_>(account).bind::<Text,_>(&aliases).bind::<BigInt,_>(through).execute(conn)?;
                }
                diesel::sql_query("DELETE FROM brazap_reactions WHERE account=? AND chat IN (SELECT value FROM json_each(?)) AND target_id IN (SELECT id FROM brazap_messages WHERE account=? AND chat IN (SELECT value FROM json_each(?)) AND timestamp<=?)")
                    .bind::<BigInt,_>(account).bind::<Text,_>(&aliases).bind::<BigInt,_>(account).bind::<Text,_>(&aliases).bind::<BigInt,_>(through).execute(conn)?;
                diesel::sql_query("DELETE FROM brazap_messages WHERE account=? AND chat IN (SELECT value FROM json_each(?)) AND timestamp<=?")
                    .bind::<BigInt,_>(account).bind::<Text,_>(&aliases).bind::<BigInt,_>(through).execute(conn)?;
                diesel::sql_query("DELETE FROM brazap_chats WHERE account=? AND jid IN (SELECT value FROM json_each(?)) AND NOT EXISTS(SELECT 1 FROM brazap_messages m WHERE m.account=brazap_chats.account AND m.chat=brazap_chats.jid)")
                    .bind::<BigInt,_>(account).bind::<Text,_>(&aliases).execute(conn)?;
                let aliases:Vec<String>=serde_json::from_str(&aliases).unwrap_or_default();
                for alias in aliases {
                    for prefix in ["thumb:","image:","link:"] {
                        let prefix=format!("{prefix}{alias}:");
                        diesel::sql_query("DELETE FROM brazap_assets WHERE account=? AND substr(key,1,?)=?")
                            .bind::<BigInt,_>(account).bind::<BigInt,_>(prefix.len() as i64).bind::<Text,_>(prefix).execute(conn)?;
                    }
                }
                Ok(())
            }).map_err(db_error)
        }).await?;
        Ok(())
    }
}

impl Service {
    pub async fn delete_chat(&self, chat: String) -> Result<()> {
        self.archive.allow_chat(&chat).await?;
        let chat = self.archive.canonical_chat(&chat).await?;
        let through = self.archive.chat_boundary(&chat).await?;
        if !self.config.demo {
            let client = self.connected().await?;
            let mut range =
                whatsapp_rust::prelude::wa::sync_action_value::SyncActionMessageRange::default();
            range.last_message_timestamp = Some(through);
            client
                .chat_actions()
                .delete_chat(&chat.parse()?, true, Some(range))
                .await?;
        }
        self.archive.delete_chat_through(&chat, through).await?;
        self.changed();
        Ok(())
    }

    /// The region of the account's own number, used for numbers typed without a country code.
    pub async fn account_region(&self) -> Option<String> {
        if let Some(region) = self.region.lock().ok().and_then(|region| region.clone()) {
            return Some(region);
        }
        let own = self.archive.own_jids().await.ok()?;
        let digits = own
            .iter()
            .find_map(|jid| jid.strip_suffix("@s.whatsapp.net"))?;
        let region = crate::phone_links::region_of(digits)?;
        if let Ok(mut cached) = self.region.lock() {
            *cached = Some(region.clone());
        }
        Some(region)
    }

    /// Opens a chat from a JID, a phone number or a WhatsApp link (`wa.me`,
    /// `whatsapp://send`). Numbers are checked with WhatsApp first, which also returns the
    /// address the account is registered under. Returns the chat and any message the link
    /// asks to start with.
    pub async fn open_address(&self, address: &str) -> Result<(String, Option<String>)> {
        let address = address.trim();
        let (jid, text) = if address.contains('@') && !address.contains('/') {
            (address.parse::<whatsapp_rust::Jid>()?, None)
        } else {
            let region = self.account_region().await;
            let target = crate::phone_links::parse(address, region.as_deref()).context(
                if region.is_some() {
                    "Enter a phone number or a WhatsApp link"
                } else {
                    "Enter a phone number with its country code, or a WhatsApp link"
                },
            )?;
            (self.registered_jid(&target.digits).await?, target.text)
        };
        let chat = self.archive.canonical_chat(&jid.to_string()).await?;
        self.archive
            .name_chat(chat.clone(), String::new(), 0, now())
            .await?;
        self.changed();
        Ok((chat, text))
    }

    async fn registered_jid(&self, digits: &str) -> Result<whatsapp_rust::Jid> {
        let jid = whatsapp_rust::Jid::pn(digits);
        // The demo has no WhatsApp connection to ask; every number opens.
        if self.config.demo {
            return Ok(jid);
        }
        let client = self.connected().await?;
        let results = client.contacts().is_on_whatsapp(&[jid]).await?;
        let found = results
            .into_iter()
            .find(|result| result.is_registered)
            .context("This number isn't on WhatsApp")?;
        // A LID answer still names the phone number it belongs to.
        Ok(found
            .pn_jid
            .filter(|_| found.jid.server.is_lid_family())
            .unwrap_or(found.jid))
    }

    pub async fn mark_chat_unread(&self, chat: String) -> Result<()> {
        self.archive.allow_chat(&chat).await?;
        if !self.config.demo {
            self.connected()
                .await?
                .chat_actions()
                .mark_chat_as_read(&chat.parse()?, false, None)
                .await?;
        }
        self.archive
            .apply_chat_read(&chat, None, Vec::new(), now() * 1000, true)
            .await?;
        self.changed();
        Ok(())
    }
}
