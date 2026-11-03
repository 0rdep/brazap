use rune::tok::motion;
use rune::{Color, Cx, El, FileDialogOptions, Icon, ScrollSpec, Tx, col, el, icon, row, text};
use rune_kit::{Kit, glyphs};
use serde_json::json;

use super::{State, actions, details, media, sidebar};
use crate::model::{Message, Phase};

pub(super) const INK: Color = Color {
    r: 0.815,
    g: 0.88,
    b: 0.913,
    a: 1.0,
};
pub(super) const MUTED: Color = Color {
    r: 0.283,
    g: 0.352,
    b: 0.392,
    a: 1.0,
};
pub(super) const MINT: Color = Color {
    r: 0.120,
    g: 0.738,
    b: 0.44,
    a: 1.0,
};
pub(super) const PANEL: Color = Color {
    r: 0.0056,
    g: 0.0103,
    b: 0.0160,
    a: 0.9,
};
pub(super) const BORDER: Color = Color {
    r: 1.0,
    g: 1.0,
    b: 1.0,
    a: 0.065,
};
pub(super) const CHAT: Icon = Icon {
    name: "brazap-chat",
    view: 24.0,
    body: r#"<path d="M5 4h14a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H9l-5 3V6a2 2 0 0 1 1-2Z" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linejoin="round"/><path d="M8 9h9M8 13h6" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"/>"#,
};
pub(super) const SEND: Icon = Icon {
    name: "brazap-send",
    view: 24.0,
    body: r#"<path d="m4 4 17 8-17 8 3-8-3-8Zm3 8h14" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linejoin="round"/>"#,
};
pub(super) const PLUS: Icon = Icon {
    name: "brazap-plus",
    view: 24.0,
    body: r#"<path d="M12 5v14M5 12h14" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/>"#,
};
pub(super) const MEDIA: Icon = Icon {
    name: "brazap-media",
    view: 24.0,
    body: r#"<rect x="3" y="4" width="18" height="16" rx="3" fill="none" stroke="currentColor" stroke-width="1.6"/><path d="m4 17 5-5 4 4 3-3 4 4" stroke="currentColor" fill="none" stroke-width="1.6"/><circle cx="15.5" cy="8.5" r="1.5" fill="currentColor"/>"#,
};
pub(super) const STICKER: Icon = Icon {
    name: "brazap-sticker",
    view: 24.0,
    body: r#"<path d="M20 13V7a3 3 0 0 0-3-3H7a3 3 0 0 0-3 3v10a3 3 0 0 0 3 3h6l7-7Zm0 0h-4a3 3 0 0 0-3 3v4" stroke="currentColor" fill="none" stroke-width="1.6"/><path d="M8 9h.1M14 9h.1M8 13c2 2 4 2 6 0" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/>"#,
};
pub(super) const FILE: Icon = Icon {
    name: "brazap-file",
    view: 24.0,
    body: r#"<path d="M14 3H6v18h12V7l-4-4Zm0 0v5h4M9 12h6M9 16h6" stroke="currentColor" stroke-width="1.6" fill="none" stroke-linejoin="round"/>"#,
};

const VIDEO: Icon = Icon {
    name: "video",
    view: 24.0,
    body: r#"<rect x="3" y="5" width="12" height="14" rx="3" fill="none" stroke="currentColor" stroke-width="1.7"/><path d="m15 10 6-4v12l-6-4" fill="none" stroke="currentColor" stroke-width="1.7"/>"#,
};
pub(super) const PHONE: Icon = Icon {
    name: "calls",
    view: 24.0,
    body: r#"<path d="m7 3 3 5-3 3c1.5 3 3 4.5 6 6l3-3 5 3c-1 4-3 5-6 3C9 18 6 15 3 9 1 6 3 4 7 3Z" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linejoin="round"/>"#,
};
const AUDIO: Icon = Icon {
    name: "audio",
    view: 24.0,
    body: r#"<rect x="9" y="3" width="6" height="12" rx="3" fill="none" stroke="currentColor" stroke-width="1.7"/><path d="M6 10v2a6 6 0 0 0 12 0v-2M12 18v3m-4 0h8" fill="none" stroke="currentColor" stroke-width="1.7"/>"#,
};
const CHECK: Icon = Icon {
    name: "sent",
    view: 24.0,
    body: r#"<path d="m4 12 5 5L20 6" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/>"#,
};
const CHECKS: Icon = Icon {
    name: "delivered",
    view: 24.0,
    body: r#"<path d="m2 12 4 5L17 6m-6 9 2 2L23 6" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/>"#,
};
const CLOCK: Icon = Icon {
    name: "sending",
    view: 24.0,
    body: r#"<circle cx="12" cy="12" r="8" fill="none" stroke="currentColor" stroke-width="1.7"/><path d="M12 7v5l3 2" fill="none" stroke="currentColor" stroke-width="1.7"/>"#,
};

