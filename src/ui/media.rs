use super::{State, view::*};
use crate::{assets::AssetRequest, model::Message};
use rune::{El, SurfaceFit, col, el, icon, row};
use rune_kit::glyphs;

pub(super) const DOWNLOAD: rune::Icon = rune::Icon {
    name: "save-original",
    view: 24.0,
    body: r#"<path d="M12 3v12m-5-5 5 5 5-5M4 16v5h16v-5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/>"#,
};

pub(super) fn avatar_key(jid: &str, version: i64) -> String {
    format!("avatar:{jid}:{version}")
}
pub(super) fn thumbnail_key(m: &Message) -> String {
    format!("thumb:{}:{}:{}:{}", m.chat, m.sender, m.id, m.has_media)
}
/// The picker's copy of a sticker, which a sticker being sent shows until its own loads.
fn sticker_key(m: &Message) -> Option<String> {
    (m.kind == "sticker")
        .then_some(m.sticker_hash.as_ref())
        .flatten()
        .map(|hash| format!("sticker:{hash}"))
}

pub(super) fn avatar(s: &State, jid: &str, name: &str, _version: i64, size: f32) -> El {
    let base = el()
        .size(size, size)
        .radius(size * 0.32)
        .fill(MINT.alpha(0.10))
        .border(1.0, MINT.alpha(0.08))
        .center()
        .clip_rounded(size * 0.32);
    if let Some(image) = s.avatars.get(jid).filter(|image| image.error().is_none()) {
        base.pixels_fit(image.pixels(), SurfaceFit::Fill)
    } else {
        let initials: String = if name.starts_with('+') {
            name.chars()
                .filter(|c| c.is_ascii_digit())
                .rev()
                .take(2)
                .collect::<String>()
                .chars()
                .rev()
                .collect()
        } else {
            // Words only: a "-" or "|" between words is not an initial.
            name.split_whitespace()
                .filter_map(|p| p.chars().next())
                .filter(|c| !c.is_ascii_punctuation())
                .take(2)
                .collect()
        };
        base.child(label(initials.to_uppercase(), size * 0.32, MINT))
    }
}

pub(super) fn sender_avatar(s: &State, message: &Message) -> El {
    let contact = message.clone();
    let name = if message.sender_name.starts_with('+') {
        message.sender_name.clone()
    } else {
        message.sender_name.trim().chars().take(1).collect()
    };
    el().size(30.0, 30.0)
        .radius(15.0)
        .clip_rounded(15.0)
        .self_start()
        .mt(2.0)
        .help(&message.sender_name)
        .role("button")
        .label(super::locale::format(
            "View {name}",
            &[("name", message.sender_name.clone())],
        ))
        .on_press(move |s: &mut State| {
            let key = format!(
                "profile:{}:{}",
                contact.sender, contact.sender_photo_version
            );
            s.request_asset(
                key,
                AssetRequest::Profile {
                    jid: contact.sender.clone(),
                },
            );
            s.contact = Some(contact.clone());
        })
        .child(avatar(
            s,
            &message.sender,
            &name,
            message.sender_photo_version,
            30.0,
        ))
}

const MORE: rune::Icon = rune::Icon {
    name: "media-menu",
    view: 24.0,
    body: r#"<circle cx="5" cy="12" r="1.7" fill="currentColor"/><circle cx="12" cy="12" r="1.7" fill="currentColor"/><circle cx="19" cy="12" r="1.7" fill="currentColor"/>"#,
};

const PLAY: rune::Icon = rune::Icon {
    name: "play-video",
    view: 24.0,
    body: r#"<path d="m9 5 11 7-11 7Z" fill="currentColor"/>"#,
};
const PAUSE: rune::Icon = rune::Icon {
    name: "pause-video",
    view: 24.0,
    body: r#"<path d="M7 5h4v14H7zm7 0h4v14h-4z" fill="currentColor"/>"#,
};
const SOUND: rune::Icon = rune::Icon {
    name: "video-sound",
    view: 24.0,
    body: r#"<path d="M3 9h4l5-4v14l-5-4H3Zm12-1c3 2 3 6 0 8m3-11c5 4 5 10 0 14" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linejoin="round"/>"#,
};
const MUTED_SOUND: rune::Icon = rune::Icon {
    name: "video-muted",
    view: 24.0,
    body: r#"<path d="M3 9h4l5-4v14l-5-4H3Zm13 0 6 6m0-6-6 6" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linejoin="round"/>"#,
};

