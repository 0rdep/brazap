pub mod assets;
pub mod calls;
pub mod chat;
pub(crate) mod chat_sync_policy;
pub mod config;
pub mod content;
pub(crate) mod demo;
pub mod diagnostics;
pub mod history;
pub(crate) mod history_prefetch;
pub(crate) mod identity;
pub mod ipc;
pub mod logging;
pub mod mcp;
pub(crate) mod media_recovery;
pub mod message_actions;
pub mod model;
pub mod notifications;
#[cfg(feature = "desktop")]
pub(crate) mod playback;
pub mod search;
pub mod service;
pub mod settings;
pub mod stickers;
pub mod storage;
pub mod storage_usage;
pub mod updates;

#[cfg(all(target_os = "windows", feature = "desktop"))]
pub mod windows_integration;

#[cfg(feature = "desktop")]
pub mod ui;