pub(super) fn kind_icon(kind: &str) -> Option<&'static Icon> {
    match kind {
        "file" => Some(&FILE),
        "image" => Some(&MEDIA),
        "video" => Some(&VIDEO),
        "sticker" => Some(&STICKER),
        "audio" => Some(&AUDIO),
        "call" | "voice_call" => Some(&PHONE),
        "video_call" => Some(&VIDEO),
        "template" | "contact" => Some(&CHAT),
        _ => None,
    }
}

pub(super) fn status_icon(status: &str, size: f32) -> El {
    let (glyph, name, color) = match status {
        "read" => (&CHECKS, "Read", MINT),
        "delivered" => (&CHECKS, "Delivered", MUTED),
        "sent" => (&CHECK, "Sent", MUTED),
        "sending" => (&CLOCK, "Sending", MUTED),
        "failed" => (
            &glyphs::CLOSE,
            "Not sent",
            Color {
                r: 0.9,
                g: 0.14,
                b: 0.19,
                a: 1.0,
            },
        ),
        _ => (&CHECK, "Received", MUTED),
    };
    el().size(size, size)
        .help(super::locale::tr(name))
        .label(super::locale::tr(name))
        .child(icon(glyph, size).color(color))
}

pub(super) fn label(value: impl Into<String>, size: f32, color: Color) -> El {
    text(value)
        .font(super::locale::style(size, 450.0))
        .color(color)
}
pub(super) fn title(value: impl Into<String>, size: f32) -> El {
    text(value)
        .font(super::locale::style(size, 620.0))
        .color(INK)
}
pub(super) fn glass(body: El) -> El {
    panel(body.backdrop_blur(18.0))
}

/// Stationary panes already sit on our tinted background; their full-window
/// blur added GPU passes without useful detail. Keep true blur for overlays.
pub(super) fn panel(body: El) -> El {
    body.fill(PANEL)
        .border(1.0, BORDER)
        .radius(22.0)
        .shadow(16.0, 48.0, Color::BLACK.alpha(0.2))
}
pub(super) fn kit() -> &'static Kit {
    static KITS: std::sync::OnceLock<Vec<Kit>> = std::sync::OnceLock::new();
    &KITS.get_or_init(|| {
        let mut kits = Vec::new();
        for family in [
            rune::Family::Sans,
            rune::Family::Named("Inter"),
            rune::Family::Named("Segoe UI"),
            rune::Family::Named("Arial"),
            rune::Family::Mono,
        ] {
            for size in 11..=19 {
                let mut k = Kit::neutral(true);
                k.c.text_accent = MINT;
                k.c.border_focus = MINT;
                k.c.bg_accent = MINT;
                for style in [
                    &mut k.ty.ui,
                    &mut k.ty.ui_medium,
                    &mut k.ty.ui_compact,
                    &mut k.ty.small,
                    &mut k.ty.small_medium,
                    &mut k.ty.body,
                    &mut k.ty.body_medium,
                    &mut k.ty.text_small,
                    &mut k.ty.text_small_medium,
                    &mut k.ty.text_xsmall,
                    &mut k.ty.text_xsmall_medium,
                ] {
                    style.family = family;
                    style.size *= size as f32 / 13.0;
                    style.line_height *= size as f32 / 13.0;
                }
                kits.push(k);
            }
        }
        kits
    })[super::locale::kit_index()]
}

pub(super) fn button(name: &str, action: impl Fn(&mut State, &mut Tx) + 'static) -> El {
    row()
        .h(42.0)
        .px(18.0)
        .center()
        .radius(12.0)
        .fill(MINT)
        .hover(Color::WHITE.alpha(0.08))
        .press(Color::BLACK.alpha(0.08))
        .role("button")
        .label(super::locale::tr(name))
        .on_press_tx(action)
        .motion(motion::SNAPPY)
        .child(
            label(super::locale::tr(name), 13.0, Color::BLACK)
                .font(super::locale::style(13.0, 650.0)),
        )
}

pub(super) fn icon_button(
    glyph: &'static Icon,
    name: &str,
    action: impl Fn(&mut State, &mut Tx) + 'static,
) -> El {
    el().size(36.0, 36.0)
        .center()
        .radius(11.0)
        .role("button")
        .label(super::locale::tr(name))
        .help(super::locale::tr(name))
        .hover(Color::WHITE.alpha(0.05))
        .press(Color::WHITE.alpha(0.09))
        .on_press_tx(action)
        .motion(motion::SNAPPY)
        .child(icon(glyph, 19.0).color(MUTED))
}

pub(super) fn brand(s: &State) -> El {
    row()
        .gap(8.0)
        .center_y()
        .child(logo(s, 40.0))
        .child(title(super::locale::tr("brazap"), 21.0))
}

