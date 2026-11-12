use crate::{
    model::Message,
    service::Service,
    storage::{Archive, db_error},
};
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use diesel::{
    prelude::*,
    sql_types::{BigInt, Binary, Nullable, Text},
};
use serde::{Deserialize, Serialize};
use whatsapp_rust::{
    prelude::wa,
    waproto::buffa::{Message as ProtoMessage, MessageField},
};

#[derive(Clone, Debug, Serialize, Deserialize, QueryableByName)]
pub struct Sticker {
    #[serde(skip)]
    #[diesel(sql_type=Text)]
    chat: String,
    #[diesel(sql_type=Text)]
    pub hash: String,
    #[diesel(sql_type=Text)]
    pub label: String,
    #[diesel(sql_type=Text)]
    pub emojis: String,
    #[diesel(sql_type=BigInt)]
    pub favorite: i64,
    #[diesel(sql_type=BigInt)]
    pub recent_at: i64,
}
#[derive(QueryableByName)]
struct Stored {
    #[diesel(sql_type=Binary)]
    encoded: Vec<u8>,
    #[diesel(sql_type=Nullable<Binary>)]
    data: Option<Vec<u8>>,
    #[diesel(sql_type=BigInt)]
    from_me: i64,
    #[diesel(sql_type=Text)]
    chat: String,
    #[diesel(sql_type=Text)]
    sender: String,
    #[diesel(sql_type=Text)]
    id: String,
}
#[derive(QueryableByName)]
struct Source {
    #[diesel(sql_type=Binary)]
    encoded: Vec<u8>,
    #[diesel(sql_type=Text)]
    payload: String,
}

pub(crate) fn index(
    conn: &mut diesel::SqliteConnection,
    account: i64,
    message: &Message,
    encoded: &[u8],
) -> diesel::QueryResult<()> {
    if message.kind != "sticker" {
        return Ok(());
    }
    let Ok(proto) = wa::Message::decode_from_slice(encoded) else {
        return Ok(());
    };
    let base = crate::content::media_message(&proto);
    let Some(sticker) = base.sticker_message.as_option() else {
        return Ok(());
    };
    let Some(hash) = sticker
        .file_sha256
        .as_ref()
        .map(|hash| STANDARD.encode(hash))
    else {
        return Ok(());
    };
    let encoded = base.encode_to_vec();
    diesel::sql_query("INSERT INTO brazap_stickers(account,hash,encoded,label,emojis,recent_at,updated_at,from_me,chat,sender,id) VALUES(?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(account,hash) DO UPDATE SET encoded=CASE WHEN excluded.updated_at>=brazap_stickers.updated_at THEN excluded.encoded ELSE brazap_stickers.encoded END, label=CASE WHEN excluded.label!='' THEN excluded.label ELSE brazap_stickers.label END,emojis=CASE WHEN excluded.emojis!='' THEN excluded.emojis ELSE brazap_stickers.emojis END,recent_at=MAX(brazap_stickers.recent_at,excluded.recent_at),updated_at=MAX(brazap_stickers.updated_at,excluded.updated_at),from_me=excluded.from_me,chat=excluded.chat,sender=excluded.sender,id=excluded.id")
        .bind::<BigInt,_>(account).bind::<Text,_>(hash).bind::<Binary,_>(&encoded)
        .bind::<Text,_>(sticker.accessibility_label.as_deref().unwrap_or("")).bind::<Text,_>(sticker.emojis.as_deref().unwrap_or(""))
        .bind::<BigInt,_>(if message.from_me {message.timestamp} else {0}).bind::<BigInt,_>(message.timestamp).bind::<BigInt,_>(i64::from(message.from_me))
        .bind::<Text,_>(&message.chat).bind::<Text,_>(&message.sender).bind::<Text,_>(&message.id).execute(conn)?;
    Ok(())
}