const PREVIOUS: rune::Icon = rune::Icon {
    name: "album-previous",
    view: 24.0,
    body: r#"<path d="m15 5-7 7 7 7" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/>"#,
};
const NEXT: rune::Icon = rune::Icon {
    name: "album-next",
    view: 24.0,
    body: r#"<path d="m9 5 7 7-7 7" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/>"#,
};

impl State {
    /// Opens the album item before or after the one in the viewer.
    pub(super) fn step_viewer(&mut self, forward: bool) -> bool {
        let Some(current) = self.viewer.clone() else {
            return false;
        };
        let items: Vec<Message> = album_items(self, &current).into_iter().cloned().collect();
        let Some(at) = items.iter().position(|item| item.id == current.id) else {
            return false;
        };
        let next = if forward {
            at + 1
        } else {
            match at.checked_sub(1) {
                Some(at) => at,
                None => return false,
            }
        };
        match items.get(next) {
            Some(item) => {
                self.open_image(item.clone());
                true
            }
            None => false,
        }
    }
}

pub(super) fn thumbnail(s: &State, message: &Message) -> El {
    let sticker = message.kind == "sticker";
    let actions = if s.forwarding.is_some() { 37.0 } else { 113.0 };
    let avatar = if !message.from_me && message.chat.ends_with("@g.us") {
        39.0
    } else {
        0.0
    };
    let available = (s.chat_bounds[2] - 48.0 - 26.0 - actions - avatar).max(96.0);
    let max_width = available.min(if sticker { 192.0 } else { 280.0 });
    let max_height = if sticker { 192.0 } else { 320.0 };
    let dimensions = s
        .image_sizes
        .get(&thumbnail_key(message))
        .copied()
        .or_else(|| {
            s.images
                .get(&thumbnail_key(message))
                .and_then(|image| image.size())
        });
    let (w, h) = match dimensions.filter(|[w, h]| *w > 0 && *h > 0) {
        Some([w, h]) => {
            let scale = (max_width / w as f32).min(max_height / h as f32).min(1.0);
            (w as f32 * scale, h as f32 * scale)
        }
        None => (
            max_width,
            if sticker {
                max_width
            } else {
                180.0_f32.min(max_height)
            },
        ),
    };
    with_sound_button(
        s,
        message,
        tile(s, message, w, h, "chat").instant_changes(),
        w,
        h,
    )
}

/// A sticker with sound gets a button to turn its sound on and off.
fn with_sound_button(s: &State, message: &Message, tile: El, w: f32, h: f32) -> El {
    if !message.sticker_sound {
        return tile;
    }
    let m = message.clone();
    let on = s.sticker_sound_on(message);
    let name = if on {
        "Mute sticker"
    } else {
        "Play sticker sound"
    };
    tile.child(
        el().key(format!("sticker-sound-{}-{}", message.sender, message.id))
            .absolute(w - 34.0, h - 34.0)
            .size(30.0, 30.0)
            .radius(15.0)
            .center()
            .fill(rune::Color::BLACK.alpha(if on { 0.7 } else { 0.5 }))
            .border(1.0, MINT.alpha(if on { 0.6 } else { 0.0 }))
            .hover(rune::Color::BLACK.alpha(0.75))
            .cursor(rune::Cursor::Pointer)
            .role("button")
            .label(super::locale::tr(name))
            .help(super::locale::tr(name))
            .on_press(move |s: &mut State| s.toggle_sticker_sound(m.clone()))
            .child(
                icon(if on { &SOUND } else { &MUTED_SOUND }, 16.0).color(if on {
                    MINT
                } else {
                    INK
                }),
            ),
    )
}

/// Stickers sent one after another share a bubble, up to three side by side.
const STICKERS_PER_ROW: usize = 3;

fn joins_stickers(previous: &Message, next: &Message) -> bool {
    previous.kind == "sticker"
        && next.kind == "sticker"
        && previous.chat == next.chat
        && previous.sender == next.sender
        && previous.from_me == next.from_me
        && next.quote.is_none()
        && next.timestamp.abs_diff(previous.timestamp) <= 300
        && super::times::day(previous.timestamp) == super::times::day(next.timestamp)
}