pub(super) fn logo(s: &State, size: f32) -> El {
    let base = el().size(size, size).center();
    if let Some(image) = &s.logo {
        base.pixels_fit(image.pixels(), rune::SurfaceFit::Fill)
    } else {
        base.child(icon(&CHAT, size * 0.6).color(MINT))
    }
}

pub(super) fn view(s: &State, cx: &mut Cx<State>) -> El {
    super::locale::configure(&s.preferences.value.appearance);
    let connected = s.authenticated;
    let mut layer = Vec::new();
    let content = if connected {
        sidebar::layout(s, cx, &mut layer)
    } else {
        pairing(s, &mut layer)
    };
    let mut root = el()
        .key("brazap-window")
        .w_full()
        .h_full()
        .fill(Color {
            r: 0.0024,
            g: 0.0040,
            b: 0.0070,
            a: 1.0,
        })
        .gradient(
            Color {
                r: 0.0052,
                g: 0.0103,
                b: 0.0144,
                a: 1.0,
            },
            Color {
                r: 0.0021,
                g: 0.0033,
                b: 0.0065,
                a: 1.0,
            },
        );
    if connected {
        root = root.child(
            el().size(450.0, 450.0)
                .absolute(-150.0, -180.0)
                .radius(225.0)
                .fill(MINT.alpha(0.035)),
        );
    }
    root = root.child(content);
    if let Some(error) = &s.error {
        root = root.child(
            row()
                .key("error-banner")
                .pin(Some(24.0), None, Some(24.0), Some(18.0))
                .padding(14.0)
                .gap(12.0)
                .center_y()
                .radius(14.0)
                .fill(Color {
                    r: 0.048,
                    g: 0.013,
                    b: 0.019,
                    a: 1.0,
                })
                .border(1.0, Color::WHITE.alpha(0.1))
                .child(label(super::locale::tr(error), 13.0, INK).grow(1.0).wrap())
                .child(icon_button(&glyphs::CLOSE, "Dismiss error", |s, _| {
                    s.error = None
                })),
        );
    }
    if let Some(details) = details::modal(s, &mut layer) {
        layer.push(details);
    }
    if let Some(contact) = details::contact_modal(s) {
        layer.push(contact);
    }
    layer.extend(media::menu(s));
    layer.extend(actions::reaction_picker(s));
    if let Some(picker) = actions::forward_picker(s) {
        layer.push(picker);
    }
    if let Some(viewer) = media::viewer(s) {
        layer.push(viewer);
    }
    if let Some(diagnostics) = super::diagnostics::modal(s) {
        layer.push(diagnostics);
    }
    if let Some(confirm) = super::preferences::confirmation(s) {
        layer.push(confirm);
    }
    if let Some((modal, above)) = super::attachments::modal(s) {
        layer.push(modal);
        layer.extend(above);
    }
    rune_kit::root(root, layer).on_paste_content(|s: &mut State, content, _| s.paste_chat(content))
}

fn pairing(s: &State, layer: &mut Vec<El>) -> El {
    let connection = &s.snapshot.connection;
    let mut card = col().w(390.0).padding(28.0).gap(22.0);
    if s.phone_mode {
        card = card
            .child(title(super::locale::tr("Link your WhatsApp"), 22.0))
            .child(label(super::locale::tr("Enter your phone number."), 13.0, MUTED).wrap())
            .child(super::phone::input(s, layer))
            .child(button(
                if s.busy {
                    "Requesting…"
                } else if connection.pair_retry_at.is_some_and(|at| at > s.second) {
                    "Please wait before retrying"
                } else {
                    "Get pairing code"
                },
                |s, _| {
                    s.request_phone_pairing();
                },
            ));
        if let Some(code) = &connection.pairing_code {
            card=card.child(title(code,29.0).center()).child(label(super::locale::tr("On your phone: WhatsApp → Linked Devices → Link a device → Link with phone number. Enter this code there."),12.0,MUTED).wrap());
            if let Some(expires) = connection.code_expires_at {
                let remaining = (expires - s.second).max(0);
                card = card.child(label(
                    super::locale::tr(
                        &(if remaining == 0 {
                            super::locale::tr("Code expired. Request a new code.")
                        } else {
                            super::locale::format(
                                "Code valid for {time}",
                                &[("time", format!("{}:{:02}", remaining / 60, remaining % 60))],
                            )
                        }),
                    ),
                    11.0,
                    MUTED,
                ));
            }
        }
    } else {
        let qr = if let Some(image) = &s.qr {
            let [width, height] = image.size().unwrap_or([248, 248]);
            el().size(width as f32, height as f32)
                .pixels(image.pixels())
        } else {
            el().size(248.0, 248.0).center().child(
                label(super::locale::tr("Preparing QR code…"), 13.0, Color::BLACK)
                    .w(190.0)
                    .wrap(),
            )
        };
        card = card
            .child(
                el().w_full().center().child(
                    el().key("pairing-qr")
                        .padding(6.0)
                        .fill(Color::WHITE)
                        .radius(16.0)
                        .role("img")
                        .label(super::locale::tr("QR code"))
                        .child(qr),
                ),
            )
            .child(
                col()
                    .gap(8.0)
                    .child(
                        label(
                            super::locale::tr("1  Open WhatsApp on your phone"),
                            13.0,
                            MUTED,
                        )
                        .wrap(),
                    )
                    .child(
                        label(
                            super::locale::tr("2  Go to Settings → Linked Devices"),
                            13.0,
                            MUTED,
                        )
                        .wrap(),
                    )
                    .child(
                        label(
                            super::locale::tr("3  Tap Link a device and scan this code"),
                            13.0,
                            MUTED,
                        )
                        .wrap(),
                    ),
            );
    }
    if connection.notice.is_some() {
        if let Some(notice) = &connection.notice {
            card = card.child(label(super::locale::tr(notice), 12.0, MUTED).wrap());
        }
        card = card.child(button("Try again", |s, _| s.tool("reconnect", json!({}))));
    }
    let alternative = if s.phone_mode {
        "Use QR code"
    } else {
        "Link with phone number"
    };
    card = card.child(
        row()
            .w_full()
            .h(36.0)
            .center()
            .radius(10.0)
            .role("button")
            .label(super::locale::tr(alternative))
            .hover(MINT.alpha(0.06))
            .on_press(|s: &mut State| {
                s.phone_mode = !s.phone_mode;
                s.phone_country.close();
            })
            .child(label(super::locale::tr(alternative), 13.0, MINT)),
    );
    el().w_full()
        .h_full()
        .padding(32.0)
        .center()
        .child(panel(card))
}

