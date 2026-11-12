//! The local attachment draft. Clipboard reads, file drops, preparation and sending are
//! separate: nothing reaches WhatsApp until the user confirms this preview.
use std::{io::Write, path::PathBuf};

use anyhow::{Context, Result, ensure};
use rune::{
    ClipboardContent, ClipboardImage, Color, El, Image, SurfaceFit, TextEdit, Tx, col, el, icon,
    row,
};
use rune_kit::glyphs;

use super::{Command, Section, State, locale, view::*};
use crate::model::{Message, Phase};

const MAX_FILES: usize = 20;
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;

pub(super) enum Source {
    File(PathBuf, bool),
    Image(ClipboardImage),
}

pub(super) struct Prepared {
    pub path: PathBuf,
    pub name: String,
    pub kind: &'static str,
    pub size: u64,
    pub pixels: Option<image::RgbaImage>,
    _temporary: Option<tempfile::TempPath>,
}

pub(super) struct Item {
    id: u64,
    source: Option<PathBuf>,
    clipboard_bytes: usize,
    kind: &'static str,
    prepared: Option<Prepared>,
    pub image: Option<Image>,
    pub caption: TextEdit,
    error: Option<String>,
}

pub(super) struct Panel {
    pub chat: String,
    generation: u64,
    pub items: Vec<Item>,
    pub selected: usize,
    pub sending: bool,
    sent: usize,
    total: usize,
}

impl Panel {
    pub(super) fn caption_available(&self) -> bool {
        !self.sending
            && self
                .items
                .get(self.selected)
                .is_some_and(|item| item.kind != "audio")
    }
}

pub(super) struct Upload {
    pub id: u64,
    pub path: String,
    pub kind: String,
    pub caption: String,
}

fn classify(path: &std::path::Path, document: bool) -> &'static str {
    if document {
        return "file";
    }
    match mime_guess::from_path(path)
        .first_or_octet_stream()
        .type_()
        .as_str()
    {
        "image" => "image",
        "video" => "video",
        "audio" => "audio",
        _ => "file",
    }
}

fn prepare_file(source: Source) -> Result<Prepared> {
    let (path, document, temporary, name) = match source {
        Source::File(path, document) => {
            ensure!(path.is_absolute(), "Choose a local file to attach");
            let name = path
                .file_name()
                .context("Missing file name")?
                .to_string_lossy()
                .into_owned();
            (path, document, None, name)
        }
        Source::Image(image) => {
            let mut file = tempfile::Builder::new()
                .prefix("brazap-pasted-")
                .suffix(".png")
                .tempfile()?;
            file.write_all(image.png())?;
            file.flush()?;
            let path = file.into_temp_path();
            (
                path.to_path_buf(),
                false,
                Some(path),
                "clipboard-image.png".into(),
            )
        }
    };
    let metadata = std::fs::metadata(&path).context("Cannot open attachment")?;
    ensure!(
        metadata.is_file(),
        "Folders cannot be attached. Choose files instead."
    );
    ensure!(metadata.len() > 0, "Empty files cannot be attached");
    ensure!(
        metadata.len() <= MAX_FILE_BYTES,
        "Choose a file smaller than 64 MiB"
    );
    let kind = classify(&path, document);
    // Decode only small, bounded previews, on the preparation worker. The
    // original file stays on disk and is read by the shared service at send time.
    let pixels = if kind == "image" {
        let thumbnail = || -> Result<image::RgbaImage> {
            let mut reader = image::ImageReader::open(&path)?.with_guessed_format()?;
            let mut limits = image::Limits::default();
            limits.max_alloc = Some(128 * 1024 * 1024);
            limits.max_image_width = Some(16384);
            limits.max_image_height = Some(16384);
            reader.limits(limits);
            Ok(reader.decode()?.thumbnail(480, 320).into_rgba8())
        };
        thumbnail().ok()
    } else {
        None
    };
    Ok(Prepared {
        path,
        name,
        kind,
        size: metadata.len(),
        pixels,
        _temporary: temporary,
    })
}

pub(super) async fn prepare(source: Source) -> Result<Prepared> {
    let mut prepared = tokio::task::spawn_blocking(move || prepare_file(source)).await??;
    if prepared.kind == "video" {
        let bytes = tokio::fs::read(&prepared.path).await?;
        if bytes.len() as u64 <= MAX_FILE_BYTES
            && let Ok(Some(poster)) = crate::playback::poster(bytes).await
        {
            prepared.pixels = image::load_from_memory(&poster)
                .ok()
                .map(|image| image.thumbnail(480, 320).into_rgba8());
        }
    }
    Ok(prepared)
}