/// The timeline range of the sticker row that holds `index`.
pub(super) fn sticker_row_range(s: &State, index: usize) -> Option<std::ops::Range<usize>> {
    let timeline = &s.timeline;
    if timeline.get(index)?.kind != "sticker" {
        return None;
    }
    let mut first = index;
    while first > 0 && joins_stickers(&timeline[first - 1], &timeline[first]) {
        first -= 1;
    }
    let mut last = index;
    while last + 1 < timeline.len() && joins_stickers(&timeline[last], &timeline[last + 1]) {
        last += 1;
    }
    let start = first + (index - first) / STICKERS_PER_ROW * STICKERS_PER_ROW;
    Some(start..(start + STICKERS_PER_ROW).min(last + 1))
}

/// The stickers of a row, side by side; a single sticker keeps its usual size.
pub(super) fn sticker_row(s: &State, message: &Message) -> El {
    let index = s
        .timeline
        .iter()
        .position(|m| m.id == message.id && m.sender == message.sender && m.chat == message.chat);
    let range = index.and_then(|index| sticker_row_range(s, index));
    match range {
        Some(range) if range.len() > 1 => {
            let side = 128.0;
            let mut stickers = row().gap(6.0);
            for item in &s.timeline[range] {
                stickers = stickers.child(with_sound_button(
                    s,
                    item,
                    tile(s, item, side, side, "chat").instant_changes(),
                    side,
                    side,
                ));
            }
            stickers
        }
        _ => thumbnail(s, message),
    }
}

/// Whether two messages are items of the same album.
pub(super) fn same_album(a: &Message, b: &Message) -> bool {
    a.album.is_some() && a.album == b.album && a.chat == b.chat && a.sender == b.sender
}

/// An album's items in timeline order.
pub(super) fn album_items<'a>(s: &'a State, message: &Message) -> Vec<&'a Message> {
    s.timeline
        .iter()
        .filter(|m| same_album(m, message) && matches!(m.kind.as_str(), "image" | "video"))
        .collect()
}

/// Photos and videos sent together, as phones show them: two side by side,
/// three as a pair over a wide one, four or more as a grid whose last tile
/// counts the rest. Each opens the viewer, which steps through the album.
pub(super) fn album(s: &State, message: &Message) -> El {
    let items = album_items(s, message);
    if items.len() < 2 {
        return thumbnail(s, message);
    }
    let gap = 3.0;
    let cell = 136.0;
    let pair = |a: &Message, b: &Message, h: f32| {
        row()
            .gap(gap)
            .child(tile(s, a, cell, h, "album"))
            .child(tile(s, b, cell, h, "album"))
    };
    let grid = col()
        .key(format!(
            "album-{}",
            message.album.as_deref().unwrap_or_default()
        ))
        .gap(gap);
    match items.as_slice() {
        [a, b] => grid.child(pair(a, b, cell * 1.35)),
        [a, b, c] => {
            grid.child(pair(a, b, cell))
                .child(tile(s, c, cell * 2.0 + gap, cell, "album"))
        }
        [a, b, c, d, rest @ ..] => {
            let mut last = tile(s, d, cell, cell, "album");
            if !rest.is_empty() {
                last = last.child(
                    el().absolute(0.0, 0.0)
                        .size(cell, cell)
                        .radius(10.0)
                        .center()
                        .fill(rune::Color::BLACK.alpha(0.55))
                        .child(title(format!("+{}", rest.len()), 26.0)),
                );
            }
            grid.child(pair(a, b, cell)).child(
                row()
                    .gap(gap)
                    .child(tile(s, c, cell, cell, "album"))
                    .child(last),
            )
        }
        _ => thumbnail(s, message),
    }
}