pub(super) fn conversation(s: &State, jid: &str, layer: &mut Vec<El>) -> El {
    let chat = s.snapshot.current_chat.as_ref().filter(|c| c.jid == jid);
    let name = chat.map_or("Conversation", |c| c.name.as_str());
    let header = row()
        .w_full()
        .h(76.0)
        .px(24.0)
        .gap(12.0)
        .center_y()
        .border(1.0, BORDER)
        .role("button")
        .label(super::locale::tr("Open chat details"))
        .help(super::locale::tr("View chat details"))
        .on_press(|s: &mut State| s.open_details())
        .child(media::avatar(
            s,
            jid,
            name,
            chat.map_or(0, |c| c.photo_version),
            39.0,
        ))
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .gap(4.0)
                .child(title(name, 16.0).truncate())
                .child(label(
                    chat.filter(|c| !c.phone.is_empty())
                        .map(|c| c.phone.as_str())
                        .unwrap_or(if jid.ends_with("@g.us") {
                            "Group conversation"
                        } else {
                            "Private conversation"
                        }),
                    11.0,
                    MUTED,
                )),
        );
    let thumb = s.scroll_thumb.clone();
    let height = s.scroll_height.clone();
    let metrics = s.scroll_metrics.clone();
    let timeline = col()
        .key("messages")
        .instant_changes()
        .grow(1.0)
        .min_h(0.0)
        .w_full()
        .scroll(ScrollSpec { stick_bottom: true })
        .scroll_anchor()
        .on_at_bottom(|s: &mut State, at| s.detached = !at)
        .on_layout_tx(|s: &mut State, rect, tx| {
            s.scroll_height.set(rect[3]);
            tx.live_only();
        })
        .on_scroll_offset(move |position| {
            let h = height.get().max(1.0);
            let max = position.max[1];
            let th = (h * h / (h + max)).clamp(28.0f32.min(h), h);
            let y = if max > 0.0 {
                position.offset[1] / max * (h - th)
            } else {
                0.0
            };
            thumb.set([0.0, y, 4.0, th]);
            metrics.set([position.offset[1], max]);
        })
        .child(
            col()
                .key(format!("message-rows-{jid}"))
                .instant_changes()
                .w_full()
                .px(24.0)
                .py(16.0)
                .growth(rune::GrowthSpec::default())
                .virtual_rows(
                    s.timeline.len() + 1 + usize::from(s.has_newer),
                    124.0,
                    |s: &State, i| {
                        if i == 0 {
                            "history-controls".into()
                        } else if i > s.timeline.len() {
                            "newer-history-controls".into()
                        } else {
                            let m = &s.timeline[i - 1];
                            format!("message-{}-{}", m.sender, m.id).into()
                        }
                    },
                    |s: &State, i| timeline_row(s, i),
                )
                .on_visible_rows(|s: &mut State, range| {
                    s.visible_media = s
                        .timeline
                        .iter()
                        .skip(range.start.saturating_sub(1))
                        .take(range.len() + 1)
                        .map(|m| (m.sender.clone(), m.id.clone()))
                        .collect();
                    s.request_chat_media();
                    if range.end >= s.timeline.len().saturating_sub(4) && s.history_notice.is_none()
                    {
                        return s.load_newer();
                    }
                    false
                }),
        );
    let mut viewport = col().grow(1.0).min_h(0.0).w_full().child(timeline);
    if s.detached || s.history_focus {
        viewport = viewport.child(
            row()
                .pin(Some(0.0), None, Some(0.0), Some(14.0))
                .center()
                .child(
                    row()
                        .h(38.0)
                        .px(17.0)
                        .gap(8.0)
                        .center_y()
                        .radius(19.0)
                        .fill(MINT)
                        .border(1.0, MINT.alpha(0.6))
                        .shadow(4.0, 16.0, Color::BLACK.alpha(0.3))
                        .role("button")
                        .label(super::locale::tr("Jump to latest"))
                        .help(super::locale::tr("Return to the newest messages"))
                        .on_press_tx(|s: &mut State, tx| s.jump_latest(tx))
                        .motion(motion::SNAPPY)
                        .child(icon(&glyphs::CHEVRON_DOWN, 16.0).color(Color::BLACK))
                        .child(label(
                            super::locale::tr("Jump to latest"),
                            12.0,
                            Color::BLACK,
                        )),
                ),
        );
        viewport = viewport.child(
            el().pin(None, Some(0.0), Some(5.0), Some(0.0))
                .w(4.0)
                .child(
                    el().absolute(0.0, 0.0)
                        .live_rect(&s.scroll_thumb)
                        .radius(2.0)
                        .fill(MUTED.alpha(0.65))
                        .cursor(rune::Cursor::ResizeRow)
                        .on_drag(|s: &mut State, d, tx| {
                            let [offset, max] = s.scroll_metrics.get();
                            let h = s.scroll_height.get();
                            let th = s.scroll_thumb.get()[3];
                            if d.phase == rune::Phase::Down {
                                d.data = offset;
                            }
                            let target = if d.cancelled() {
                                d.data
                            } else {
                                d.data + d.dy / (h - th).max(1.0) * max
                            };
                            tx.scroll_to_y("messages", target.clamp(0.0, max));
                            tx.instant();
                        }),
                ),
        );
    }
    col()
        .w_full()
        .h_full()
        .on_layout(|s: &mut State, bounds| s.chat_bounds = bounds)
        .child(header)
        .child(viewport)
        .child(composer(s, layer))
}

