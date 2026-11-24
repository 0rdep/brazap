//! Who is in a group, for `@` mentions in the composer and MCP clients.
use anyhow::{Result, ensure};
use diesel::{
    prelude::*,
    sql_types::{BigInt, Text},
};

use crate::{
    model::Member,
    service::Service,
    storage::{Archive, db_error, meaningful_name},
};

#[derive(QueryableByName)]
struct Sender {
    #[diesel(sql_type = Text)]
    sender: String,
}

impl Archive {
    /// People who have written in a group, newest first: the members known offline.
    async fn chat_senders(&self, chat: &str) -> Result<Vec<String>> {
        let account = self.account as i64;
        let aliases = self.identities().await?.aliases_json(chat);
        Ok(self
            .sql()
            .read(move |conn| {
                diesel::sql_query("SELECT sender FROM brazap_messages WHERE account=? AND chat IN (SELECT value FROM json_each(?)) AND sender!='self' GROUP BY sender ORDER BY MAX(timestamp) DESC LIMIT 1024")
                    .bind::<BigInt, _>(account)
                    .bind::<Text, _>(aliases)
                    .load::<Sender>(conn)
                    .map_err(db_error)
            })
            .await?
            .into_iter()
            .map(|row| row.sender)
            .collect())
    }
}

impl Service {
    /// A group's members, named as the rest of the app names them, without this
    /// account. From WhatsApp when connected; otherwise the people seen writing.
    pub async fn group_members(&self, chat: String) -> Result<Vec<Member>> {
        let chat = self.archive.canonical_chat(&chat).await?;
        ensure!(
            chat.ends_with("@g.us"),
            "Only groups have members to mention"
        );
        self.archive.allow_chat(&chat).await?;
        let mut jids = Vec::new();
        if !self.config.demo
            && let Ok(client) = self.connected().await
        {
            match tokio::time::timeout(
                std::time::Duration::from_secs(10),
                client.groups().fetch_metadata(&chat.parse()?),
            )
            .await
            {
                Ok(Ok(group)) => {
                    jids = group
                        .participants
                        .iter()
                        .map(|p| p.jid.to_non_ad().to_string())
                        .collect();
                }
                Ok(Err(error)) => log::debug!("Group members unavailable: {error}"),
                Err(_) => log::debug!("Group members timed out"),
            }
        }
        if jids.is_empty() {
            jids = self.archive.chat_senders(&chat).await?;
        }
        let identities = self.archive.identities().await?;
        let own: std::collections::HashSet<_> = self
            .archive
            .own_jids()
            .await?
            .iter()
            .map(|jid| identities.canonical(jid))
            .collect();
        let mut seen = std::collections::HashSet::new();
        let mut members = Vec::new();
        for jid in jids {
            let canonical = identities.canonical(&jid);
            if own.contains(&canonical) || !seen.insert(canonical.clone()) {
                continue;
            }
            let (name, phone) = identities.person(&jid, "");
            members.push(Member {
                // The address the group uses is the one its mentions use.
                jid,
                name: if meaningful_name(&canonical, &name) {
                    name
                } else {
                    String::new()
                },
                phone,
            });
        }
        // Named people first, alphabetically; then unnamed numbers.
        members.sort_by(|a, b| {
            (a.name.is_empty(), a.name.to_lowercase(), &a.phone).cmp(&(
                b.name.is_empty(),
                b.name.to_lowercase(),
                &b.phone,
            ))
        });
        Ok(members)
    }
}