pub(super) fn tile(s: &State, message: &Message, w: f32, h: f32, scope: &str) -> El {
    let fallback = sticker_key(message);
    let image = s
        .images
        .get(&thumbnail_key(message))
        .or_else(|| fallback.as_ref().and_then(|key| s.images.get(key)))
        .filter(|image| image.error().is_none());
    let key = format!("media-{scope}-{}-{}", message.sender, message.id);
    let open = s.media_menu.as_ref().is_some_and(|(id, _)| id == &key);
    let m = message.clone();
    let mut preview = el()
        .key(key.clone())
        .size(w, h)
        .radius(10.0)
        .center()
        .fill(if message.kind == "sticker" {
            rune::Color::TRANSPARENT
        } else {
            rune::Color::BLACK.alpha(0.14)
        })
        .role("button")
        .label(super::locale::tr(
            &(if message.kind == "video" {
                "Play video"
            } else {
                "Open image preview"
            }),
        ))
        .hover_group()
        .on_press(move |s: &mut State| {
            if s.forwarding.is_some() {
                s.toggle_forward(&m);
            } else {
                s.open_image(m.clone());
            }
        });
    // Album tiles are cropped to their cell, as on phones.
    let fit = if scope == "album" {
        preview = preview.clip_rounded(10.0);
        SurfaceFit::Fill
    } else {
        SurfaceFit::Fit
    };
    if let Some(pixels) = s.animations.pixels(&thumbnail_key(message)).or_else(|| {
        s.images
            .get(&thumbnail_key(message))
            .is_none()
            .then(|| fallback.as_ref().and_then(|key| s.animations.pixels(key)))
            .flatten()
    }) {
        preview = preview.pixels_fit(pixels, fit);
    } else if let Some(image) = image {
        preview = preview.pixels_fit(image.pixels(), fit);
    } else {
        preview = preview.child(
            col()
                .gap(9.0)
                .center()
                .child(icon(&MEDIA, 26.0).color(MINT))
                .child(label(
                    super::locale::tr(
                        &(if s.image_failures.contains_key(&thumbnail_key(message)) {
                            "Preview unavailable"
                        } else if message.has_media {
                            "Loading preview…"
                        } else if message.kind == "video" {
                            "Play video"
                        } else {
                            "Downloading media…"
                        }),
                    ),
                    11.0,
                    MUTED,
                )),
        );
    }
    if message.kind == "video" {
        let play = message.clone();
        preview = preview.child(
            el().absolute(0.0, 0.0)
                .w_full()
                .h_full()
                .center()
                .role("button")
                .label(super::locale::tr("Play video"))
                .on_press(move |s: &mut State| {
                    if s.forwarding.is_some() {
                        s.toggle_forward(&play);
                    } else {
                        s.open_image(play.clone());
                    }
                })
                .child(
                    el().size(48.0, 48.0)
                        .radius(24.0)
                        .center()
                        .fill(rune::Color::BLACK.alpha(0.65))
                        .border(1.0, rune::Color::WHITE.alpha(0.15))
                        .child(icon(&PLAY, 25.0).color(INK)),
                ),
        );
    }
    {
        let button_key = key.clone();
        let m = message.clone();
        preview = preview.child(
            el().key(format!("options-{key}"))
                .group_opacity(if open { 1.0 } else { 0.0 }, 1.0)
                .size(30.0, 30.0)
                .center()
                .radius(9.0)
                .fill(rune::Color::BLACK.alpha(0.65))
                .pin(None, Some(6.0), Some(6.0), None)
                .role("button")
                .label(super::locale::tr("Media options"))
                .help(super::locale::tr("Media options"))
                .on_press(move |s: &mut State| s.media_menu = Some((button_key.clone(), m.clone())))
                .child(icon(&MORE, 18.0).color(INK)),
        );
    }
    preview
}

pub(super) fn menu(s: &State) -> Vec<El> {
    super::context_menu::menu(s)
}

