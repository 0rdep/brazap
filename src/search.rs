use crate::{
    model::{Message, SearchHit, SearchResults},
    storage::{Archive, MESSAGE_SELECT, MessageRow, db_error, display_identity},
};
use anyhow::Result;
use diesel::{
    prelude::*,
    sql_types::{BigInt, Text},
};

#[derive(QueryableByName)]
struct NamedIdentity {
    #[diesel(sql_type=Text)]
    jid: String,
    #[diesel(sql_type=Text)]
    name: String,
}

pub(crate) fn decode(row: MessageRow) -> Result<Message> {
    let mut message: Message = serde_json::from_str(&row.payload)?;
    message.deleted_at = row.deleted_at;
    if row.deleted_at.is_some() && row.retain_deleted == 0 {
        message.text = "This message was deleted".into();
        message.kind = "text".into();
        message.title.clear();
        message.footer.clear();
        message.file_name = None;
        message.actions.clear();
        message.quote = None;
    }
    message.status = row.status;
    message.has_media = row.has_media != 0;
    message.reactions = serde_json::from_str(&row.reactions)?;
    if !message.from_me {
        message.sender_photo_version = row.sender_photo_version;
        (message.sender_name, message.sender_phone) =
            display_identity(&message.sender, &row.sender_name, &row.sender_phone);
    }
    Ok(message)
}

impl Archive {
    pub async fn search(&self, query: String) -> Result<SearchResults> {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return Ok(SearchResults::default());
        }
        let all = self.chats().await?;
        let identities = self.identities().await?;
        let account = self.account as i64;
        let names=self.backend.shared().read(move|conn| {
            diesel::sql_query("SELECT jid,name FROM brazap_chats WHERE account=? UNION ALL SELECT jid,name FROM brazap_contacts WHERE account=? UNION ALL SELECT jid,name FROM brazap_saved_contacts WHERE account=?")
                .bind::<BigInt,_>(account).bind::<BigInt,_>(account).bind::<BigInt,_>(account).load::<NamedIdentity>(conn).map_err(db_error)
        }).await?;
        let matching_names: std::collections::HashSet<_> = names
            .into_iter()
            .filter(|n| n.name.to_lowercase().contains(&query))
            .map(|n| identities.canonical(&n.jid))
            .collect();
        let digits: String = query.chars().filter(|c| c.is_ascii_digit()).collect();
        let chats = all
            .iter()
            .filter(|c| {
                c.name.to_lowercase().contains(&query)
                    || matching_names.contains(&c.jid)
                    || (!digits.is_empty() && digits.len() >= 3 && c.phone.contains(&digits))
            })
            .cloned()
            .collect();
        let text_query = query.clone();
        let account = self.account as i64;
        let rows:Vec<MessageRow>=self.backend.shared().read(move |conn| {
            diesel::sql_query(format!("{MESSAGE_SELECT} JOIN brazap_message_search s ON m.account=s.account AND m.chat=s.chat AND m.sender=s.sender AND m.id=s.id WHERE s.account=? AND instr(s.text,?)>0 ORDER BY m.timestamp DESC,m.id DESC,m.sender DESC LIMIT 400"))
                .bind::<BigInt,_>(account).bind::<Text,_>(query).load(conn).map_err(db_error)
        }).await?;
        let mut messages = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for row in rows {
            let message = identities.message(decode(row)?);
            if identities.is_locked(&message.chat) {
                continue;
            }
            let author = if message.chat.ends_with("@g.us") {
                identities.canonical(&message.sender)
            } else {
                message.from_me.to_string()
            };
            if !seen.insert((message.chat.clone(), message.id.clone(), author)) {
                continue;
            }
            if let Some(chat) = all.iter().find(|c| c.jid == message.chat) {
                messages.push(SearchHit {
                    chat: chat.clone(),
                    excerpt: excerpt(&message, &text_query),
                    message,
                });
            }
            if messages.len() >= 100 {
                break;
            }
        }
        Ok(SearchResults { chats, messages })
    }
}

fn excerpt(message: &Message, query: &str) -> String {
    let text = format!(
        "{} {} {} {}",
        message.title,
        message.text,
        message.footer,
        message.file_name.as_deref().unwrap_or("")
    );
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let lower = text.to_lowercase();
    let pos = lower
        .find(query)
        .map(|i| lower[..i].chars().count())
        .unwrap_or(0);
    let start = pos.saturating_sub(35);
    let count = text.chars().count();
    format!(
        "{}{}{}",
        if start > 0 { "…" } else { "" },
        text.chars().skip(start).take(150).collect::<String>(),
        if count > start + 150 { "…" } else { "" }
    )
}
