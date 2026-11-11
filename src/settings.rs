use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

use crate::{service::Service, storage::Archive};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub notifications: NotificationSettings,
    pub privacy: PrivacySettings,
    pub appearance: AppearanceSettings,
    pub storage: StorageSettings,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            notifications: Default::default(),
            privacy: Default::default(),
            appearance: Default::default(),
            storage: Default::default(),
        }
    }
}
/// Retention choices in days; `0` keeps downloaded media forever.
pub const RETENTION_CHOICES: [u32; 7] = [1, 3, 7, 14, 30, 90, 0];
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StorageSettings {
    pub retention: RetentionSettings,
}
/// How long downloaded media stays on this device after it was last opened.
/// Messages and their raw data are never removed; removed media is downloaded
/// again when opened, while WhatsApp (or the phone) still has it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RetentionSettings {
    pub photos: u32,
    pub videos: u32,
    pub stickers: u32,
    pub audio: u32,
    pub files: u32,
}
impl Default for RetentionSettings {
    fn default() -> Self {
        Self {
            photos: 30,
            videos: 7,
            stickers: 0,
            audio: 0,
            files: 0,
        }
    }
}
impl RetentionSettings {
    /// Message kinds (as stored in `payload.kind`) with their retention in days.
    pub fn by_kind(&self) -> [(&'static str, u32); 5] {
        [
            ("image", self.photos),
            ("video", self.videos),
            ("sticker", self.stickers),
            ("audio", self.audio),
            ("file", self.files),
        ]
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NotificationSettings {
    pub enabled: bool,
    pub sound: bool,
    pub preview: bool,
}
impl Default for NotificationSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            sound: true,
            preview: true,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PrivacySettings {
    pub retain_deleted: bool,
    pub send_read_receipts: bool,
    pub send_typing: bool,
    pub show_online: bool,
}
impl Default for PrivacySettings {
    fn default() -> Self {
        Self {
            retain_deleted: true,
            send_read_receipts: true,
            send_typing: true,
            show_online: true,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppearanceSettings {
    pub font: String,
    pub font_size: u8,
    pub language: String,
}
impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            font: "system".into(),
            font_size: 13,
            language: "en".into(),
        }
    }
}
impl Settings {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (11..=19).contains(&self.appearance.font_size),
            "Font size must be between 11 and 19"
        );
        ensure!(
            ["system", "Inter", "Segoe UI", "Arial", "monospace"]
                .contains(&self.appearance.font.as_str()),
            "Unsupported font"
        );
        ensure!(
            ["en", "pt-BR", "es"].contains(&self.appearance.language.as_str()),
            "Unsupported language"
        );
        for (_, days) in self.storage.retention.by_kind() {
            ensure!(
                RETENTION_CHOICES.contains(&days),
                "Unsupported media retention"
            );
        }
        Ok(())
    }
}

impl Archive {
    pub async fn app_settings(&self) -> Result<Settings> {
        self.preference("app_settings".into())
            .await?
            .map(|s| serde_json::from_str(&s).context("Read app settings"))
            .transpose()
            .map(|s| s.unwrap_or_default())
    }
}

impl Service {
    pub async fn settings_view(&self) -> Result<Value> {
        Ok(
            json!({"settings": self.settings.read().await.clone(), "revision": self.revision(), "storage": self.archive.storage_usage(&self.config).await?, "updates": self.updates.read().await.clone()}),
        )
    }
    pub async fn save_settings(&self, value: Settings) -> Result<u64> {
        value.validate()?;
        self.archive
            .set_preference("app_settings".into(), serde_json::to_string(&value)?)
            .await?;
        *self.settings.write().await = value.clone();
        // Presence follows the window focus and this setting on the next desktop poll.
        self.changed();
        Ok(self.revision())
    }
    pub async fn whatsapp_privacy(&self) -> Result<BTreeMap<String, String>> {
        if self.config.demo {
            return self
                .archive
                .preference("demo_privacy".into())
                .await?
                .map(|value| serde_json::from_str(&value).map_err(Into::into))
                .unwrap_or_else(|| {
                    Ok([
                        ("last", "contacts"),
                        ("online", "match_last_seen"),
                        ("profile", "contacts"),
                        ("groupadd", "contacts"),
                        ("readreceipts", "all"),
                    ]
                    .into_iter()
                    .map(|(k, v)| (k.into(), v.into()))
                    .collect())
                });
        }
        let response = self.connected().await?.fetch_privacy_settings().await?;
        Ok(response
            .settings
            .into_iter()
            .map(|s| (s.category.as_str().to_owned(), s.value.as_str().to_owned()))
            .collect())
    }
    pub async fn set_whatsapp_privacy(&self, category: &str, value: &str) -> Result<()> {
        use whatsapp_rust::wacore::iq::privacy::{PrivacyCategory, PrivacyValue};
        let category: PrivacyCategory = category.into();
        let value: PrivacyValue = value.into();
        ensure!(
            !matches!(category, PrivacyCategory::Other(_))
                && !matches!(value, PrivacyValue::Other(_))
                && category.is_valid_value(&value),
            "Unsupported WhatsApp privacy setting"
        );
        ensure!(
            !matches!(value, PrivacyValue::ContactBlacklist),
            "Manage contact exclusions on your phone; existing lists are preserved"
        );
        if self.config.demo {
            let mut settings = self.whatsapp_privacy().await?;
            settings.insert(category.as_str().into(), value.as_str().into());
            self.archive
                .set_preference("demo_privacy".into(), serde_json::to_string(&settings)?)
                .await?;
            self.changed();
            return Ok(());
        }
        self.connected()
            .await?
            .set_privacy_setting(category, value)
            .await?;
        self.changed();
        Ok(())
    }
    pub async fn typing(&self, chat: &str, active: bool) -> Result<()> {
        if self.config.demo {
            return Ok(());
        }
        if !self.settings.read().await.privacy.send_typing {
            return Ok(());
        }
        let client = self.connected().await?;
        let jid = chat.parse()?;
        if active {
            client.chatstate().send_composing(&jid).await?;
        } else {
            client.chatstate().send_paused(&jid).await?;
        }
        Ok(())
    }
    pub async fn request_logout(&self) -> Result<()> {
        if self.config.demo {
            self.stop_calls().await;
            *self.connection.write().await = crate::model::Connection::default();
            self.changed();
            tokio::time::sleep(std::time::Duration::from_millis(350)).await;
            *self.connection.write().await = crate::demo::pairing();
            self.changed();
            return Ok(());
        }
        let _ = self.connected().await?;
        self.stop_calls().await;
        let (send, receive) = tokio::sync::oneshot::channel();
        {
            let mut pending = self.logout_request.lock().await;
            ensure!(
                !self
                    .logout_in_progress
                    .swap(true, std::sync::atomic::Ordering::AcqRel),
                "Logout is already in progress"
            );
            *pending = Some(send);
        }
        *self.connection.write().await = crate::model::Connection {
            phase: crate::model::Phase::LoggedOut,
            ..Default::default()
        };
        self.changed();
        self.restart.notify_one();
        receive
            .await
            .context("The connection stopped during logout")?
            .map_err(anyhow::Error::msg)
    }
}
