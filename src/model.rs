use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Connection {
    pub phase: Phase,
    pub qr: Option<String>,
    pub qr_expires_at: Option<i64>,
    pub pairing_code: Option<String>,
    pub code_expires_at: Option<i64>,
    pub pair_retry_at: Option<i64>,
    pub notice: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    #[default]
    Starting,
    Pairing,
    Connected,
    Reconnecting,
    LoggedOut,
    Error,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, diesel::QueryableByName)]
pub struct Chat {
    #[serde(default)]
    #[diesel(sql_type=diesel::sql_types::Text, deserialize_as=String)]
    pub aliases: ChatAliases,
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub jid: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub name: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub preview: String,
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    pub updated_at: i64,
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    pub unread: i64,
    #[serde(default)]
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub phone: String,
    #[serde(default)]
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    pub photo_version: i64,
    #[serde(default)]
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub preview_kind: String,
    #[serde(default)]
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub preview_status: String,
    #[serde(default)]
    #[diesel(sql_type = diesel::sql_types::Bool)]
    pub preview_from_me: bool,
    #[serde(default)]
    #[diesel(sql_type = diesel::sql_types::Bool)]
    pub muted: bool,
    #[serde(default)]
    #[diesel(sql_type = diesel::sql_types::Bool)]
    pub pinned: bool,
    #[serde(default)]
    #[diesel(sql_type = diesel::sql_types::Bool)]
    pub archived: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ChatAliases(pub Vec<String>);
impl From<String> for ChatAliases {
    fn from(value: String) -> Self {
        Self(serde_json::from_str(&value).unwrap_or_default())
    }
}
impl std::ops::Deref for ChatAliases {
    type Target = [String];
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl Chat {
    pub fn matches_jid(&self, jid: &str) -> bool {
        self.jid == jid || self.aliases.iter().any(|a| a == jid)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Call {
    pub id: String,
    pub peer: String,
    pub name: String,
    pub timestamp: Option<i64>,
    pub from_me: Option<bool>,
    pub video: bool,
    pub duration: Option<i64>,
    pub outcome: String,
    #[serde(default)]
    pub photo_version: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Message {
    #[serde(default)]
    pub deleted_at: Option<i64>,
    pub id: String,
    pub chat: String,
    pub sender: String,
    #[serde(default)]
    pub sender_name: String,
    #[serde(default)]
    pub sender_phone: String,
    #[serde(default)]
    pub sender_photo_version: i64,
    pub from_me: bool,
    pub timestamp: i64,
    pub text: String,
    pub kind: String,
    pub file_name: Option<String>,
    pub mime: Option<String>,
    pub status: String,
    pub has_media: bool,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub footer: String,
    #[serde(default)]
    pub actions: Vec<MessageAction>,
    #[serde(default)]
    pub quote: Option<Quote>,
    #[serde(default)]
    pub forwarded: bool,
    #[serde(default)]
    pub reactions: Vec<Reaction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reaction_update: Option<ReactionUpdate>,
    #[serde(default)]
    pub duration_seconds: Option<u32>,
    #[serde(default)]
    pub waveform: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Quote {
    pub id: String,
    pub sender: String,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reaction {
    pub sender: String,
    pub emoji: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReactionUpdate {
    pub target_sender: String,
    pub target_id: String,
    pub emoji: String,
    pub timestamp_ms: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ForwardResult {
    pub destination: String,
    pub sent: Vec<MessageCursor>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MessageAction {
    pub label: String,
    pub target: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MessageCursor {
    pub timestamp: i64,
    pub id: String,
    pub sender: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VoiceUpload {
    pub chat: String,
    pub path: String,
    pub duration_seconds: u32,
    #[serde(default)]
    pub waveform: Vec<u8>,
    #[serde(default)]
    pub reply_to: Option<MessageCursor>,
}

impl Message {
    pub fn is_call(&self) -> bool {
        matches!(self.kind.as_str(), "call" | "voice_call" | "video_call")
    }
    pub fn cursor(&self) -> MessageCursor {
        MessageCursor {
            timestamp: self.timestamp,
            id: self.id.clone(),
            sender: self.sender.clone(),
        }
    }
    pub fn attachment(&self) -> bool {
        matches!(
            self.kind.as_str(),
            "image" | "video" | "audio" | "file" | "sticker"
        )
    }
    pub fn visible(&self) -> bool {
        self.kind != "protocol"
    }
    pub fn actionable(&self) -> bool {
        !matches!(
            self.kind.as_str(),
            "protocol" | "placeholder" | "unsupported" | "call" | "voice_call" | "video_call"
        )
    }
    pub fn preview(&self) -> String {
        let name = self
            .file_name
            .as_deref()
            .filter(|name| !name.trim().is_empty());
        let fallback = match self.kind.as_str() {
            "image" => "Photo",
            "video" => "Video",
            "audio" => "Voice message",
            "file" => "Document",
            "sticker" => "Sticker",
            "placeholder" => "Message unavailable",
            "template" => "Business message",
            "contact" => "Contact",
            "location" => "Location",
            _ => "Message",
        };
        let body = if let Some(name) = name {
            if self.text.trim().is_empty() {
                name.to_owned()
            } else {
                format!("{name} · {}", self.text)
            }
        } else if !self.text.trim().is_empty() {
            self.text.clone()
        } else if !self.title.is_empty() {
            self.title.clone()
        } else {
            fallback.into()
        };
        body.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(240)
            .collect()
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MessagePage {
    pub messages: Vec<Message>,
    pub has_older: bool,
    #[serde(default)]
    pub has_newer: bool,
    #[serde(default)]
    pub notice: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchHit {
    pub chat: Chat,
    pub message: Message,
    pub excerpt: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SearchResults {
    pub chats: Vec<Chat>,
    pub messages: Vec<SearchHit>,
    #[serde(default)]
    pub next: Option<MessageCursor>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    #[serde(default)]
    pub settings: crate::settings::Settings,
    pub connection: Connection,
    pub chats: Vec<Chat>,
    pub messages: Vec<Message>,
    pub selected_chat: Option<String>,
    pub revision: u64,
    #[serde(default)]
    pub calls: Vec<Call>,
    #[serde(default)]
    pub live_call: Option<crate::calling::LiveCall>,
    #[serde(default)]
    pub current_chat: Option<Chat>,
}

pub fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MediaPage {
    pub items: Vec<Message>,
    pub next: Option<MessageCursor>,
}