fn timeline_row(s: &State, index: usize) -> El {
    if index == 0 {
        let mut row = col().w_full().gap(12.0).pb(14.0).child(
            label(
                super::locale::tr("MESSAGES ARE END-TO-END ENCRYPTED"),
                9.0,
                MUTED.alpha(0.65),
            )
            .center(),
        );
        if !s.timeline.is_empty() {
            row = row.child(
                label(
                    super::locale::tr(
                        &(if s.loading_older {
                            "Loading older messages…"
                        } else if s.has_older {
                            "Load older messages"
                        } else {
                            "Load more history from phone"
                        }),
                    ),
                    11.0,
                    MINT,
                )
                .center()
                .role("button")
                .on_press(|s: &mut State| s.load_older()),
            );
        }
        if let Some(notice) = &s.history_notice {
            row = row.child(
                label(super::locale::tr(notice), 11.0, MUTED)
                    .wrap()
                    .center(),
            );
        }
        if s.timeline.is_empty() {
            row = row.child(
                label(
                    super::locale::tr(
                        &(if s.loading_older {
                            "Loading conversation…"
                        } else {
                            "No messages yet. Say hello."
                        }),
                    ),
                    13.0,
                    MUTED,
                )
                .center()
                .py(60.0),
            );
        }
        return row;
    }
    if index > s.timeline.len() {
        let mut row = col().w_full().py(12.0).gap(8.0).child(
            label(
                super::locale::tr(
                    &(if s.loading_newer {
                        "Loading newer messages…"
                    } else {
                        "Load newer messages"
                    }),
                ),
                11.0,
                MINT,
            )
            .center()
            .role("button")
            .on_press(|s: &mut State| {
                s.load_newer();
            }),
        );
        if let Some(notice) = &s.history_notice {
            row = row.child(
                label(super::locale::tr(notice), 11.0, MUTED)
                    .wrap()
                    .center(),
            );
        }
        return row;
    }
    let message = &s.timeline[index - 1];
    let mut row = col().w_full().pb(14.0).gap(12.0);
    if index == 1
        || super::times::day(message.timestamp)
            != super::times::day(s.timeline[index - 2].timestamp)
    {
        row = row.child(row_day(message.timestamp));
    }
    row.child(message_bubble(s, message))
}