pub(super) fn viewer(s: &State) -> Option<El> {
    let m = s.viewer.as_ref()?;
    if m.kind == "video" {
        return Some(video_viewer(s, m));
    }
    let key = AssetRequest::Image {
        chat: m.chat.clone(),
        sender: m.sender.clone(),
        id: m.id.clone(),
    }
    .key();
    let image = s.images.get(&key).filter(|image| image.error().is_none());
    let save = m.clone();
    let mut header = row()
        .gap(10.0)
        .center_y()
        .child(title(super::locale::tr("Image preview"), 16.0))
        .child(el().grow(1.0));
    // An album's photos and videos are browsed in place: ←/→ or these buttons.
    let items = album_items(s, m);
    if items.len() > 1
        && let Some(at) = items.iter().position(|item| item.id == m.id)
    {
        header = header
            .child(icon_button(&PREVIOUS, "Previous · ←", |s, _| {
                s.step_viewer(false);
            }))
            .child(label(format!("{} / {}", at + 1, items.len()), 12.0, MUTED))
            .child(icon_button(&NEXT, "Next · →", |s, _| {
                s.step_viewer(true);
            }));
    }
    let mut content = col()
        .w_full()
        .h_full()
        .padding(16.0)
        .gap(12.0)
        .on_press(|_: &mut State| {})
        .child(
            header
                .child(icon_button(&DOWNLOAD, "Save original", move |s, tx| {
                    save_file(s, tx, save.clone())
                }))
                .child(icon_button(
                    &glyphs::CLOSE,
                    "Close preview · Escape",
                    |s, _| s.close_viewer(),
                )),
        );
    let target = m.clone();
    let display = el()
        .grow(1.0)
        .min_h(0.0)
        .w_full()
        .center()
        .on_secondary_press(move |s: &mut State, p, _| s.message_menu(target.clone(), p));
    let pixels = s
        .animations
        .pixels(&key)
        .or_else(|| image.map(|image| image.pixels()));
    content = content.child(if let Some(pixels) = pixels {
        zoomable(s, pixels, display)
    } else {
        display.child(label(super::locale::tr("Loading image…"), 14.0, MUTED))
    });
    if let Some((_, Some(error))) = s.image_failures.get(&key) {
        let retry = m.clone();
        content = content
            .child(label(super::locale::tr(error), 12.0, INK).wrap())
            .child(button("Retry image", move |s, _| {
                s.open_image(retry.clone())
            }));
    } else if s.image_pending.contains(&key) {
        content =
            content.child(label(super::locale::tr("Loading full image…"), 11.0, MUTED).center());
    }
    let extra = if s
        .image_failures
        .get(&key)
        .is_some_and(|(_, error)| error.is_some())
    {
        100.0
    } else if s.image_pending.contains(&key) {
        24.0
    } else {
        0.0
    };
    Some(viewer_shell(
        s,
        "image-viewer",
        "Image preview",
        content,
        s.image_sizes
            .get(&key)
            .copied()
            .or_else(|| image.and_then(|i| i.size())),
        80.0 + extra,
    ))
}

/// The image viewer's zoom: the wheel or a pinch zooms about the pointer, a drag pans
/// while zoomed and a double click toggles between fitting and 2×.
#[derive(Clone, Copy, Debug)]
pub(super) struct Zoom {
    scale: f32,
    pan: [f32; 2],
    size: [f32; 2],
    /// The pan when the current drag began.
    grab: [f32; 2],
}

impl Default for Zoom {
    fn default() -> Self {
        Self {
            scale: 1.0,
            pan: [0.0; 2],
            size: [0.0; 2],
            grab: [0.0; 2],
        }
    }
}

const MAX_ZOOM: f32 = 8.0;
/// A notched wheel reports 40 points per step; each step zooms by 20%.
const WHEEL_ZOOM: f32 = 0.004_558;

impl Zoom {
    /// Scale by `factor`, keeping the image point under (`x`, `y`) where it is.
    fn zoom_at(&mut self, factor: f32, x: f32, y: f32) {
        let scale = (self.scale * factor).clamp(1.0, MAX_ZOOM);
        let ratio = scale / self.scale;
        let [cx, cy] = [self.size[0] / 2.0, self.size[1] / 2.0];
        self.pan = [
            x - cx - ratio * (x - cx - self.pan[0]),
            y - cy - ratio * (y - cy - self.pan[1]),
        ];
        self.scale = scale;
        self.clamp();
    }
    fn clamp(&mut self) {
        for axis in 0..2 {
            let limit = (self.scale - 1.0) * self.size[axis] / 2.0;
            self.pan[axis] = self.pan[axis].clamp(-limit, limit);
        }
    }
}

