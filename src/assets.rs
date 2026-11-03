use std::{io::Cursor, time::Duration};

use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use diesel::{
    prelude::*,
    sql_types::{BigInt, Binary, Nullable, Text},
};
use serde::{Deserialize, Serialize};
use whatsapp_rust::{ProfilePictureRequest, ProfilePictureTarget, ProfilePictureType};

use whatsapp_rust::wacore_binary::JidExt;

use crate::{
    model::{Message, now},
    service::Service,
    storage::Archive,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AssetRequest {
    Sticker {
        hash: String,
    },
    Avatar {
        jid: String,
        #[serde(default)]
        common_group: Option<String>,
    },
    Profile {
        jid: String,
    },
    Thumbnail {
        chat: String,
        sender: String,
        id: String,
    },
    Image {
        chat: String,
        sender: String,
        id: String,
    },
}

impl AssetRequest {
    pub fn key(&self) -> String {
        match self {
            Self::Sticker { hash } => format!("sticker-v2:{hash}"),
            Self::Avatar { jid, .. } => format!("avatar:{jid}"),
            Self::Profile { jid } => format!("profile:{jid}"),
            Self::Thumbnail { chat, sender, id } => format!("thumb:{chat}:{sender}:{id}"),
            Self::Image { chat, sender, id } => format!("image:{chat}:{sender}:{id}"),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Asset {
    pub mime: String,
    pub data: String,
}

impl Asset {
    pub fn bytes(self) -> Result<Vec<u8>> {
        Ok(STANDARD.decode(self.data)?)
    }
}

#[derive(QueryableByName)]
struct CachedAsset {
    #[diesel(sql_type=Text)]
    mime: String,
    #[diesel(sql_type=Nullable<Binary>)]
    data: Option<Vec<u8>>,
    #[diesel(sql_type=BigInt)]
    expires_at: i64,
}

impl Archive {
    async fn cached_asset(&self, key: String) -> Result<Option<CachedAsset>> {
        let account = self.account as i64;
        Ok(self
            .backend
            .shared()
            .read(move |conn| {
                diesel::sql_query(
                    "SELECT mime,data,expires_at FROM brazap_assets WHERE account=? AND key=?",
                )
                .bind::<BigInt, _>(account)
                .bind::<Text, _>(key)
                .get_result(conn)
                .optional()
                .map_err(|e| whatsapp_rust::store::error::StoreError::Database(Box::new(e)))
            })
            .await?)
    }

    async fn cache_asset(
        &self,
        key: String,
        mime: String,
        data: Option<Vec<u8>>,
        ttl: i64,
    ) -> Result<()> {
        let account = self.account as i64;
        let time = now();
        self.backend.shared().run(move |conn| {
            diesel::sql_query("INSERT INTO brazap_assets(account,key,mime,data,updated_at,expires_at) VALUES(?,?,?,?,?,?) ON CONFLICT(account,key) DO UPDATE SET mime=excluded.mime,data=excluded.data,updated_at=MAX(brazap_assets.updated_at+1,excluded.updated_at),expires_at=excluded.expires_at")
                .bind::<BigInt,_>(account).bind::<Text,_>(key).bind::<Text,_>(mime).bind::<Nullable<Binary>,_>(data)
                .bind::<BigInt,_>(time).bind::<BigInt,_>(time.saturating_add(ttl)).execute(conn)
                .map_err(|e|whatsapp_rust::store::error::StoreError::Database(Box::new(e)))?;
            Ok(())
        }).await?;
        Ok(())
    }

    pub async fn invalidate_avatar(&self, jid: String) -> Result<()> {
        let account = self.account as i64;
        let aliases = self.chat_aliases(&jid).await?;
        self.backend.shared().run(move |conn| {
            for jid in aliases {
            diesel::sql_query("UPDATE brazap_assets SET expires_at=0,updated_at=updated_at+1 WHERE account=? AND key IN (?,?)")
                .bind::<BigInt,_>(account).bind::<Text,_>(format!("avatar:{jid}")).bind::<Text,_>(format!("profile:{jid}")).execute(conn)
                .map_err(|e|whatsapp_rust::store::error::StoreError::Database(Box::new(e)))?;
            } Ok(())
        }).await?;
        Ok(())
    }
}

impl Service {
    pub async fn asset(&self, request: AssetRequest) -> Result<Option<Asset>> {
        let jid = match &request {
            AssetRequest::Avatar { jid, .. } | AssetRequest::Profile { jid } => Some(jid.as_str()),
            AssetRequest::Thumbnail { chat, .. } | AssetRequest::Image { chat, .. } => {
                Some(chat.as_str())
            }
            AssetRequest::Sticker { .. } => None,
        };
        if let Some(jid) = jid {
            self.archive.allow_chat(jid).await?;
        }
        let animated_sticker = match &request {
            AssetRequest::Thumbnail { chat, sender, id }
            | AssetRequest::Image { chat, sender, id } => {
                let raw = self
                    .archive
                    .raw_message(chat.clone(), sender.clone(), id.clone())
                    .await?;
                let base = crate::content::media_message(&raw);
                if let Some(sticker) = base.sticker_message.as_option() {
                    sticker.is_animated == Some(true)
                        || self
                            .archive
                            .media(chat.clone(), sender.clone(), id.clone())
                            .await?
                            .as_deref()
                            .is_some_and(animated_webp)
                } else {
                    false
                }
            }
            _ => false,
        };
        let key = request.key();
        let mut cached = self.archive.cached_asset(key.clone()).await?;
        if cached.as_ref().is_none_or(|c| c.data.is_none()) {
            if let AssetRequest::Avatar { jid, .. } | AssetRequest::Profile { jid } = &request {
                let prefix = if matches!(&request, AssetRequest::Profile { .. }) {
                    "profile"
                } else {
                    "avatar"
                };
                for alias in self.archive.chat_aliases(jid).await? {
                    if alias == *jid {
                        continue;
                    }
                    let candidate = self
                        .archive
                        .cached_asset(format!("{prefix}:{alias}"))
                        .await?;
                    if candidate.as_ref().is_some_and(|c| c.data.is_some()) {
                        cached = candidate;
                        break;
                    }
                }
            }
        }
        if let Some(cache) = &cached
            && cache.expires_at > now()
            && (!animated_sticker || cache.mime == "image/webp")
            && (cache.data.is_some() || !matches!(&request, AssetRequest::Thumbnail { .. }))
        {
            return Ok(cache.data.as_ref().map(|bytes| Asset {
                mime: cache.mime.clone(),
                data: STANDARD.encode(bytes),
            }));
        }
        let data = match &request {
            AssetRequest::Sticker { hash } => {
                let bytes = self.sticker_bytes(hash.clone()).await?;
                Some(if animated_webp(&bytes) {
                    bytes
                } else {
                    resize(bytes, 120).await?
                })
            }
            AssetRequest::Avatar { jid, .. } | AssetRequest::Profile { jid } => {
                let Ok(client) = self.connected().await else {
                    return Ok(cached.and_then(|c| {
                        c.data.map(|bytes| Asset {
                            mime: c.mime,
                            data: STANDARD.encode(bytes),
                        })
                    }));
                };
                let jid = jid.parse::<whatsapp_rust::Jid>()?;
                let common_group = match &request {
                    AssetRequest::Avatar {
                        common_group: Some(group),
                        ..
                    } => Some(group.parse::<whatsapp_rust::Jid>()?),
                    _ => None,
                };
                let target = if jid.is_group() {
                    ProfilePictureTarget::Group(&jid)
                } else {
                    ProfilePictureTarget::Contact(&jid)
                };
                let lookup = client
                    .pictures()
                    .lookup(
                        ProfilePictureRequest::new(
                            target,
                            if matches!(&request, AssetRequest::Profile { .. }) {
                                ProfilePictureType::Full
                            } else {
                                ProfilePictureType::Preview
                            },
                        )
                        .common_gid(common_group.as_ref())
                        .timeout(Some(Duration::from_secs(8))),
                    )
                    .await;
                let picture = match lookup {
                    Ok(outcome) => outcome.into_found(),
                    Err(error) => {
                        log::debug!("Avatar lookup unavailable: {error}");
                        let previous = cached.and_then(|cache| cache.data);
                        self.archive
                            .cache_asset(key, "image/png".into(), previous.clone(), 300)
                            .await?;
                        return Ok(previous.map(|bytes| Asset {
                            mime: "image/png".into(),
                            data: STANDARD.encode(bytes),
                        }));
                    }
                };
                if let Some(picture) = picture {
                    let bytes = tokio::task::spawn_blocking(move || -> Result<Vec<u8>> {
                        let agent: ureq::Agent = ureq::Agent::config_builder()
                            .timeout_global(Some(Duration::from_secs(10)))
                            .build()
                            .into();
                        Ok(agent
                            .get(&picture.url)
                            .call()?
                            .body_mut()
                            .with_config()
                            .limit(2 * 1024 * 1024)
                            .read_to_vec()?)
                    })
                    .await?;
                    let bytes = match bytes {
                        Ok(bytes) => bytes,
                        Err(error) => {
                            log::debug!("Avatar download unavailable: {error}");
                            let previous = cached.and_then(|c| c.data);
                            self.archive
                                .cache_asset(key, "image/png".into(), previous.clone(), 300)
                                .await?;
                            return Ok(previous.map(|bytes| Asset {
                                mime: "image/png".into(),
                                data: STANDARD.encode(bytes),
                            }));
                        }
                    };
                    Some(
                        resize(
                            bytes,
                            if matches!(&request, AssetRequest::Profile { .. }) {
                                512
                            } else {
                                96
                            },
                        )
                        .await?,
                    )
                } else {
                    None
                }
            }
            AssetRequest::Thumbnail { chat, sender, id } => {
                let proto = self
                    .archive
                    .raw_message(chat.clone(), sender.clone(), id.clone())
                    .await?;
                let base = crate::content::media_message(&proto);
                // An MP4 remains a video after download; its embedded JPEG is the poster.
                // Never feed the cached movie bytes into the image decoder.
                if let Some(video) = base.video_message.as_option() {
                    match video.jpeg_thumbnail.as_ref().filter(|b| !b.is_empty()) {
                        Some(bytes) => Some(resize(bytes.clone(), 360).await?),
                        None => {
                            #[cfg(feature = "desktop")]
                            if let Some(bytes) = self
                                .archive
                                .media(chat.clone(), sender.clone(), id.clone())
                                .await?
                            {
                                crate::playback::poster(bytes).await?
                            } else {
                                return Ok(None);
                            }
                            #[cfg(not(feature = "desktop"))]
                            return Ok(None);
                        }
                    }
                } else if let Some(bytes) = self
                    .archive
                    .media(chat.clone(), sender.clone(), id.clone())
                    .await?
                {
                    Some(if animated_webp(&bytes) {
                        bytes
                    } else {
                        resize(bytes, 360).await?
                    })
                } else {
                    let thumbnail = base
                        .image_message
                        .as_option()
                        .and_then(|m| m.jpeg_thumbnail.clone())
                        .or_else(|| {
                            base.video_message
                                .as_option()
                                .and_then(|m| m.jpeg_thumbnail.clone())
                        })
                        .or_else(|| {
                            base.sticker_message
                                .as_option()
                                .and_then(|m| m.png_thumbnail.clone())
                        });
                    if let Some(bytes) = thumbnail.filter(|b| !b.is_empty()) {
                        Some(resize(bytes, 360).await?)
                    } else {
                        return Ok(None);
                    }
                }
            }
            AssetRequest::Image { chat, sender, id } => {
                let proto = self
                    .archive
                    .raw_message(chat.clone(), sender.clone(), id.clone())
                    .await?;
                let message = Message {
                    id: id.clone(),
                    chat: chat.clone(),
                    sender: sender.clone(),
                    from_me: sender == "self",
                    timestamp: 0,
                    text: String::new(),
                    kind: "image".into(),
                    file_name: None,
                    mime: None,
                    status: String::new(),
                    has_media: false,
                    ..Default::default()
                };
                let bytes = self.download_media(&message, proto).await?;
                Some(if animated_sticker && animated_webp(&bytes) {
                    bytes
                } else {
                    resize(bytes, 1800).await?
                })
            }
        };
        let ttl = if matches!(
            request,
            AssetRequest::Avatar { .. } | AssetRequest::Profile { .. }
        ) {
            24 * 3600
        } else {
            365 * 24 * 3600
        };
        let mime = if data.as_deref().is_some_and(animated_webp) {
            "image/webp"
        } else {
            "image/png"
        };
        // Original animated bytes already live in the media/sticker archive.
        // Do not duplicate the entire animation in the thumbnail cache.
        if mime != "image/webp" {
            self.archive
                .cache_asset(key, mime.into(), data.clone(), ttl)
                .await?;
        }
        Ok(data.map(|bytes| Asset {
            mime: mime.into(),
            data: STANDARD.encode(bytes),
        }))
    }
}

pub(crate) fn animated_webp(bytes: &[u8]) -> bool {
    bytes.len() >= 30
        && &bytes[..4] == b"RIFF"
        && &bytes[8..12] == b"WEBP"
        && &bytes[12..16] == b"VP8X"
        && bytes[20] & 2 != 0
}

async fn resize(bytes: Vec<u8>, side: u32) -> Result<Vec<u8>> {
    tokio::task::spawn_blocking(move || {
        procwatch::alloc::tag_thread("image transforms");
        ensure!(
            bytes.len() <= 64 * 1024 * 1024,
            "Image is too large to preview"
        );
        let mut reader = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
        let mut limits = image::Limits::default();
        limits.max_alloc = Some(256 * 1024 * 1024);
        limits.max_image_width = Some(16384);
        limits.max_image_height = Some(16384);
        reader.limits(limits);
        let image = reader
            .decode()
            .context("Unsupported image format")?
            .thumbnail(side, side);
        let mut output = Cursor::new(Vec::new());
        image.write_to(&mut output, image::ImageFormat::Png)?;
        Ok(output.into_inner())
    })
    .await?
}

#[derive(QueryableByName)]
struct AvatarTarget {
    #[diesel(sql_type=Text)]
    jid: String,
    #[diesel(sql_type=Nullable<Text>)]
    common_group: Option<String>,
}
impl Service {
    /// Populate the durable small-avatar cache gradually; decoded UI images stay lazy.
    pub(crate) async fn warm_avatars(self: std::sync::Arc<Self>) -> Result<()> {
        use std::sync::atomic::{AtomicBool, Ordering};
        struct Guard<'a>(&'a AtomicBool);
        impl Drop for Guard<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::Relaxed);
            }
        }
        if self.avatar_warming.swap(true, Ordering::Relaxed) {
            return Ok(());
        }
        let _guard = Guard(&self.avatar_warming);
        let account = self.archive.account as i64;
        let targets = self.archive.backend.shared().read(move |conn| {
            diesel::sql_query("SELECT jid,MAX(common_group) common_group FROM (SELECT jid,NULL common_group FROM brazap_contacts WHERE account=? UNION ALL SELECT jid,NULL FROM brazap_chats WHERE account=? UNION ALL SELECT sender jid,chat common_group FROM brazap_messages WHERE account=? AND chat LIKE '%@g.us' AND sender!='self' GROUP BY sender) WHERE jid LIKE '%@s.whatsapp.net' OR jid LIKE '%@lid' OR jid LIKE '%@g.us' GROUP BY jid")
                .bind::<BigInt,_>(account).bind::<BigInt,_>(account).bind::<BigInt,_>(account).load::<AvatarTarget>(conn).map_err(crate::storage::db_error)
        }).await?;
        for target in targets {
            if self.connected().await.is_err() {
                break;
            }
            let key = format!("avatar:{}", target.jid);
            if self
                .archive
                .cached_asset(key)
                .await?
                .is_some_and(|c| c.expires_at > now())
            {
                continue;
            }
            let _ = self
                .asset(AssetRequest::Avatar {
                    jid: target.jid,
                    common_group: target.common_group,
                })
                .await;
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
        Ok(())
    }
}