impl State {
    fn chat_content_active(&self) -> bool {
        self.selected.is_some()
            && self.section == Section::Messages
            && !self.preferences.open
            && !self.details
            && self.contact.is_none()
            && self.viewer.is_none()
            && self.forwarding.is_none()
            && !self.diagnostics.open
    }

    fn can_drop_attachments(&self) -> bool {
        self.chat_content_active()
            && self.authenticated
            && self.recording.is_none()
            && !self.busy
            && !self.attachments.as_ref().is_some_and(|panel| panel.sending)
    }

    fn drop_chat_files(&mut self, files: &[PathBuf]) {
        self.attachment_drag_over = false;
        if self.can_drop_attachments() {
            self.queue_attachments(
                files
                    .iter()
                    .cloned()
                    .map(|path| Source::File(path, false))
                    .collect(),
            );
        }
    }

    pub(super) fn paste_attachments(&mut self, content: &ClipboardContent) -> bool {
        let sources = if !content.files().is_empty() {
            content
                .files()
                .iter()
                .cloned()
                .map(|path| Source::File(path, false))
                .collect()
        } else if let Some(image) = content.image() {
            vec![Source::Image(image.clone())]
        } else {
            return false;
        };
        self.queue_attachments(sources);
        true
    }

    pub(super) fn paste_chat(&mut self, content: &ClipboardContent) {
        if !self.chat_content_active() {
            return;
        }
        if !self.paste_attachments(content) && !content.text().is_empty() {
            if let Some(panel) = &mut self.attachments {
                if !panel.sending
                    && let Some(item) = panel.items.get_mut(panel.selected)
                    && item.kind != "audio"
                {
                    item.caption.insert(content.text());
                }
            } else if self.recording.is_none() && !self.busy {
                self.draft.insert(content.text());
            }
        }
    }

    pub(super) fn queue_attachments(&mut self, sources: Vec<Source>) {
        if sources.is_empty() || !self.authenticated || self.selected.is_none() {
            return;
        }
        if self.recording.is_some()
            || self.forwarding.is_some()
            || self.busy
            || self.attachments.as_ref().is_some_and(|p| p.sending)
        {
            self.error = Some("Finish the current action before attaching files.".into());
            return;
        }
        if self.attachments.as_ref().map_or(0, |p| p.items.len()) + sources.len() > MAX_FILES {
            self.error = Some("You can attach up to 20 files at a time.".into());
            return;
        }
        let clipboard_bytes: usize = sources
            .iter()
            .map(|source| match source {
                Source::Image(image) => image.png().len(),
                _ => 0,
            })
            .sum();
        let pending_bytes: usize = self
            .attachments
            .iter()
            .flat_map(|p| p.items.iter())
            .map(|i| i.clipboard_bytes)
            .sum();
        if clipboard_bytes.saturating_add(pending_bytes) > 64 * 1024 * 1024 {
            self.error =
                Some("Too many clipboard images are being prepared. Try again shortly.".into());
            return;
        }
        self.draft.focused = false;
        self.close_viewer();
        self.picker.open = false;
        self.attachments_open = false;
        self.error = None;
        if self.attachments.is_none() {
            self.attachment_generation = self.attachment_generation.wrapping_add(1);
            self.attachments = Some(Panel {
                chat: self.selected.clone().unwrap(),
                generation: self.attachment_generation,
                items: Vec::new(),
                selected: 0,
                sending: false,
                sent: 0,
                total: 0,
            });
        }
        let panel = self.attachments.as_mut().unwrap();
        for source in sources {
            let path = match &source {
                Source::File(path, _) => Some(path.clone()),
                _ => None,
            };
            if path.is_some() && panel.items.iter().any(|i| i.source == path) {
                continue;
            }
            self.attachment_sequence = self.attachment_sequence.wrapping_add(1);
            let id = self.attachment_sequence;
            let kind = match &source {
                Source::File(path, document) => classify(path, *document),
                Source::Image(_) => "image",
            };
            let clipboard_bytes = match &source {
                Source::Image(image) => image.png().len(),
                _ => 0,
            };
            let mut caption = TextEdit::new("");
            caption.focused = panel.items.is_empty();
            panel.items.push(Item {
                id,
                source: path,
                clipboard_bytes,
                kind,
                prepared: None,
                image: None,
                caption,
                error: None,
            });
            if self
                .tx
                .send(Command::PrepareAttachment(panel.generation, id, source))
                .is_err()
            {
                panel.items.last_mut().unwrap().error = Some("The local service stopped".into());
            }
        }
    }