fn zoomable(s: &State, pixels: &rune::Pixels, display: El) -> El {
    let zoom = s.viewer_zoom;
    let zoomed = zoom.scale > 1.001;
    let mut display = display
        .clip_rounded(8.0)
        .on_layout(|s: &mut State, [_, _, w, h]| {
            s.viewer_zoom.size = [w, h];
            s.viewer_zoom.clamp();
        })
        .on_wheel(|s: &mut State, e, _| {
            if e.dy != 0.0 {
                s.viewer_zoom.zoom_at((e.dy * WHEEL_ZOOM).exp(), e.x, e.y);
            }
            true
        })
        .on_magnify(|s: &mut State, e, _| {
            s.viewer_zoom.zoom_at(1.0 + e.delta, e.x, e.y);
            true
        })
        .on_double_press_tx(|s: &mut State, _| {
            let [w, h] = s.viewer_zoom.size;
            if s.viewer_zoom.scale > 1.001 {
                s.viewer_zoom.zoom_at(0.0, w / 2.0, h / 2.0);
            } else {
                s.viewer_zoom.zoom_at(2.0, w / 2.0, h / 2.0);
            }
        })
        .help(super::locale::tr("Scroll to zoom · Double-click to reset"));
    if zoomed {
        display = display
            .cursor(rune::Cursor::Grab)
            .on_drag(|s: &mut State, d, _| {
                let zoom = &mut s.viewer_zoom;
                if d.phase == rune::Phase::Down {
                    zoom.grab = zoom.pan;
                }
                zoom.pan = [zoom.grab[0] + d.dx, zoom.grab[1] + d.dy];
                zoom.clamp();
            });
    }
    display.child(
        el().w_full()
            .h_full()
            .pixels_fit(pixels, SurfaceFit::Fit)
            .scale(zoom.scale)
            .offset(zoom.pan[0], zoom.pan[1])
            .motion(rune::tok::motion::SNAPPY),
    )
}

fn video_viewer(s: &State, message: &Message) -> El {
    let status = s.video.as_ref().map(|v| v.status()).unwrap_or_default();
    let error = s.video_error.as_ref().or(status.error.as_ref());
    let save = message.clone();
    let target = message.clone();
    let mut display = el()
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .center()
        .radius(12.0)
        .clip_rounded(12.0)
        .fill(rune::Color::BLACK)
        .role("button")
        .label(super::locale::tr("Play or pause video"))
        .on_press(|s: &mut State| {
            if let Some(video) = &s.video {
                video.toggle();
            }
        })
        .on_secondary_press(move |s: &mut State, p, _| s.message_menu(target.clone(), p));
    if let Some(video) = &s.video {
        display = display.child(video.view());
    } else if let Some(poster) = s.images.get(&thumbnail_key(message)) {
        display = display.pixels_fit(poster.pixels(), SurfaceFit::Fit);
    }
    if s.video.is_none() || status.loading {
        display = display.child(
            label(
                super::locale::tr(
                    &(if error.is_some() {
                        "Video unavailable"
                    } else if s.video.is_none() {
                        "Preparing video…"
                    } else {
                        "Loading video…"
                    }),
                ),
                14.0,
                INK,
            )
            .padding(14.0)
            .radius(10.0)
            .fill(rune::Color::BLACK.alpha(0.7)),
        );
    }
    let progress = status
        .duration
        .filter(|d| *d > 0.0)
        .map_or(0.0, |d| (status.position / d).clamp(0.0, 1.0) as f32);
    let seek = el()
        .key("video-seek")
        .w_full()
        .h(24.0)
        .center_y()
        .role("slider")
        .label(super::locale::tr("Video position"))
        .help(super::locale::tr("Click or drag to seek"))
        .on_drag(|s: &mut State, d, _| {
            if d.phase == rune::Phase::Up
                && !d.cancelled()
                && let Some(video) = &s.video
                && let Some(duration) = video.status().duration
            {
                video.seek((d.x / d.w.max(1.0)).clamp(0.0, 1.0) as f64 * duration);
            }
        })
        .child(
            el().w_full()
                .h(4.0)
                .radius(2.0)
                .fill(rune::Color::WHITE.alpha(0.12))
                .child(el().w_frac(progress).h(4.0).radius(2.0).fill(MINT)),
        );
    let mut content = col()
        .w_full()
        .h_full()
        .padding(16.0)
        .gap(12.0)
        .on_press(|_: &mut State| {})
        .child(
            row()
                .gap(10.0)
                .center_y()
                .child(title(super::locale::tr("Video"), 16.0))
                .child(el().grow(1.0))
                .child(icon_button(&DOWNLOAD, "Download video", move |s, tx| {
                    save_file(s, tx, save.clone())
                }))
                .child(icon_button(
                    &glyphs::CLOSE,
                    "Close preview · Escape",
                    |s, _| s.close_viewer(),
                )),
        )
        .child(display)
        .child(seek)
        .child(
            row()
                .center_y()
                .gap(12.0)
                .child(icon_button(
                    if status.playing { &PAUSE } else { &PLAY },
                    if status.playing {
                        "Pause · Space"
                    } else if status.ended {
                        "Replay · Space"
                    } else {
                        "Play · Space"
                    },
                    |s, _| {
                        if let Some(video) = &s.video {
                            video.toggle();
                        }
                    },
                ))
                .child(label(
                    format!(
                        "{} / {}",
                        clock(status.position),
                        status.duration.map(clock).unwrap_or_else(|| "–:––".into())
                    ),
                    12.0,
                    INK,
                ))
                .child(el().grow(1.0))
                .child(icon_button(
                    if status.muted { &MUTED_SOUND } else { &SOUND },
                    if status.muted { "Unmute" } else { "Mute" },
                    |s, _| {
                        if let Some(video) = &s.video {
                            video.toggle_mute();
                        }
                    },
                ))
                .child(volume_control(status.volume, status.muted)),
        );
    if let Some(error) = error {
        let retry = message.clone();
        content = content
            .child(label(super::locale::tr(error), 12.0, INK).wrap())
            .child(button("Retry video", move |s, _| {
                s.open_image(retry.clone())
            }));
    }
    let dimensions = status.dimensions.or_else(|| {
        s.image_sizes
            .get(&thumbnail_key(message))
            .copied()
            .or_else(|| {
                s.images
                    .get(&thumbnail_key(message))
                    .and_then(|image| image.size())
            })
    });
    viewer_shell(
        s,
        "video-viewer",
        "Video preview",
        content,
        dimensions,
        164.0 + if error.is_some() { 100.0 } else { 0.0 },
    )
}