impl Archive {
    pub(crate) async fn index_saved_stickers(&self) -> Result<()> {
        if self
            .preference("stickers_indexed_v1".into())
            .await?
            .is_some()
        {
            return Ok(());
        }
        let account = self.account as i64;
        self.backend.shared().run(move |conn| {
            let rows: Vec<Source> = diesel::sql_query("SELECT encoded,payload FROM brazap_messages WHERE account=? AND json_extract(payload,'$.kind')='sticker' ORDER BY timestamp")
                .bind::<BigInt,_>(account).load(conn).map_err(db_error)?;
            conn.transaction::<_,diesel::result::Error,_>(|conn| {
                for row in rows {
                    let message: Message = serde_json::from_str(&row.payload).map_err(|e|diesel::result::Error::DeserializationError(Box::new(e)))?;
                    index(conn,account,&message,&row.encoded)?;
                }
                Ok(())
            }).map_err(db_error)
        }).await?;
        self.set_preference("stickers_indexed_v1".into(), "1".into())
            .await
    }

    pub(crate) async fn favorite_sticker(
        &self,
        event: &whatsapp_rust::types::events::FavoriteStickerUpdate,
    ) -> Result<()> {
        let action = &event.action;
        let has_media = i64::from(
            action.media_key.as_ref().is_some_and(|key| !key.is_empty())
                && (action.direct_path.is_some() || action.url.is_some()),
        );
        let mut sticker = wa::message::StickerMessage::default();
        sticker.file_sha256 = Some(STANDARD.decode(&event.filehash)?);
        sticker.file_enc_sha256 = action.file_enc_sha256.clone();
        sticker.media_key = action.media_key.clone();
        sticker.direct_path = action.direct_path.clone();
        sticker.url = action.url.clone();
        sticker.mimetype = action.mimetype.clone();
        sticker.file_length = action.file_length;
        sticker.width = action.width;
        sticker.height = action.height;
        let mut proto = wa::Message::default();
        proto.sticker_message = MessageField::some(sticker);
        let (account, hash, encoded, favorite, at) = (
            self.account as i64,
            event.filehash.clone(),
            proto.encode_to_vec(),
            i64::from(action.is_favorite == Some(true)),
            event.timestamp.timestamp(),
        );
        self.backend.shared().run(move |conn| {
            diesel::sql_query("INSERT INTO brazap_stickers(account,hash,encoded,favorite,updated_at) VALUES(?,?,?,?,?) ON CONFLICT(account,hash) DO UPDATE SET favorite=excluded.favorite,encoded=CASE WHEN ?=1 AND excluded.updated_at>=brazap_stickers.updated_at THEN excluded.encoded ELSE brazap_stickers.encoded END,updated_at=MAX(brazap_stickers.updated_at,excluded.updated_at)")
                .bind::<BigInt,_>(account).bind::<Text,_>(hash).bind::<Binary,_>(encoded).bind::<BigInt,_>(favorite).bind::<BigInt,_>(at).bind::<BigInt,_>(has_media).execute(conn).map_err(db_error)?;
            Ok(())
        }).await?;
        Ok(())
    }

    async fn stored_sticker(&self, hash: String) -> Result<Stored> {
        let account = self.account as i64;
        Ok(self.backend.shared().read(move|conn|diesel::sql_query("SELECT encoded,data,from_me,chat,sender,id FROM brazap_stickers WHERE account=? AND hash=?")
            .bind::<BigInt,_>(account).bind::<Text,_>(hash).get_result(conn).map_err(db_error)).await?)
    }
}

impl Service {
    pub async fn sticker_catalog(&self, sync: bool) -> Result<Vec<Sticker>> {
        self.archive.index_saved_stickers().await?;
        if sync && !self.config.demo {
            let client = self.connected().await?;
            let report = client
                .resync_app_state(
                    [whatsapp_rust::wacore::appstate::patch_decode::WAPatchName::RegularLow],
                    whatsapp_rust::AppStateResyncMode::Snapshot,
                )
                .await?;
            ensure!(
                report.all_synced(),
                "WhatsApp could not finish syncing stickers. Try again shortly."
            );
        }
        let account = self.archive.account as i64;
        let identities = self.archive.identities().await?;
        let stickers:Vec<Sticker>=self.archive.backend.shared().read(move|conn|diesel::sql_query("SELECT chat,hash,label,emojis,favorite,recent_at FROM brazap_stickers WHERE account=? ORDER BY MAX(recent_at,updated_at) DESC LIMIT 2000")
            .bind::<BigInt,_>(account).load(conn).map_err(db_error)).await?;
        Ok(stickers
            .into_iter()
            .filter(|sticker| !identities.is_locked(&sticker.chat))
            .collect())
    }