    pub(super) fn receive_attachment(
        &mut self,
        generation: u64,
        id: u64,
        result: Result<Prepared, String>,
    ) {
        let Some(panel) = &mut self.attachments else {
            return;
        };
        if generation != panel.generation {
            return;
        }
        let Some(item) = panel.items.iter_mut().find(|i| i.id == id) else {
            return;
        };
        item.clipboard_bytes = 0;
        match result {
            Ok(mut prepared) => {
                item.image = prepared.pixels.take().and_then(|pixels| {
                    self.image_cache
                        .as_ref()
                        .map(|cache| cache.rgba(pixels.width(), pixels.height(), pixels.into_raw()))
                });
                item.kind = prepared.kind;
                item.prepared = Some(prepared);
            }
            Err(error) => item.error = Some(error),
        }
    }

    pub(super) fn cancel_attachments(&mut self) {
        if self.attachments.as_ref().is_some_and(|p| p.sending) {
            return;
        }
        self.attachments = None;
        self.draft.focused = true;
        if self.picker.for_attachment {
            self.picker.open = false;
        }
    }

    fn remove_attachment(&mut self) {
        let Some(panel) = &mut self.attachments else {
            return;
        };
        if panel.sending {
            return;
        }
        panel.items.remove(panel.selected);
        panel.selected = panel.selected.min(panel.items.len().saturating_sub(1));
        if panel.items.is_empty() {
            self.cancel_attachments();
        }
    }

    pub(super) fn send_attachments(&mut self) {
        let Some(panel) = &mut self.attachments else {
            return;
        };
        if panel.sending
            || self.busy
            || self.snapshot.connection.phase != Phase::Connected
            || panel.items.is_empty()
            || panel.items.iter().any(|i| i.prepared.is_none())
        {
            return;
        }
        let mut uploads = Vec::new();
        for item in &mut panel.items {
            let prepared = item.prepared.as_ref().unwrap();
            if item.caption.text.len() > 65536 {
                item.error = Some("Message is too long".into());
                return;
            }
            item.error = None;
            uploads.push(Upload {
                id: item.id,
                path: prepared.path.to_string_lossy().into_owned(),
                kind: prepared.kind.into(),
                caption: item.caption.text.clone(),
            });
        }
        panel.total = uploads.len();
        panel.sent = 0;
        if self
            .tx
            .send(Command::SendAttachments(
                panel.generation,
                panel.chat.clone(),
                uploads,
            ))
            .is_ok()
        {
            panel.sending = true;
            self.busy = true;
            self.picker.open = false;
        }
    }

    pub(super) fn receive_attachment_sent(
        &mut self,
        generation: u64,
        id: u64,
        result: Result<Message, String>,
        tx: &mut Tx,
    ) {
        let Some(panel) = &mut self.attachments else {
            return;
        };
        if panel.generation != generation {
            return;
        }
        match result {
            Ok(message) => {
                panel.items.retain(|i| i.id != id);
                panel.sent += 1;
                panel.selected = panel.selected.min(panel.items.len().saturating_sub(1));
                self.merge_messages(vec![message]);
                if !self.detached {
                    tx.scroll_to_bottom_instant("messages");
                }
            }
            Err(error) => {
                if let Some(item) = panel.items.iter_mut().find(|i| i.id == id) {
                    item.error = Some(error);
                }
            }
        }
    }

    pub(super) fn finish_attachments(&mut self, generation: u64) {
        if let Some(panel) = &mut self.attachments
            && panel.generation == generation
        {
            self.busy = false;
            panel.sending = false;
            if panel.items.is_empty() {
                self.attachments = None;
                self.draft.focused = true;
            }
        }
    }
}

/// Native file drops share the clipboard's attachment draft and validation.
/// Only the conversation and its attachment preview are drop targets.
pub(super) fn drop_zone(s: &State, body: El) -> El {
    if !s.can_drop_attachments() {
        return body;
    }
    let body = body
        .on_drop_files(|s: &mut State, files, _| s.drop_chat_files(files))
        .on_drop_hover(|s: &mut State, over, _| s.attachment_drag_over = over);
    if !s.attachment_drag_over {
        return body;
    }
    body.child(
        el().key("attachment-drop-highlight")
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .center()
            .radius(22.0)
            .fill(MINT.alpha(0.09))
            .border(2.0, MINT.alpha(0.8))
            .inert()
            .child(
                col()
                    .padding(24.0)
                    .gap(12.0)
                    .center()
                    .radius(20.0)
                    .fill(PANEL)
                    .border(1.0, MINT.alpha(0.4))
                    .child(icon(&glyphs::FOLDER_OPEN, 32.0).color(MINT))
                    .child(title(locale::tr("Drop files to attach"), 18.0)),
            ),
    )
}