fn row_day(timestamp: i64) -> El {
    row().w_full().center().py(8.0).child(
        label(super::times::day_label(timestamp), 10.0, MUTED)
            .px(12.0)
            .py(6.0)
            .radius(13.0)
            .fill(Color::WHITE.alpha(0.055))
            .border(1.0, BORDER),
    )
}

fn message_bubble(s: &State, message: &Message) -> El {
    let mine = message.from_me;
    let mut bubble = col()
        .key(format!("message-{}-{}", message.sender, message.id))
        .max_w(480.0)
        .padding(13.0)
        .gap(8.0)
        .radius(15.0)
        .fill(if mine {
            MINT.alpha(0.135)
        } else {
            Color::WHITE.alpha(0.045)
        })
        .border(
            1.0,
            if s.highlighted.as_deref() == Some(&message.id) {
                MINT.alpha(0.6)
            } else if mine {
                MINT.alpha(0.14)
            } else {
                BORDER
            },
        )
        .instant_changes();
    if message.kind == "sticker" && message.text.is_empty() && message.title.is_empty() {
        bubble = bubble
            .fill(Color::TRANSPARENT)
            .border(0.0, Color::TRANSPARENT)
            .padding(4.0);
    }
    if !mine && message.chat.ends_with("@g.us") {
        let sender = crate::storage::display_identity(
            &message.sender,
            &message.sender_name,
            &message.sender_phone,
        )
        .0;
        bubble = bubble.child(label(sender, 10.0, MINT));
    }
    if message.deleted_at.is_some() {
        bubble = bubble.child(label(super::locale::tr("Deleted on WhatsApp"), 10.0, MUTED));
    }
    if message.forwarded {
        bubble = bubble.child(label(super::locale::tr("↪ Forwarded"), 10.0, MUTED));
    }
    if let Some(quote) = &message.quote {
        let quoted = quote.clone();
        let chat = message.chat.clone();
        let name = if quote.sender == "self" {
            "You".to_owned()
        } else if let Some(original) = s.timeline.iter().find(|m| m.id == quote.id) {
            if original.from_me {
                super::locale::tr("You")
            } else {
                original.sender_name.clone()
            }
        } else {
            crate::storage::display_identity(&quote.sender, "", "").0
        };
        bubble = bubble.child(
            col()
                .gap(5.0)
                .padding(10.0)
                .radius(9.0)
                .fill(MINT.alpha(0.07))
                .border(1.0, MINT.alpha(0.18))
                .role("button")
                .label(super::locale::tr("Go to replied message"))
                .help(super::locale::tr("Go to replied message"))
                .hover(MINT.alpha(0.14))
                .on_press_tx(move |s: &mut State, tx| s.jump_to_quote(&quoted, &chat, tx))
                .child(label(name, 11.0, MINT))
                .child(label(&quote.text, 12.0, MUTED).truncate()),
        );
    }
    if !message.title.is_empty() {
        bubble = bubble.child(title(&message.title, 14.0).wrap());
    }
    if message.is_call() {
        let mut detail = row().gap(6.0).center_y().child(label(
            super::locale::tr(if mine { "Outgoing" } else { "Incoming" }),
            11.0,
            MUTED,
        ));
        if !message.footer.is_empty() {
            detail = detail.child(label("·", 11.0, MUTED)).child(label(
                super::locale::tr(&message.footer),
                11.0,
                MUTED,
            ));
        }
        if let Some(seconds) = message.duration_seconds.filter(|s| *s > 0) {
            detail = detail.child(label("·", 11.0, MUTED)).child(label(
                format!("{}:{:02}", seconds / 60, seconds % 60),
                11.0,
                MUTED,
            ));
        }
        bubble = bubble.child(
            row()
                .gap(12.0)
                .center_y()
                .min_w(210.0)
                .child(
                    el().size(42.0, 42.0)
                        .radius(21.0)
                        .fill(MINT.alpha(0.12))
                        .center()
                        .child(
                            icon(
                                if message.kind == "video_call" {
                                    &VIDEO
                                } else {
                                    &PHONE
                                },
                                22.0,
                            )
                            .color(MINT),
                        ),
                )
                .child(
                    col()
                        .gap(7.0)
                        .child(title(super::locale::tr(&message.text), 14.0))
                        .child(detail),
                ),
        );
    } else if !message.text.is_empty() {
        bubble = bubble.child(label(&message.text, 14.0, INK).wrap().selectable(true));
    }
    if matches!(message.kind.as_str(), "image" | "video" | "sticker") {
        bubble = bubble.child(media::thumbnail(s, message));
    } else if message.kind == "audio" {
        bubble = bubble.child(super::audio::view(s, message));
    } else if message.attachment() {
        let m = message.clone();
        bubble = bubble.child(
            row()
                .gap(9.0)
                .center_y()
                .role("button")
                .label(super::locale::tr("Save attachment"))
                .on_press_tx(move |s: &mut State, tx| save_file(s, tx, m.clone()))
                .child(
                    icon(
                        if message.kind == "sticker" {
                            &STICKER
                        } else if message.kind == "image" || message.kind == "video" {
                            &MEDIA
                        } else {
                            &FILE
                        },
                        22.0,
                    )
                    .color(MINT),
                )
                .child(
                    col()
                        .gap(4.0)
                        .child(label(
                            message
                                .file_name
                                .as_deref()
                                .unwrap_or(match message.kind.as_str() {
                                    "audio" => "Voice message",
                                    "file" => "Document",
                                    _ => "Attachment",
                                }),
                            12.0,
                            INK,
                        ))
                        .child(label(super::locale::tr("Click to save"), 10.0, MUTED)),
                ),
        );
    }
    if !message.footer.is_empty() && !message.is_call() {
        bubble = bubble.child(label(&message.footer, 11.0, MUTED).wrap());
    }
    for action in &message.actions {
        let mut item = col()
            .gap(4.0)
            .padding(10.0)
            .radius(9.0)
            .fill(Color::WHITE.alpha(0.025))
            .border(1.0, BORDER)
            .child(label(&action.label, 12.0, MINT).wrap());
        if let Some(target) = &action.target {
            if target.starts_with("https://") || target.starts_with("http://") {
                let url = target.clone();
                item = item
                    .role("link")
                    .help(target)
                    .on_press_tx(move |s: &mut State, tx| {
                        if let Err(error) = tx.open_url(&url) {
                            s.error = Some(error.to_string());
                        }
                    });
            } else {
                item = item.child(label(target, 11.0, MUTED).wrap().selectable(true));
            }
        }
        bubble = bubble.child(item);
    }
    let mut metadata = row().gap(6.0).center_y().child(el().grow(1.0)).child(
        label(clock(message.timestamp), 9.0, MUTED)
            .help(super::times::day_label(message.timestamp)),
    );
    if mine && !message.is_call() {
        metadata = metadata.child(status_icon(&message.status, 14.0));
    }
    bubble = bubble.child(metadata);
    if let Some(reactions) = actions::reactions(message) {
        bubble = bubble.child(reactions);
    }
    let mut line = row().w_full().gap(9.0).center_y().hover_group();
    if mine {
        line = line.child(el().grow(1.0));
    }
    if mine && s.forwarding.is_none() && message.actionable() {
        line = line.child(actions::toolbar(message));
    }
    if !mine && message.chat.ends_with("@g.us") {
        line = line.child(media::sender_avatar(s, message));
    }
    line = line.child(bubble);
    if !mine && s.forwarding.is_none() && message.actionable() {
        line = line.child(actions::toolbar(message));
    }
    if !mine {
        line = line.child(el().grow(1.0));
    }
    if s.forwarding.is_some() && message.actionable() {
        actions::selection_row(s, message, line)
    } else {
        line
    }
}

