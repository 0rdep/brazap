use super::{Command, State, view::*};
use crate::{
    config::Config,
    ipc::Remote,
    model::{Chat, Message, Phase},
};
use rune::{ClipboardContent, ClipboardImage, Color, El, Icon, Tx, col, el, row};
use serde_json::json;

pub(super) const PIN: Icon = Icon {
    name: "pinned-chat",
    view: 24.0,
    body: r#"<path d="m8 3 8 0-1 6 4 4v2H5v-2l4-4-1-6Zm4 12v7" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linejoin="round"/>"#,
};
pub(super) const MUTE: Icon = Icon {
    name: "muted-chat",
    view: 24.0,
    body: r#"<path d="M8 6a5 5 0 0 1 9 3v5l2 3H7M10 21h4M3 3l18 18" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round"/>"#,
};

pub(super) enum Target {
    Chat(Chat),
    Profile { jid: String, name: String },
    Message(Message),
}
pub(super) struct Open {
    target: Target,
    point: [f32; 2],
}

impl State {
    pub(super) fn chat_menu(&mut self, chat: Chat, p: rune::PointerPress) {
        self.media_menu = None;
        self.context_menu = Some(Open {
            target: Target::Chat(chat),
            point: [p.x, p.y],
        });
    }
    pub(super) fn profile_menu(&mut self, jid: String, name: String, p: rune::PointerPress) {
        self.media_menu = None;
        self.context_menu = Some(Open {
            target: Target::Profile { jid, name },
            point: [p.x, p.y],
        });
    }
    pub(super) fn message_menu(&mut self, message: Message, p: rune::PointerPress) {
        self.media_menu = None;
        self.context_menu = Some(Open {
            target: Target::Message(message),
            point: [p.x, p.y],
        });
    }
    pub(super) fn close_context_menu(&mut self) {
        self.context_menu = None;
        self.media_menu = None;
    }
    fn manage_chat(&mut self, name: &'static str, args: serde_json::Value) {
        if self.busy {
            return;
        }
        self.close_context_menu();
        self.busy = true;
        self.error = None;
        if self.tx.send(Command::ManageChat(name, args)).is_err() {
            self.busy = false;
            self.error = Some("The local service stopped".into());
        }
    }
    fn copy_media(&mut self, message: Message) {
        self.close_context_menu();
        self.copy_generation = self.copy_generation.wrapping_add(1);
        let _ = self
            .tx
            .send(Command::CopyMedia(self.copy_generation, message));
    }
}

fn item(
    name: &str,
    enabled: bool,
    danger: bool,
    action: impl Fn(&mut State, &mut Tx) + 'static,
) -> El {
    let item = label(
        super::locale::tr(name),
        12.0,
        if danger {
            Color {
                r: 0.95,
                g: 0.25,
                b: 0.25,
                a: 1.0,
            }
        } else {
            INK
        },
    )
    .key(format!("context-{name}"))
    .w_full()
    .px(12.0)
    .py(10.0)
    .radius(8.0)
    .role("menuitem")
    .label(super::locale::tr(name))
    .hover(Color::WHITE.alpha(0.065))
    .on_press_tx(action);
    if enabled { item } else { item.dim(0.4).inert() }
}

fn chat_items(s: &State, c: &Chat) -> El {
    let enabled = !s.busy && s.snapshot.connection.phase == Phase::Connected;
    let mut body = col().w(222.0).gap(2.0).padding(6.0);
    for (action, active, yes, no) in [
        ("pin", c.pinned, "Pin chat", "Unpin chat"),
        (
            "mute",
            c.muted,
            "Mute notifications",
            "Unmute notifications",
        ),
        ("archive", c.archived, "Archive chat", "Unarchive chat"),
    ] {
        let jid = c.jid.clone();
        body = body.child(item(
            if active { no } else { yes },
            enabled,
            false,
            move |s, _| {
                s.manage_chat(
                    "chat_action",
                    json!({"chat":jid,"action":action,"enabled":!active}),
                )
            },
        ));
    }
    let jid = c.jid.clone();
    let unread = c.unread > 0;
    body = body.child(item(
        if unread {
            "Mark as read"
        } else {
            "Mark as unread"
        },
        enabled,
        false,
        move |s, _| {
            if !unread && s.selected.as_deref() == Some(&jid) {
                s.drafts.insert(jid.clone(), s.draft.text.clone());
                s.selected = None;
                s.snapshot.selected_chat = None;
                s.snapshot.current_chat = None;
                s.timeline.clear();
                s.stop_voice();
                s.close_viewer();
                s.update_viewing();
                let _ = s.tx.send(Command::Deselect);
            }
            s.manage_chat(
                if unread { "mark_read" } else { "mark_unread" },
                json!({"chat":jid}),
            );
        },
    ));
    let chat = c.clone();
    body.child(el().w_full().h(1.0).my(4.0).fill(BORDER))
        .child(item("Delete chat…", enabled, true, move |s, _| {
            s.close_context_menu();
            s.delete_chat = Some(chat.clone());
        }))
}

