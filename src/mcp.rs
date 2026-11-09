use std::sync::Arc;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::service::Service;

#[derive(Default)]
pub struct Session {
    negotiated: bool,
    initialized: bool,
}

fn text_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .with_context(|| format!("{key} must be a string"))
}

pub async fn call(service: &Arc<Service>, name: &str, args: Value) -> Result<Value> {
    match name {
        "sticker_catalog" => Ok(json!({
            "stickers": service.sticker_catalog(args["sync"].as_bool().unwrap_or(false)).await?,
            "recent_emojis": service.archive.preference("recent_emojis".into()).await?.and_then(|value|serde_json::from_str::<Vec<String>>(&value).ok()).unwrap_or_default()
        })),
        "send_sticker" => Ok(
            json!({"message": service.send_sticker(text_arg(&args,"chat")?.into(),text_arg(&args,"hash")?.into()).await?}),
        ),
        "remember_emoji" => {
            let emoji = text_arg(&args, "emoji")?;
            anyhow::ensure!(emoji.chars().count() <= 16, "Invalid emoji");
            let mut recent = service
                .archive
                .preference("recent_emojis".into())
                .await?
                .and_then(|value| serde_json::from_str::<Vec<String>>(&value).ok())
                .unwrap_or_default();
            recent.retain(|item| item != emoji);
            recent.insert(0, emoji.into());
            recent.truncate(32);
            service
                .archive
                .set_preference("recent_emojis".into(), serde_json::to_string(&recent)?)
                .await?;
            Ok(json!({"saved":true}))
        }
        "get_status" => {
            let snapshot = service.snapshot(None).await?;
            Ok(json!({"phase":snapshot.connection.phase,"notice":snapshot.connection.notice}))
        }
        "list_chats" => Ok(json!({"chats":service.archive.chats().await?})),
        "list_statuses" => Ok(json!({"statuses":service.archive.statuses().await?})),
        "reply_status" => Ok(
            json!({"message":service.reply_status(text_arg(&args,"sender")?.into(),text_arg(&args,"id")?.into(),text_arg(&args,"text")?.into(),args["message_id"].as_str().map(str::to_owned).unwrap_or_else(crate::outbox::new_id)).await?}),
        ),
        "next_audio" => Ok(
            json!({"message":service.archive.next_audio(text_arg(&args,"chat")?.into(),serde_json::from_value(args["cursor"].clone())?).await?}),
        ),
        "view_status" => service
            .view_status(
                text_arg(&args, "sender")?.into(),
                text_arg(&args, "id")?.into(),
            )
            .await
            .map(|_| json!({"viewed":true})),
        "get_current_call" => Ok(json!({"call": service.live_call().await})),
        "start_call" => service
            .start_call(
                text_arg(&args, "chat")?.into(),
                args["video"].as_bool().unwrap_or(false),
            )
            .await
            .map(|call| json!({"call":call})),
        "answer_call" => service
            .answer_call(
                text_arg(&args, "id")?.into(),
                args["video"].as_bool().unwrap_or(false),
            )
            .await
            .map(|_| json!({"ok":true})),
        "end_call" => service
            .end_call(&text_arg(&args, "id")?)
            .await
            .map(|_| json!({"ok":true})),
        "dismiss_call" => service
            .dismiss_call(&text_arg(&args, "id")?)
            .await
            .map(|_| json!({"ok":true})),
        "mute_call" => service
            .mute_call(
                &text_arg(&args, "id")?,
                args["muted"].as_bool().unwrap_or(true),
            )
            .await
            .map(|_| json!({"ok":true})),
        "camera_call" => service
            .camera_call(
                &text_arg(&args, "id")?,
                args["enabled"].as_bool().unwrap_or(false),
            )
            .await
            .map(|_| json!({"ok":true})),
        "simulate_incoming_call" => service
            .demo_incoming_call(
                text_arg(&args, "chat")?.into(),
                args["video"].as_bool().unwrap_or(false),
            )
            .await
            .map(|call| json!({"call":call})),
        "list_calls" => Ok(json!({"calls":service.archive.calls().await?})),
        "read_messages" => {
            let chat = text_arg(&args, "chat")?.to_owned();
            let limit = args
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(100)
                .clamp(1, 500) as usize;
            let before = args.get("before").and_then(Value::as_i64);
            let cursor = args
                .get("cursor")
                .cloned()
                .map(serde_json::from_value)
                .transpose()?
                .or_else(|| {
                    before.map(|timestamp| crate::model::MessageCursor {
                        timestamp,
                        id: String::new(),
                        sender: String::new(),
                    })
                });
            Ok(serde_json::to_value(
                service.archive.message_page(chat, limit, cursor).await?,
            )?)
        }
        "prepare_media" => {
            service.prepare_media(
                text_arg(&args, "chat")?.into(),
                serde_json::from_value(args["items"].clone())?,
            );
            Ok(json!({"queued":true}))
        }
        "chat_action" => {
            let action =
                serde_json::from_value(args.get("action").cloned().unwrap_or(Value::Null))?;
            let enabled = args
                .get("enabled")
                .and_then(Value::as_bool)
                .context("enabled must be a boolean")?;
            service
                .chat_action(text_arg(&args, "chat")?.into(), action, enabled)
                .await?;
            Ok(json!({"updated":true}))
        }
        "chat_media" => Ok(serde_json::to_value(
            service
                .archive
                .media_page(
                    text_arg(&args, "chat")?.into(),
                    args.get("kind")
                        .and_then(Value::as_str)
                        .unwrap_or("media")
                        .into(),
                    args.get("cursor")
                        .filter(|v| !v.is_null())
                        .cloned()
                        .map(serde_json::from_value)
                        .transpose()?,
                )
                .await?,
        )?),
        "search" => Ok(serde_json::to_value(
            service
                .archive
                .search(
                    text_arg(&args, "query")?.into(),
                    args.get("chat").and_then(Value::as_str).map(str::to_owned),
                    args.get("cursor")
                        .filter(|v| !v.is_null())
                        .cloned()
                        .map(serde_json::from_value)
                        .transpose()?,
                )
                .await?,
        )?),
        "message_window" => Ok(serde_json::to_value(
            service
                .archive
                .message_window(
                    text_arg(&args, "chat")?.into(),
                    text_arg(&args, "id")?.into(),
                    text_arg(&args, "sender")?.into(),
                )
                .await?,
        )?),
        "newer_messages" => Ok(serde_json::to_value(
            service
                .archive
                .newer_message_page(
                    text_arg(&args, "chat")?.into(),
                    args["limit"].as_u64().unwrap_or(100).clamp(1, 500) as usize,
                    serde_json::from_value(args["cursor"].clone())?,
                )
                .await?,
        )?),
        "older_messages" => Ok(serde_json::to_value(
            service
                .older_messages(
                    text_arg(&args, "chat")?.into(),
                    serde_json::from_value(args["cursor"].clone())?,
                )
                .await?,
        )?),
        "send_message" => Ok(json!({"message":service.send_text_with_id(
            text_arg(&args,"chat")?.into(), text_arg(&args,"text")?.into(),
            args.get("reply_to").filter(|v|!v.is_null()).map(|v|serde_json::from_value(v.clone())).transpose()?,
            args.get("message_id").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(crate::outbox::new_id),
            args.get("timestamp").and_then(Value::as_i64),
        ).await?})),
        "react_message" => {
            service
                .react(
                    text_arg(&args, "chat")?.into(),
                    serde_json::from_value(args["target"].clone())?,
                    text_arg(&args, "emoji")?.into(),
                )
                .await?;
            Ok(json!({"updated":true}))
        }
        "forward_messages" => Ok(serde_json::to_value(
            service
                .forward(
                    text_arg(&args, "source")?.into(),
                    text_arg(&args, "destination")?.into(),
                    serde_json::from_value(args["targets"].clone())?,
                )
                .await?,
        )?),
        "send_voice" => {
            Ok(json!({"message":service.send_voice(serde_json::from_value(args)?).await?}))
        }
        "send_attachment" => Ok(
            json!({"message":service.send_attachment(text_arg(&args,"chat")?.into(),text_arg(&args,"path")?.into(),text_arg(&args,"kind")?.into(),args.get("caption").and_then(Value::as_str).unwrap_or("").into()).await?}),
        ),
        "save_attachment" => {
            service
                .save_attachment(
                    text_arg(&args, "chat")?.into(),
                    text_arg(&args, "sender")?.into(),
                    text_arg(&args, "id")?.into(),
                    text_arg(&args, "path")?.into(),
                )
                .await?;
            Ok(json!({"saved":true}))
        }
        "pair_phone" => {
            service.pair_phone(text_arg(&args, "phone")?.into()).await?;
            Ok(json!({"requested":true}))
        }
        "reconnect" => {
            service.reconnect().await;
            Ok(json!({"requested":true}))
        }
        "mark_read" => {
            service
                .mark_chat_read(text_arg(&args, "chat")?.into())
                .await?;
            Ok(json!({"updated":true}))
        }
        "open_chat" => {
            let address = text_arg(&args, "address")?.trim();
            let address = if address.contains('@') {
                address.to_owned()
            } else {
                let number = address
                    .trim_start_matches('+')
                    .replace([' ', '-', '(', ')'], "");
                anyhow::ensure!(
                    !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()),
                    "Enter a phone number with country code or a chat JID"
                );
                format!("{number}@s.whatsapp.net")
            };
            let jid: whatsapp_rust::Jid = address.parse()?;
            let address = service.archive.canonical_chat(&jid.to_string()).await?;
            service
                .archive
                .name_chat(address.clone(), String::new(), 0, crate::model::now())
                .await?;
            Ok(json!({"chat":address}))
        }
        _ => bail!("Unknown tool: {name}"),
    }
}