fn composer(s: &State, layer: &mut Vec<El>) -> El {
    if let Some(selection) = actions::selection_bar(s) {
        return selection;
    }
    let recording = s.recording.is_some();
    let enabled = s.snapshot.connection.phase == Phase::Connected && !s.busy && !recording;
    let edit = rune::text_input(&s.draft, super::locale::style(13.0, 450.0))
        .key("composer-edit")
        .w_full()
        .color(INK)
        .placeholder(super::locale::tr("Write a message…"), MUTED)
        .caret(INK, kit().selection());
    let edit = if enabled {
        edit.on_edit(|s: &mut State| &mut s.draft)
            .on_input_paste(|s: &mut State, content, _| s.paste_attachments(content))
            .on_submit(|s: &mut State, _| s.send())
    } else {
        edit
    };
    let field = rune::auto_grow(edit, 1, 5)
        .key("composer-input")
        .padding(8.0)
        .grow(1.0)
        .min_w(0.0);
    let send = el()
        .size(42.0, 42.0)
        .radius(13.0)
        .center()
        .fill(if enabled { MINT } else { MINT.alpha(0.3) })
        .role("button")
        .label(super::locale::tr("Send message"))
        .help(super::locale::tr("Send message"))
        .on_press(|s: &mut State| s.send())
        .child(icon(&SEND, 21.0).color(Color::BLACK));
    let trigger = icon_button(&PLUS, "Add an attachment", |s, _| {
        if s.recording.is_none() {
            s.attachments_open = !s.attachments_open;
        }
    });
    let mut menu = col().w(250.0).gap(4.0);
    for (glyph, name, description, kind) in [
        (
            &STICKER,
            "Stickers",
            "Choose from your WhatsApp stickers",
            "sticker",
        ),
        (&MEDIA, "Photos & videos", "Share images or videos", "media"),
        (&AUDIO, "Audio", "Share an audio file", "audio"),
        (&FILE, "File", "Send a document or any file", "file"),
    ] {
        menu = menu.child(
            row()
                .w_full()
                .padding(12.0)
                .gap(12.0)
                .center_y()
                .radius(10.0)
                .hover(Color::WHITE.alpha(0.06))
                .role("button")
                .label(super::locale::tr(name))
                .help(super::locale::tr(description))
                .on_press_tx(move |s: &mut State, tx| {
                    s.attachments_open = false;
                    if kind == "sticker" {
                        s.open_picker(super::picker::Tab::Stickers, None);
                    } else {
                        pick_file(s, tx, kind);
                    }
                })
                .child(icon(glyph, 22.0).color(MINT))
                .child(
                    col()
                        .gap(4.0)
                        .child(title(super::locale::tr(name), 13.0))
                        .child(label(super::locale::tr(description), 11.0, MUTED)),
                ),
        );
    }
    let attach = rune_kit::Popover::new("attachments")
        .trigger(trigger)
        .open(s.attachments_open)
        .placement(rune_kit::Placement::TopStart)
        .on_close(|s: &mut State| s.attachments_open = false)
        .child(menu)
        .build(kit(), layer);
    let mut content = col()
        .key("composer")
        .w_full()
        .padding(18.0)
        .gap(9.0)
        .instant_changes();
    if let Some(reply) = actions::reply_banner(s) {
        content = content.child(reply);
    }
    let controls = row()
        .key("composer-controls")
        .gap(10.0)
        .center_y()
        .child(super::picker::trigger(s, layer))
        .child(attach);
    content.child(
        controls
            .child(field)
            .child(send)
            .child(super::recording::button(s, layer)),
    )
}