fn size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.0} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

fn picture(item: &Item, width: f32, height: f32) -> El {
    let content = el().size(width, height).center();
    if let Some(image) = &item.image {
        return content.pixels_fit(image.pixels(), SurfaceFit::Fit);
    }
    let kind = item.prepared.as_ref().map_or("file", |p| p.kind);
    content.child(
        icon(
            kind_icon(kind).unwrap_or(&FILE),
            width.min(height).min(48.0),
        )
        .color(MINT),
    )
}

pub(super) fn modal(s: &State) -> Option<(El, Vec<El>)> {
    let panel = s.attachments.as_ref()?;
    let mut picker_layer = Vec::new();
    let [x, y, width, height] = s.chat_bounds;
    let w = (width - 48.0).clamp(320.0, 700.0);
    let h = (height - 56.0).clamp(430.0, 660.0);
    let heading = row()
        .w_full()
        .center_y()
        .gap(12.0)
        .child(
            col()
                .grow(1.0)
                .gap(3.0)
                .child(title(locale::tr("Send attachments"), 18.0))
                .child(label(
                    s.snapshot
                        .current_chat
                        .as_ref()
                        .map(|c| c.name.clone())
                        .unwrap_or_default(),
                    12.0,
                    MUTED,
                )),
        )
        .child(icon_button(&glyphs::CLOSE, "Cancel", |s, _| {
            s.cancel_attachments()
        }));
    let mut body = col().size(w, h).padding(20.0).gap(12.0).child(heading);
    if let Some(item) = panel.items.get(panel.selected) {
        let display = el()
            .w_full()
            .grow(1.0)
            .min_h(70.0)
            .center()
            .radius(16.0)
            .clip_rounded(16.0)
            .fill(Color::BLACK.alpha(0.15));
        body = body.child(if item.prepared.is_none() && item.error.is_none() {
            display.child(label(locale::tr("Preparing attachment…"), 13.0, MUTED))
        } else {
            display.child(if let Some(image) = &item.image {
                el().w_full()
                    .h_full()
                    .pixels_fit(image.pixels(), SurfaceFit::Fit)
            } else {
                icon(kind_icon(item.kind).unwrap_or(&FILE), 48.0).color(MINT)
            })
        });
        let name = item
            .prepared
            .as_ref()
            .map(|p| p.name.clone())
            .or_else(|| {
                item.source
                    .as_ref()
                    .and_then(|p| p.file_name())
                    .map(|n| n.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| locale::tr("Clipboard image").into());
        let info = item
            .prepared
            .as_ref()
            .map(|p| {
                format!(
                    "{} · {}",
                    locale::tr(match p.kind {
                        "image" => "Photo",
                        "video" => "Video",
                        "audio" => "Audio",
                        _ => "File",
                    }),
                    size(p.size)
                )
            })
            .unwrap_or_default();
        body = body.child(
            row()
                .w_full()
                .center_y()
                .gap(10.0)
                .child(
                    col()
                        .grow(1.0)
                        .min_w(0.0)
                        .gap(4.0)
                        .child(label(name, 13.0, INK).w_full().truncate())
                        .child(label(info, 11.0, MUTED)),
                )
                .child(icon_button(&glyphs::CLOSE, "Remove attachment", |s, _| {
                    s.remove_attachment()
                })),
        );
        if let Some(error) = &item.error {
            body = body.child(label(locale::tr(error), 12.0, MUTED).wrap());
        }
        if panel.items.len() > 1 {
            let mut strip = row().gap(8.0).h(58.0);
            for (index, item) in panel.items.iter().enumerate() {
                strip = strip.child(
                    el().key(format!("attachment-{}", item.id))
                        .size(58.0, 58.0)
                        .radius(10.0)
                        .border(
                            1.5,
                            if index == panel.selected {
                                MINT
                            } else {
                                BORDER
                            },
                        )
                        .fill(INK.alpha(0.04))
                        .center()
                        .role("button")
                        .help(
                            item.prepared
                                .as_ref()
                                .map(|p| p.name.as_str())
                                .unwrap_or("…"),
                        )
                        .on_press(move |s: &mut State| {
                            if let Some(panel) = &mut s.attachments
                                && !panel.sending
                            {
                                for item in &mut panel.items {
                                    item.caption.focused = false;
                                }
                                panel.selected = index;
                                panel.items[index].caption.focused = true;
                                s.picker.open = false;
                            }
                        })
                        .child(picture(item, 50.0, 50.0)),
                );
            }
            body = body.child(el().w_full().h(62.0).scroll_x().child(strip));
        }
        if item.kind == "audio" {
            body = body.child(
                label(
                    locale::tr("Audio files do not support captions."),
                    12.0,
                    MUTED,
                )
                .wrap(),
            );
        } else {
            let id = item.id;
            let edit = rune::text_input(&item.caption, locale::style(13.0, 450.0))
                .key(format!("attachment-caption-{id}"))
                .w_full()
                .color(INK)
                .placeholder(locale::tr("Add a caption…"), MUTED)
                .caret(INK, kit().selection());
            let edit = if !panel.caption_available() {
                edit
            } else {
                // Bind this editor to the file identity, not the current index:
                // an old focus/edit event must never overwrite the next caption.
                edit.on_edit(move |s: &mut State| {
                    &mut s
                        .attachments
                        .as_mut()
                        .unwrap()
                        .items
                        .iter_mut()
                        .find(|item| item.id == id)
                        .unwrap()
                        .caption
                })
                .on_input_key(super::message_enter)
                .on_submit(|s: &mut State, _| s.send_attachments())
                .on_input_paste(|s: &mut State, content, _| s.paste_attachments(content))
            };
            body = body.child(
                row()
                    .w_full()
                    .center_y()
                    .gap(8.0)
                    .padding(8.0)
                    .radius(14.0)
                    .fill(Color::BLACK.alpha(0.18))
                    .border(
                        1.0,
                        if item.caption.focused {
                            MINT.alpha(0.55)
                        } else {
                            CONTROL_BORDER
                        },
                    )
                    .child(
                        rune::auto_grow(edit, 1, 3)
                            .key(format!("caption-scroll-{id}"))
                            .grow(1.0)
                            .min_w(0.0),
                    )
                    .child(super::picker::attachment_trigger(s, &mut picker_layer)),
            );
        }
    } else {
        body = body.child(el().grow(1.0).center().child(label(
            locale::tr("Sending…"),
            13.0,
            MUTED,
        )));
    }
    let ready = !panel.sending
        && !panel.items.is_empty()
        && panel.items.iter().all(|i| i.prepared.is_some())
        && s.snapshot.connection.phase == Phase::Connected;
    let status = if panel.sending {
        locale::format(
            "Sending {sent} of {total}…",
            &[
                ("sent", panel.sent.to_string()),
                ("total", panel.total.to_string()),
            ],
        )
    } else {
        locale::format(
            "{count} attachment(s)",
            &[("count", panel.items.len().to_string())],
        )
    };
    body = body.child(
        row()
            .w_full()
            .center_y()
            .gap(12.0)
            .child(label(status, 11.0, MUTED).grow(1.0))
            .child(
                row()
                    .h(40.0)
                    .px(20.0)
                    .gap(8.0)
                    .center()
                    .radius(20.0)
                    .fill(if ready { MINT } else { MINT.alpha(0.3) })
                    .role("button")
                    .label(locale::tr("Send attachments"))
                    .help(locale::tr("Send attachments"))
                    .on_press(move |s: &mut State| {
                        if ready {
                            s.send_attachments();
                        }
                    })
                    .child(icon(&SEND, 17.0).color(Color::BLACK))
                    .child(label(locale::tr("Send"), 13.0, Color::BLACK)),
            ),
    );
    let overlay = el()
        .key("attachment-preview-modal")
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .fill(Color::BLACK.alpha(0.6))
        .backdrop_blur(12.0)
        .role("dialog")
        .label(locale::tr("Send attachments"))
        .on_paste_content(|s: &mut State, content, _| s.paste_chat(content))
        .on_press(|s: &mut State| s.cancel_attachments())
        .child(
            glass(body)
                .absolute(x + (width - w) / 2.0, y + (height - h) / 2.0)
                .on_press(|_: &mut State| {}),
        );
    Some((drop_zone(s, overlay), picker_layer))
}