pub(crate) fn tools() -> Value {
    let definitions = [
        (
            "reply_status",
            "Reply privately to a status, quoting the original update in the author's chat.",
            json!({"sender":{"type":"string"},"id":{"type":"string"},"text":{"type":"string"},"message_id":{"type":"string"}}),
            vec!["sender", "id", "text"],
            false,
        ),
        (
            "list_statuses",
            "List unexpired status updates and their viewed state, excluding locked contacts.",
            json!({}),
            vec![],
            true,
        ),
        (
            "view_status",
            "Mark a status update viewed and send a read receipt when enabled.",
            json!({"sender":{"type":"string"},"id":{"type":"string"}}),
            vec!["sender", "id"],
            false,
        ),
        (
            "next_audio",
            "Find the next audio message in a conversation.",
            json!({"chat":{"type":"string"},"cursor":{"type":"object"}}),
            vec!["chat", "cursor"],
            true,
        ),
        (
            "get_status",
            "Check WhatsApp connection status.",
            json!({}),
            vec![],
            true,
        ),
        (
            "list_chats",
            "List locally stored WhatsApp chats, most recent first.",
            json!({}),
            vec![],
            true,
        ),
        (
            "get_current_call",
            "Read the active voice or video call.",
            json!({}),
            vec![],
            true,
        ),
        (
            "start_call",
            "Start a real one-to-one WhatsApp call using this computer's microphone and optional camera.",
            json!({"chat":{"type":"string"},"video":{"type":"boolean"}}),
            vec!["chat"],
            false,
        ),
        (
            "answer_call",
            "Answer the currently ringing call. Video enables this computer's camera.",
            json!({"id":{"type":"string"},"video":{"type":"boolean"}}),
            vec!["id"],
            false,
        ),
        (
            "end_call",
            "Decline or hang up a WhatsApp call.",
            json!({"id":{"type":"string"}}),
            vec!["id"],
            false,
        ),
        (
            "mute_call",
            "Mute or unmute the call microphone.",
            json!({"id":{"type":"string"},"muted":{"type":"boolean"}}),
            vec!["id", "muted"],
            false,
        ),
        (
            "camera_call",
            "Enable or disable the camera during a connected call.",
            json!({"id":{"type":"string"},"enabled":{"type":"boolean"}}),
            vec!["id", "enabled"],
            false,
        ),
        (
            "list_calls",
            "List locally stored WhatsApp call history, most recent first.",
            json!({}),
            vec![],
            true,
        ),
        (
            "search",
            "Search locally saved chat names, phone numbers, filenames, and message contents. Pass chat to search one conversation; pass next as cursor to load older matches.",
            json!({"query":{"type":"string","minLength":1},"chat":{"type":"string"},"cursor":{"type":"object","properties":{"timestamp":{"type":"integer"},"id":{"type":"string"},"sender":{"type":"string"}},"required":["timestamp","id","sender"]}}),
            vec!["query"],
            true,
        ),
        (
            "read_messages",
            "Read persisted messages chronologically. Pass the oldest message timestamp, id and sender as cursor to page without skipping same-second messages.",
            json!({"chat":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":500},"before":{"type":"integer","description":"Exclusive Unix timestamp; prefer cursor"},"cursor":{"type":"object","properties":{"timestamp":{"type":"integer"},"id":{"type":"string"},"sender":{"type":"string"}},"required":["timestamp","id","sender"]}}),
            vec!["chat"],
            true,
        ),
        (
            "send_message",
            "Send a WhatsApp text message to the exact chat JID. Reuse message_id to retry a failed send without changing its identity.",
            json!({"chat":{"type":"string"},"text":{"type":"string","minLength":1},"message_id":{"type":"string","maxLength":80},"reply_to":{"type":"object","properties":{"timestamp":{"type":"integer"},"id":{"type":"string"},"sender":{"type":"string"}},"required":["timestamp","id","sender"]}}),
            vec!["chat", "text"],
            false,
        ),
        (
            "react_message",
            "React to a saved message with an emoji, or an empty string to remove your reaction.",
            json!({"chat":{"type":"string"},"target":{"type":"object","properties":{"timestamp":{"type":"integer"},"id":{"type":"string"},"sender":{"type":"string"}},"required":["timestamp","id","sender"]},"emoji":{"type":"string"}}),
            vec!["chat", "target", "emoji"],
            false,
        ),
        (
            "forward_messages",
            "Forward selected saved messages to a person or group. Returns sent source identities and any partial failure; retry only unsent messages.",
            json!({"source":{"type":"string"},"destination":{"type":"string"},"targets":{"type":"array","minItems":1,"maxItems":50,"items":{"type":"object","properties":{"timestamp":{"type":"integer"},"id":{"type":"string"},"sender":{"type":"string"}},"required":["timestamp","id","sender"]}}}),
            vec!["source", "destination", "targets"],
            false,
        ),
        (
            "send_voice",
            "Send a local Ogg Opus recording as a WhatsApp voice message.",
            json!({"chat":{"type":"string"},"path":{"type":"string"},"duration_seconds":{"type":"integer","minimum":1,"maximum":600},"waveform":{"type":"array","maxItems":64,"items":{"type":"integer","minimum":0,"maximum":255}},"reply_to":{"type":"object","properties":{"timestamp":{"type":"integer"},"id":{"type":"string"},"sender":{"type":"string"}},"required":["timestamp","id","sender"]}}),
            vec!["chat", "path", "duration_seconds"],
            false,
        ),
        (
            "send_attachment",
            "Upload and send a local file as an image, video, audio file, WebP sticker, or document. Captions are supported for images, videos, and documents.",
            json!({"chat":{"type":"string"},"path":{"type":"string"},"kind":{"type":"string","enum":["image","video","audio","sticker","file"]},"caption":{"type":"string"}}),
            vec!["chat", "path", "kind"],
            false,
        ),
        (
            "sticker_catalog",
            "List your synced favorite and recent WhatsApp stickers.",
            json!({}),
            vec![],
            true,
        ),
        (
            "send_sticker",
            "Send a sticker from your WhatsApp sticker catalog.",
            json!({"chat":{"type":"string"},"hash":{"type":"string"}}),
            vec!["chat", "hash"],
            false,
        ),
        (
            "save_attachment",
            "Save a message attachment from SQLite or WhatsApp to a local path.",
            json!({"chat":{"type":"string"},"sender":{"type":"string"},"id":{"type":"string"},"path":{"type":"string"}}),
            vec!["chat", "sender", "id", "path"],
            false,
        ),
    ];
    Value::Array(definitions.into_iter().map(|(name,description,properties,required,read)|json!({
        "name":name,"description":description,"inputSchema":{"type":"object","properties":properties,"required":required},
        "annotations":{"readOnlyHint":read,"destructiveHint":false,"openWorldHint":!read}
    })).collect())
}

pub async fn handle(
    service: &Arc<Service>,
    request: Value,
    session: &mut Session,
) -> Option<Value> {
    let id = request.get("id").cloned();
    let method = request.get("method").and_then(Value::as_str).unwrap_or("");
    let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
    let error = |code: i32, message: String| json!({"jsonrpc":"2.0","id":id.clone().unwrap_or(Value::Null),"error":{"code":code,"message":message}});
    if request.get("jsonrpc").and_then(Value::as_str) != Some("2.0") || method.is_empty() {
        return Some(error(-32600, "Invalid JSON-RPC request".into()));
    }
    if method == "notifications/initialized" {
        session.initialized = session.negotiated;
        return None;
    }
    if method.starts_with("notifications/") {
        return None;
    }
    id.as_ref()?;
    let result: Result<Value> = match method {
        "initialize" => {
            session.negotiated = true;
            Ok(
                json!({"protocolVersion":"2025-06-18","serverInfo":{"name":"brazap","version":env!("CARGO_PKG_VERSION")},"capabilities":{"tools":{"listChanged":false}},"instructions":if service.config.demo {"Offline demo profile. All messages and actions are simulated locally in SQLite; no WhatsApp connection exists."} else {"Use exact chat JIDs from list_chats. Text and attachment sends deliver real WhatsApp messages."}}),
            )
        }
        "ping" => Ok(json!({})),
        "brazap/activate" => {
            service
                .activate_desktop(
                    params
                        .get("chat")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                )
                .await
        }
        "brazap/desktop" => Ok(service
            .poll_desktop(
                params["pid"].as_u64().unwrap_or(0) as u32,
                params["chat"].as_str().map(str::to_owned),
            )
            .await),
        "brazap/settings" => service.settings_view().await,
        "brazap/settings/save" => match serde_json::from_value(params) {
            Ok(settings) => service
                .save_settings(settings)
                .await
                .map(|revision| json!({"saved":true,"revision":revision})),
            Err(e) => Err(e.into()),
        },
        "brazap/privacy" => service.whatsapp_privacy().await.map(|v| json!(v)),
        "brazap/privacy/set" => match (text_arg(&params, "category"), text_arg(&params, "value")) {
            (Ok(category), Ok(value)) => service
                .set_whatsapp_privacy(category, value)
                .await
                .map(|_| json!({"saved":true})),
            _ => Err(anyhow::anyhow!("Invalid privacy choice")),
        },
        "brazap/cache/clear" => service
            .archive
            .clear_derived_cache()
            .await
            .map(|_| json!({"cleared":true})),
        "brazap/updates" => service
            .check_updates(params["download"].as_bool().unwrap_or(false))
            .await
            .map(|v| json!(v)),
        "brazap/logout" => service
            .request_logout()
            .await
            .map(|_| json!({"logged_out":true})),
        "brazap/typing" => match text_arg(&params, "chat") {
            Ok(chat) => service
                .typing(chat, params["active"].as_bool().unwrap_or(false))
                .await
                .map(|_| json!({})),
            Err(e) => Err(e),
        },
        "brazap/call/frame" => match text_arg(&params, "id") {
            Ok(id) => Ok(
                json!({"frame":service.call_preview(&id, params["local"].as_bool().unwrap_or(false), params["after"].as_u64().unwrap_or(0)).await}),
            ),
            Err(e) => Err(e),
        },
        "brazap/diagnostics" => service.diagnostics().await,
        "brazap/snapshot" => service
            .snapshot(
                params
                    .get("chat")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            )
            .await
            .and_then(|v| Ok(serde_json::to_value(v)?)),
        "brazap/asset" => match serde_json::from_value(params) {
            Ok(request) => service
                .asset(request)
                .await
                .and_then(|asset| Ok(serde_json::to_value(asset)?)),
            Err(error) => Err(error.into()),
        },
        "brazap/call" => match text_arg(&params, "name") {
            Ok(name) => {
                call(
                    service,
                    name,
                    params
                        .get("arguments")
                        .cloned()
                        .unwrap_or_else(|| json!({})),
                )
                .await
            }
            Err(e) => Err(e),
        },
        "tools/list" if session.initialized => Ok(json!({"tools":tools()})),
        "tools/call" if session.initialized => {
            let name = match text_arg(&params, "name") {
                Ok(name) => name,
                Err(e) => return Some(error(-32602, e.to_string())),
            };
            if !tools()
                .as_array()
                .is_some_and(|tools| tools.iter().any(|tool| tool["name"].as_str() == Some(name)))
            {
                return Some(error(-32602, format!("Unknown tool: {name}")));
            }
            match call(
                service,
                name,
                params
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({})),
            )
            .await
            {
                Ok(value) => Ok(
                    json!({"content":[{"type":"text","text":value.to_string()}],"structuredContent":value,"isError":false}),
                ),
                Err(e) => {
                    Ok(json!({"content":[{"type":"text","text":format!("{e:#}")}],"isError":true}))
                }
            }
        }
        "tools/list" | "tools/call" => {
            return Some(error(-32002, "Initialize this MCP session first".into()));
        }
        _ => return Some(error(-32601, format!("Method not found: {method}"))),
    };
    Some(match result {
        Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
        Err(e) => error(-32602, format!("{e:#}")),
    })
}