    pub(crate) async fn sticker_bytes(&self, hash: String) -> Result<Vec<u8>> {
        let stored = self.archive.stored_sticker(hash.clone()).await?;
        if let Some(data) = stored.data {
            return Ok(data);
        }
        // History/demo stickers may already be in the message media archive
        // before the catalog has its own cached copy. This also works offline.
        if !stored.chat.is_empty()
            && let Some(data) = self
                .archive
                .media(
                    stored.chat.clone(),
                    stored.sender.clone(),
                    stored.id.clone(),
                )
                .await?
        {
            return Ok(data);
        }
        let proto = wa::Message::decode_from_slice(&stored.encoded)?;
        let sticker = proto
            .sticker_message
            .as_option()
            .context("Sticker is unavailable")?;
        let client = self.connected().await?;
        let bytes = if !stored.chat.is_empty() {
            let message = Message {
                chat: stored.chat,
                sender: stored.sender,
                id: stored.id,
                kind: "sticker".into(),
                from_me: stored.from_me != 0,
                ..Default::default()
            };
            self.download_media(&message, proto).await?
        } else {
            client.download(sticker,whatsapp_rust::download::DownloadLimits::new(16*1024*1024,8*1024*1024)).await
                .context("This saved sticker is unavailable. Open it on your phone, then sync stickers again.")?
        };
        let (account, data) = (self.archive.account as i64, bytes.clone());
        self.archive
            .backend
            .shared()
            .run(move |conn| {
                diesel::sql_query("UPDATE brazap_stickers SET data=? WHERE account=? AND hash=?")
                    .bind::<Binary, _>(data)
                    .bind::<BigInt, _>(account)
                    .bind::<Text, _>(hash)
                    .execute(conn)
                    .map_err(db_error)?;
                Ok(())
            })
            .await?;
        Ok(bytes)
    }

    pub async fn send_sticker(&self, chat: String, hash: String) -> Result<Message> {
        self.archive.allow_chat(&chat).await?;
        let bytes = self.sticker_bytes(hash.clone()).await?;
        if self.config.demo {
            let stored = self.archive.stored_sticker(hash).await?;
            return self
                .demo_send(
                    chat,
                    wa::Message::decode_from_slice(&stored.encoded)?,
                    Some(bytes),
                )
                .await;
        }
        let client = self.connected().await?;
        let uploaded = client
            .upload(
                bytes.clone(),
                whatsapp_rust::download::MediaType::Sticker,
                Default::default(),
            )
            .await?;
        let stored = self.archive.stored_sticker(hash).await?;
        let mut proto = wa::Message::decode_from_slice(&stored.encoded)?;
        let sticker = proto
            .sticker_message
            .as_option_mut()
            .context("Sticker is unavailable")?;
        sticker.url = Some(uploaded.url);
        sticker.direct_path = Some(uploaded.direct_path);
        sticker.media_key = Some(uploaded.media_key.to_vec());
        sticker.file_enc_sha256 = Some(uploaded.file_enc_sha256.to_vec());
        sticker.file_sha256 = Some(uploaded.file_sha256.to_vec());
        sticker.file_length = Some(uploaded.file_length);
        sticker.context_info = MessageField::default();
        self.send_prepared(client, chat.parse()?, proto, Some(bytes))
            .await
    }
}

/// The sticker in an archived message, with the hash WhatsApp identifies it by.
fn message_sticker(proto: &wa::Message) -> Result<(String, wa::message::StickerMessage)> {
    let base = crate::content::media_message(proto);
    let sticker = base
        .sticker_message
        .as_option()
        .context("This message is not a sticker")?
        .clone();
    let hash = sticker
        .file_sha256
        .as_ref()
        .map(|hash| STANDARD.encode(hash))
        .context("This sticker cannot be saved")?;
    Ok((hash, sticker))
}