fn profile_items(jid: &str, name: &str) -> El {
    let mut body = col().w(222.0).gap(2.0).padding(6.0);
    let target = jid.to_owned();
    body = body.child(item("Copy image", true, false, move |s, _| {
        s.close_context_menu();
        s.copy_generation = s.copy_generation.wrapping_add(1);
        let _ =
            s.tx.send(Command::CopyProfile(s.copy_generation, target.clone()));
    }));
    let (target, name) = (jid.to_owned(), name.to_owned());
    body.child(item("Download image", true, false, move |s, tx| {
        s.close_context_menu();
        let (target, stem) = (target.clone(), name.clone());
        let file: String = stem
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let options = rune::FileDialogOptions {
            title: "Save profile picture".into(),
            file_name: Some(format!(
                "{}.png",
                if file.trim_matches('_').is_empty() {
                    "profile"
                } else {
                    &file
                }
            )),
            ..Default::default()
        };
        tx.save_file(options, move |s: &mut State, result, _| match result {
            Ok(Some(path)) => {
                let _ = s.tx.send(Command::SaveProfile(target.clone(), path));
            }
            Ok(None) => {}
            Err(error) => s.error = Some(error.to_string()),
        });
    }))
}

fn message_items(_s: &State, m: &Message) -> El {
    let mut body = col().w(222.0).gap(2.0).padding(6.0);
    if !m.text.is_empty() {
        let text = m.text.clone();
        body = body.child(item("Copy text", true, false, move |s, tx| {
            s.close_context_menu();
            s.copy_generation = s.copy_generation.wrapping_add(1);
            tx.copy_rich(ClipboardContent::new(text.clone()));
        }));
    }
    if m.attachment() {
        let target = m.clone();
        body = body.child(item(
            match m.kind.as_str() {
                "image" | "sticker" => "Copy image",
                "video" => "Copy video",
                "audio" => "Copy audio",
                _ => "Copy file",
            },
            true,
            false,
            move |s, _| s.copy_media(target.clone()),
        ));
        let target = m.clone();
        body = body.child(item("Download media", true, false, move |s, tx| {
            s.close_context_menu();
            save_file(s, tx, target.clone());
        }));
    }
    let reply = m.clone();
    body = body.child(item(
        "Reply to message",
        m.actionable(),
        false,
        move |s, tx| {
            s.close_context_menu();
            s.reply = Some(reply.clone());
            s.defer_composer_focus(tx);
        },
    ));
    let forward = m.clone();
    body.child(item(
        "Forward message",
        m.actionable(),
        false,
        move |s, _| {
            s.close_context_menu();
            s.begin_forward(forward.clone());
        },
    ))
}

pub(super) fn menu(s: &State) -> Vec<El> {
    let (body, point, anchor) = if let Some(open) = &s.context_menu {
        (
            match &open.target {
                Target::Chat(c) => chat_items(s, c),
                Target::Profile { jid, name } => profile_items(jid, name),
                Target::Message(m) => message_items(s, m),
            },
            Some(open.point),
            None,
        )
    } else if let Some((key, m)) = &s.media_menu {
        (message_items(s, m), None, Some(format!("options-{key}")))
    } else {
        return Vec::new();
    };
    let popup = if let Some([x, y]) = point {
        let height = 300.0;
        glass(body).absolute(
            x.clamp(8.0, (s.viewport[0] - 242.0).max(8.0)),
            y.clamp(8.0, (s.viewport[1] - height).max(8.0)),
        )
    } else {
        rune_kit::popover::anchored(
            kit(),
            rune_kit::popover::raised(kit(), body, 12.0),
            anchor.as_deref().unwrap(),
            rune_kit::Placement::BottomEnd,
            6.0,
        )
    };
    vec![
        el().absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .on_press(|s: &mut State| s.close_context_menu()),
        popup.key("context-menu").role("menu"),
    ]
}