pub(super) fn pick_file(s: &mut State, tx: &mut Tx, kind: &'static str) {
    if s.busy || s.snapshot.connection.phase != Phase::Connected {
        return;
    }
    if s.recording.is_some() {
        return;
    }
    let Some(chat) = s.selected.clone() else {
        return;
    };
    let mut options = FileDialogOptions {
        title: super::locale::tr("Choose an attachment").into(),
        ..Default::default()
    };
    if kind == "sticker" {
        options = options.filter("WebP stickers", &["webp"]);
    }
    if kind == "media" {
        options = options.filter(
            "Images and videos",
            &["png", "jpg", "jpeg", "webp", "mp4", "mov"],
        );
    }
    if kind == "audio" {
        options = options.filter(
            super::locale::tr("Audio"),
            &["mp3", "m4a", "ogg", "wav", "aac", "flac", "opus"],
        );
    }
    tx.open_file(options, move |s: &mut State, result, _| match result {
        Ok(Some(path)) if s.selected.as_ref() == Some(&chat) => {
            s.queue_attachments(vec![super::attachments::Source::File(path, kind == "file")]);
        }
        Ok(_) => {}
        Err(error) => s.error = Some(error.to_string()),
    });
}

pub(super) fn save_file(_s: &mut State, tx: &mut Tx, message: Message) {
    let options = FileDialogOptions {
        title: "Save attachment".into(),
        file_name: Some(message.file_name.clone().unwrap_or_else(|| {
            let extension = message
                .mime
                .as_deref()
                .and_then(mime_guess::get_mime_extensions_str)
                .and_then(|ext| ext.first())
                .copied()
                .unwrap_or(match message.kind.as_str() {
                    "image" => "jpg",
                    "video" => "mp4",
                    "sticker" => "webp",
                    _ => "bin",
                });
            format!("{}-{}.{}", message.kind, message.id, extension)
        })),
        ..Default::default()
    };
    tx.save_file(options, move |s: &mut State, result, _| match result {
        Ok(Some(path)) => s.tool(
            "save_attachment",
            json!({"chat":message.chat,"sender":message.sender,"id":message.id,"path":path}),
        ),
        Ok(None) => {}
        Err(error) => s.error = Some(error.to_string()),
    });
}

pub(super) fn empty_chat() -> El {
    col()
        .w_full()
        .h_full()
        .center()
        .gap(18.0)
        .child(
            el().size(82.0, 82.0)
                .center()
                .radius(25.0)
                .fill(MINT.alpha(0.07))
                .border(1.0, MINT.alpha(0.12))
                .child(icon(&CHAT, 37.0).color(MINT)),
        )
        .child(title(super::locale::tr("A little closer."), 29.0))
        .child(label(
            super::locale::tr("Choose a conversation to get started."),
            14.0,
            MUTED,
        ))
}

pub(super) fn clock(timestamp: i64) -> String {
    if timestamp == 0 {
        return String::new();
    }
    chrono::DateTime::from_timestamp(timestamp, 0)
        .map(|t| t.with_timezone(&chrono::Local).format("%H:%M").to_string())
        .unwrap_or_default()
}