impl Service {
    /// Adds a sticker from a message to My stickers, or removes it, on every linked device.
    pub async fn set_sticker_favorite(
        &self,
        chat: String,
        sender: String,
        id: String,
        favorite: bool,
    ) -> Result<()> {
        self.archive.allow_chat(&chat).await?;
        let proto = self
            .archive
            .raw_message(chat.clone(), sender.clone(), id.clone())
            .await?;
        let (hash, sticker) = message_sticker(&proto)?;
        if !self.config.demo {
            self.sync_favorite(&hash, &sticker, favorite).await?;
        }
        let mut stored = wa::Message::default();
        stored.sticker_message = MessageField::some(sticker);
        self.store_favorite(
            hash,
            stored.encode_to_vec(),
            None,
            favorite,
            (chat, sender, id),
        )
        .await?;
        self.changed();
        Ok(())
    }

    async fn sync_favorite(
        &self,
        hash: &str,
        sticker: &wa::message::StickerMessage,
        favorite: bool,
    ) -> Result<()> {
        ensure!(
            sticker
                .media_key
                .as_ref()
                .is_some_and(|key| !key.is_empty())
                && (sticker.direct_path.is_some() || sticker.url.is_some()),
            "This sticker can no longer be downloaded, so it cannot be saved"
        );
        let mut action = wa::sync_action_value::StickerAction::default();
        action.url = sticker.url.clone();
        action.file_enc_sha256 = sticker.file_enc_sha256.clone();
        action.media_key = sticker.media_key.clone();
        action.mimetype = sticker.mimetype.clone();
        action.height = sticker.height;
        action.width = sticker.width;
        action.direct_path = sticker.direct_path.clone();
        action.file_length = sticker.file_length;
        action.is_favorite = Some(favorite);
        let mut value = wa::SyncActionValue::default();
        value.sticker_action = MessageField::some(action);
        value.timestamp = Some(crate::model::now() * 1000);
        self.connected()
            .await?
            .send_app_state_action(&whatsapp_rust::schemas::FAVORITE_STICKER, &[hash], &value)
            .await?;
        Ok(())
    }

    async fn store_favorite(
        &self,
        hash: String,
        encoded: Vec<u8>,
        data: Option<Vec<u8>>,
        favorite: bool,
        (chat, sender, id): (String, String, String),
    ) -> Result<()> {
        let account = self.archive.account as i64;
        let now = crate::model::now();
        self.archive.backend.shared().run(move |conn| {
            diesel::sql_query("INSERT INTO brazap_stickers(account,hash,encoded,favorite,updated_at,chat,sender,id,data) VALUES(?,?,?,?,?,?,?,?,?) ON CONFLICT(account,hash) DO UPDATE SET favorite=excluded.favorite,updated_at=MAX(brazap_stickers.updated_at,excluded.updated_at),data=COALESCE(excluded.data,brazap_stickers.data),chat=CASE WHEN brazap_stickers.chat='' THEN excluded.chat ELSE brazap_stickers.chat END,sender=CASE WHEN brazap_stickers.chat='' THEN excluded.sender ELSE brazap_stickers.sender END,id=CASE WHEN brazap_stickers.chat='' THEN excluded.id ELSE brazap_stickers.id END")
                .bind::<BigInt,_>(account).bind::<Text,_>(hash).bind::<Binary,_>(encoded).bind::<BigInt,_>(i64::from(favorite)).bind::<BigInt,_>(now)
                .bind::<Text,_>(chat).bind::<Text,_>(sender).bind::<Text,_>(id).bind::<Nullable<Binary>,_>(data)
                .execute(conn).map_err(db_error)?;
            Ok(())
        }).await?;
        Ok(())
    }

    /// The hashes of the stickers in My stickers.
    pub async fn favorite_sticker_hashes(&self) -> Result<Vec<String>> {
        let account = self.archive.account as i64;
        #[derive(QueryableByName)]
        struct Row {
            #[diesel(sql_type=Text)]
            hash: String,
        }
        let rows: Vec<Row> = self
            .archive
            .backend
            .shared()
            .read(move |conn| {
                diesel::sql_query("SELECT hash FROM brazap_stickers WHERE account=? AND favorite=1")
                    .bind::<BigInt, _>(account)
                    .load(conn)
                    .map_err(db_error)
            })
            .await?;
        Ok(rows.into_iter().map(|row| row.hash).collect())
    }