pub(super) fn confirmation(s: &State) -> Option<El> {
    let chat = s.delete_chat.as_ref()?.clone();
    Some(el().absolute(0.0,0.0).w_full().h_full().center().fill(Color::BLACK.alpha(0.6)).backdrop_blur(14.0)
        .role("dialog").label(super::locale::tr("Delete chat"))
        .child(glass(col().w(430.0).padding(24.0).gap(18.0)
            .child(title(super::locale::format("Delete chat with {name}?",&[("name",chat.name.clone())]),19.0).wrap())
            .child(label(super::locale::tr("This removes the chat and its messages from Brazap and syncs the deletion to your linked devices. It does not delete messages for other participants."),13.0,MUTED).wrap())
            .child(row().gap(12.0)
                .child(button("Cancel",|s,_|s.delete_chat=None))
                .child(button("Delete chat",move |s,_|{
                    if s.attachments.is_some() || s.recording.is_some() {s.error=Some("Finish or discard attachments and recordings before deleting a chat.".into());return;}
                    s.delete_chat=None;s.manage_chat("delete_chat",json!({"chat":chat.jid}));
                }))))))
}

/// The profile picture as PNG bytes: the cached 512-point copy the profile view shows.
async fn profile_png(remote: &Remote, jid: String) -> anyhow::Result<Vec<u8>> {
    use anyhow::Context;
    remote
        .asset(crate::assets::AssetRequest::Profile { jid })
        .await?
        .context("This profile has no picture")
}

pub(super) async fn copy_profile(remote: &Remote, jid: String) -> anyhow::Result<ClipboardContent> {
    use anyhow::Context;
    let png = profile_png(remote, jid).await?;
    let image = ClipboardImage::from_png(png).context("Image is too large for the clipboard")?;
    Ok(ClipboardContent::default().with_image(image))
}

pub(super) async fn save_profile(
    remote: &Remote,
    jid: String,
    path: std::path::PathBuf,
) -> anyhow::Result<()> {
    let png = profile_png(remote, jid).await?;
    tokio::fs::write(path, png).await?;
    Ok(())
}

pub(super) async fn prepare_copy(
    remote: &Remote,
    config: &Config,
    message: Message,
) -> anyhow::Result<ClipboardContent> {
    use anyhow::Context;
    if matches!(message.kind.as_str(), "image" | "sticker") {
        let file = tempfile::Builder::new()
            .prefix("brazap-copy-")
            .tempfile()?
            .into_temp_path();
        remote.call("save_attachment",json!({"chat":message.chat,"sender":message.sender,"id":message.id,"path":file.to_string_lossy()})).await?;
        let bytes = tokio::fs::read(&file).await?;
        tokio::task::spawn_blocking(move || {
            let image = image::load_from_memory(&bytes)?;
            let mut png = std::io::Cursor::new(Vec::new());
            image.write_to(&mut png, image::ImageFormat::Png)?;
            let image = ClipboardImage::from_png(png.into_inner())
                .context("Image is too large for the clipboard")?;
            Ok(ClipboardContent::default().with_image(image))
        })
        .await?
    } else {
        // Clipboard file references must remain valid after the player or app closes.
        // Originals remain in SQLite; these are disposable clipboard exports.
        let dir = config.cache_dir().join("clipboard");
        tokio::fs::create_dir_all(&dir).await?;
        let extension = message
            .file_name
            .as_deref()
            .and_then(|name| std::path::Path::new(name).extension())
            .and_then(|ext| ext.to_str())
            .filter(|ext| ext.len() <= 16 && ext.chars().all(|c| c.is_ascii_alphanumeric()))
            .or_else(|| {
                message
                    .mime
                    .as_deref()
                    .and_then(mime_guess::get_mime_extensions_str)
                    .and_then(|extensions| extensions.first().copied())
            })
            .unwrap_or(match message.kind.as_str() {
                "video" => "mp4",
                "audio" => "ogg",
                _ => "bin",
            });
        let file = dir.join(format!("{}.{extension}", uuid::Uuid::new_v4()));
        if let Err(error)=remote.call("save_attachment",json!({"chat":message.chat,"sender":message.sender,"id":message.id,"path":file.to_string_lossy()})).await {
            let _=tokio::fs::remove_file(&file).await;return Err(error);
        }
        Ok(ClipboardContent::default().with_files([file]))
    }
}