fn viewer_shell(
    s: &State,
    key: &str,
    name: &str,
    content: El,
    dimensions: Option<[u32; 2]>,
    chrome: f32,
) -> El {
    let [x, y, width, height] = s.chat_bounds;
    let max_width = (width - 64.0).max(240.0).min(1040.0);
    let max_height = (height - 56.0).max(chrome + 100.0);
    let [natural_w, natural_h] = dimensions
        .filter(|[w, h]| *w > 0 && *h > 0)
        .unwrap_or([640, 400]);
    let scale = (max_width - 32.0).min(natural_w as f32) / natural_w as f32;
    let scale = scale
        .min((max_height - chrome) / natural_h as f32)
        .max(0.01);
    let card_width = (natural_w as f32 * scale + 32.0).max(280.0).min(max_width);
    let card_height = natural_h as f32 * scale + chrome;
    el().key(key)
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .fill(rune::Color::BLACK.alpha(0.78))
        .role("dialog")
        .label(super::locale::tr(name))
        .on_press(|s: &mut State| s.close_viewer())
        .child(
            glass(content)
                .absolute(
                    x + (width - card_width) / 2.0,
                    y + (height - card_height) / 2.0,
                )
                .size(card_width, card_height)
                .on_press(|_: &mut State| {}),
        )
}

fn clock(seconds: f64) -> String {
    let seconds = seconds.max(0.0) as u64;
    if seconds >= 3600 {
        format!(
            "{}:{:02}:{:02}",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60
        )
    } else {
        format!("{}:{:02}", seconds / 60, seconds % 60)
    }
}

#[cfg(target_os = "windows")]
fn volume_control(volume: f64, muted: bool) -> El {
    let amount = if muted { 0.0 } else { volume as f32 };
    el().key("video-volume")
        .w(92.0)
        .h(24.0)
        .center_y()
        .role("slider")
        .label(super::locale::tr("Volume"))
        .help(super::locale::format(
            "{volume}% volume",
            &[("volume", format!("{:.0}", amount * 100.0))],
        ))
        .on_drag(|s: &mut State, d, tx| {
            if !d.cancelled() {
                if let Some(player) = &s.video {
                    player.set_volume((d.x / d.w.max(1.0)).clamp(0.0, 1.0) as f64);
                    tx.instant();
                }
            }
        })
        .on_step(|s: &mut State, step, _| {
            if let Some(player) = &s.video {
                player.set_volume((player.status().volume + step as f64 * 0.05).clamp(0.0, 1.0));
            }
        })
        .child(
            el().w_full()
                .h(4.0)
                .radius(2.0)
                .fill(rune::Color::WHITE.alpha(0.15))
                .child(el().w_frac(amount).h(4.0).radius(2.0).fill(MINT)),
        )
}
#[cfg(not(target_os = "windows"))]
fn volume_control(_: f64, _: bool) -> El {
    el()
}