    /// The media bytes of an archived message, downloading them when needed.
    async fn message_media(
        &self,
        chat: &str,
        sender: &str,
        id: &str,
    ) -> Result<(Message, wa::Message, Vec<u8>)> {
        self.archive.allow_chat(chat).await?;
        let proto = self
            .archive
            .raw_message(chat.into(), sender.into(), id.into())
            .await?;
        let view = crate::storage::project_message(
            chat.into(),
            sender.into(),
            id.into(),
            false,
            0,
            &proto,
            String::new(),
            "{}".into(),
        )
        .view;
        let bytes = self.download_media(&view, proto.clone()).await?;
        Ok((view, proto, bytes))
    }

    /// Makes a sticker from a photo, video or GIF and adds it to My stickers.
    #[cfg(feature = "desktop")]
    pub async fn create_sticker(&self, chat: String, sender: String, id: String) -> Result<()> {
        let (view, _, bytes) = self.message_media(&chat, &sender, &id).await?;
        let kind = view.kind.clone();
        let webp = tokio::task::spawn_blocking(move || match kind.as_str() {
            "image" => crate::media_convert::image_sticker(&bytes),
            "video" => crate::media_convert::video_sticker(&bytes),
            _ => anyhow::bail!("Stickers can be made from photos, videos and GIFs"),
        })
        .await??;
        let animated = crate::media_convert::animated_webp(&webp);
        let mut sticker = wa::message::StickerMessage::default();
        sticker.mimetype = Some("image/webp".into());
        sticker.width = Some(crate::media_convert::STICKER_SIDE);
        sticker.height = Some(crate::media_convert::STICKER_SIDE);
        sticker.is_animated = Some(animated);
        if self.config.demo {
            use sha2::Digest;
            sticker.file_sha256 = Some(sha2::Sha256::digest(&webp).to_vec());
            sticker.file_length = Some(webp.len() as u64);
        } else {
            let client = self.connected().await?;
            let uploaded = client
                .upload(
                    webp.clone(),
                    whatsapp_rust::download::MediaType::Sticker,
                    Default::default(),
                )
                .await?;
            sticker.url = Some(uploaded.url);
            sticker.direct_path = Some(uploaded.direct_path);
            sticker.media_key = Some(uploaded.media_key.to_vec());
            sticker.file_sha256 = Some(uploaded.file_sha256.to_vec());
            sticker.file_enc_sha256 = Some(uploaded.file_enc_sha256.to_vec());
            sticker.file_length = Some(uploaded.file_length);
        }
        let hash = STANDARD.encode(sticker.file_sha256.as_deref().unwrap_or_default());
        if !self.config.demo {
            self.sync_favorite(&hash, &sticker, true).await?;
        }
        let mut stored = wa::Message::default();
        stored.sticker_message = MessageField::some(sticker);
        self.store_favorite(
            hash,
            stored.encode_to_vec(),
            Some(webp),
            true,
            Default::default(),
        )
        .await?;
        self.changed();
        Ok(())
    }

    /// Writes an animated sticker or a WhatsApp GIF to `path` as a GIF file.
    #[cfg(feature = "desktop")]
    pub async fn save_gif(
        &self,
        chat: String,
        sender: String,
        id: String,
        path: String,
    ) -> Result<()> {
        let (view, _, bytes) = self.message_media(&chat, &sender, &id).await?;
        let gif = tokio::task::spawn_blocking(move || match view.kind.as_str() {
            "sticker" if crate::media_convert::animated_webp(&bytes) => {
                crate::media_convert::webp_to_gif(&bytes)
            }
            "video" => crate::media_convert::video_to_gif(&bytes),
            _ => anyhow::bail!("Only GIFs and animated stickers can be copied as GIFs"),
        })
        .await??;
        tokio::fs::write(path, gif).await?;
        Ok(())
    }
}
