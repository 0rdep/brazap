use super::{State, media, view::*};
use rune::{Color, El, Icon, ScrollSpec, SurfaceFit, col, el, icon, row};
use rune_kit::glyphs;
use serde_json::json;

const PIN: Icon = Icon {
    name: "pin-chat",
    view: 24.0,
    body: r#"<path d="m8 3 8 0-1 6 4 4v2H5v-2l4-4-1-6Zm4 12v6" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linejoin="round"/>"#,
};
const MUTE: Icon = Icon {
    name: "mute-chat",
    view: 24.0,
    body: r#"<path d="M5 15V9a7 7 0 0 1 14 0v6l2 3H3l2-3Zm5 6h4M3 3l18 18" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linejoin="round"/>"#,
};
const ARCHIVE: Icon = Icon {
    name: "archive-chat",
    view: 24.0,
    body: r#"<rect x="3" y="3" width="18" height="5" rx="1" fill="none" stroke="currentColor" stroke-width="1.6"/><path d="M5 8v13h14V8m-10 4h6" fill="none" stroke="currentColor" stroke-width="1.6"/>"#,
};

fn copy_row(value: &str, description: &str) -> El {
    let copy = value.to_owned();
    row()
        .gap(8.0)
        .center_y()
        .child(label(value, 13.0, MUTED).grow(1.0).min_w(0.0).truncate())
        .child(icon_button(&glyphs::COPY, description, move |_, tx| {
            tx.copy_rich(rune::ClipboardContent::new(copy.clone()))
        }))
}

pub(super) fn modal(s: &State, _layer: &mut Vec<El>) -> Option<El> {
    if !s.details {
        return None;
    }
    let chat = s
        .snapshot
        .current_chat
        .as_ref()
        .filter(|chat| s.selected.as_deref() == Some(&chat.jid))?;
    let profile_key = format!("profile:{}:{}", chat.jid, chat.photo_version);
    let picture = if let Some(image) = s.images.get(&profile_key).filter(|i| i.error().is_none()) {
        el().size(142.0, 142.0)
            .radius(34.0)
            .clip_rounded(34.0)
            .pixels_fit(image.pixels(), SurfaceFit::Fill)
    } else {
        media::avatar(s, &chat.jid, &chat.name, chat.photo_version, 142.0)
    };
    let picture = picture.on_secondary_press({
        let (jid, name) = (chat.jid.clone(), chat.name.clone());
        move |s: &mut State, p, _| s.profile_menu(jid.clone(), name.clone(), p)
    });
    let header = row()
        .px(24.0)
        .h(58.0)
        .center_y()
        .child(title(super::locale::tr("Chat details"), 17.0))
        .child(el().grow(1.0))
        .child(icon_button(&glyphs::CLOSE, "Close chat details", |s, _| {
            s.details = false;
            s.gallery.clear();
        }));
    let copy_name = chat.name.clone();
    let mut body = col().w_full().padding(24.0).gap(20.0).child(
        row().gap(24.0).center_y().child(picture).child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .gap(8.0)
                .child(
                    row()
                        .gap(8.0)
                        .center_y()
                        .child(title(&chat.name, 23.0).grow(1.0).min_w(0.0).wrap())
                        .child(icon_button(
                            &glyphs::COPY,
                            "Copy chat name",
                            move |_, tx| {
                                tx.copy_rich(rune::ClipboardContent::new(copy_name.clone()))
                            },
                        )),
                )
                .child(if chat.phone.is_empty() {
                    label(
                        super::locale::tr(
                            &(if chat.jid.ends_with("@g.us") {
                                "Group conversation"
                            } else {
                                "Phone number unavailable"
                            }),
                        ),
                        12.0,
                        MUTED,
                    )
                } else {
                    copy_row(&chat.phone, "Copy phone number")
                }),
        ),
    );
    let mut actions = row().w_full().gap(9.0);
    for (glyph, action, active, name, undo) in [
        (&MUTE, "mute", chat.muted, "Mute", "Unmute"),
        (&PIN, "pin", chat.pinned, "Pin", "Unpin"),
        (&ARCHIVE, "archive", chat.archived, "Archive", "Unarchive"),
    ] {
        let jid = chat.jid.clone();
        actions = actions.child(
            row()
                .grow(1.0)
                .h(40.0)
                .gap(8.0)
                .center()
                .radius(20.0)
                .role("button")
                .label(if active { undo } else { name })
                .help(if active { undo } else { name })
                .fill(if active {
                    MINT.alpha(0.14)
                } else {
                    Color::WHITE.alpha(0.045)
                })
                .border(1.0, if active { MINT.alpha(0.28) } else { BORDER })
                .hover(Color::WHITE.alpha(0.04))
                .dim(if s.busy { 0.45 } else { 1.0 })
                .on_press(move |s: &mut State| {
                    if !s.busy {
                        s.tool(
                            "chat_action",
                            json!({"chat":jid,"action":action,"enabled":!active}),
                        );
                    }
                })
                .child(icon(glyph, 16.0).color(if active { MINT } else { MUTED }))
                .child(label(
                    super::locale::tr(if active { undo } else { name }),
                    12.0,
                    INK,
                )),
        );
    }
    body = body.child(actions);
    if let Some(error) = &s.error {
        body = body.child(label(super::locale::tr(error), 12.0, INK).wrap());
    }
    let mut tabs = row().gap(8.0);
    for (kind, name) in [("media", "Media"), ("files", "Files"), ("audio", "Audio")] {
        tabs = tabs.child(
            el().grow(1.0)
                .h(34.0)
                .center()
                .radius(10.0)
                .role("tab")
                .label(super::locale::tr(name))
                .fill(if s.gallery_kind == kind {
                    MINT.alpha(0.10)
                } else {
                    Color::TRANSPARENT
                })
                .on_press(move |s: &mut State| s.gallery_tab(kind))
                .child(label(
                    super::locale::tr(name),
                    12.0,
                    if s.gallery_kind == kind { MINT } else { MUTED },
                )),
        );
    }
    body = body
        .child(
            row()
                .center_y()
                .child(title(super::locale::tr("Shared in this chat"), 15.0))
                .child(el().grow(1.0))
                .child(label(
                    format!(
                        "{}{}",
                        s.gallery.len(),
                        if s.gallery_next.is_some() { "+" } else { "" }
                    ),
                    11.0,
                    MUTED,
                )),
        )
        .child(tabs);
    let mut gallery = col().w_full().gap(10.0);
    if s.gallery.is_empty() {
        gallery = gallery.child(
            label(
                super::locale::tr(
                    &(if s.gallery_loading {
                        "Loading shared media…"
                    } else {
                        "No shared items in the saved history"
                    }),
                ),
                12.0,
                MUTED,
            )
            .py(30.0)
            .center(),
        );
    }
    if s.gallery_kind == "media" {
        gallery = gallery.child(
            col()
                .w_full()
                .h(310.0)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .child(
                    col()
                        .key("gallery-rows")
                        .growth(rune::GrowthSpec::default())
                        .virtual_rows(
                            s.gallery.len().div_ceil(3),
                            154.0,
                            |s: &State, i| format!("gallery-row-{}", s.gallery[i * 3].id).into(),
                            |s: &State, i| {
                                let mut line = row().gap(10.0).pb(10.0);
                                for m in s.gallery.iter().skip(i * 3).take(3) {
                                    line = line.child(media::tile(s, m, 182.0, 142.0, "gallery"));
                                }
                                line
                            },
                        )
                        .on_visible_rows(|s: &mut State, range| {
                            let items: Vec<_> = s
                                .gallery
                                .iter()
                                .skip(range.start * 3)
                                .take(range.len() * 3)
                                .cloned()
                                .collect();
                            for m in items {
                                s.request_thumbnail(&m);
                            }
                            // Fetch the next page before the grid runs out.
                            if range.end * 3 + 6 >= s.gallery.len() && s.gallery_next.is_some() {
                                s.load_gallery();
                                return s.gallery_loading;
                            }
                            false
                        }),
                ),
        );
    } else {
        for message in &s.gallery {
            let download = message.clone();
            if message.kind == "audio" {
                gallery = gallery.child(
                    row()
                        .key(format!("gallery-audio-{}-{}", message.sender, message.id))
                        .w_full()
                        .gap(14.0)
                        .center_y()
                        .padding(12.0)
                        .radius(11.0)
                        .fill(Color::WHITE.alpha(0.035))
                        .child(
                            col()
                                .grow(1.0)
                                .min_w(0.0)
                                .gap(5.0)
                                .child(super::audio::view(s, message))
                                .child(label(
                                    super::times::chat_date(message.timestamp),
                                    10.0,
                                    MUTED,
                                )),
                        )
                        .child(icon_button(
                            &media::DOWNLOAD,
                            "Download voice message",
                            move |s, tx| save_file(s, tx, download.clone()),
                        )),
                );
                continue;
            }
            gallery = gallery.child(
                row()
                    .w_full()
                    .gap(10.0)
                    .center_y()
                    .padding(12.0)
                    .radius(11.0)
                    .fill(Color::WHITE.alpha(0.035))
                    .child(icon(kind_icon(&message.kind).unwrap_or(&FILE), 21.0).color(MINT))
                    .child(
                        col()
                            .grow(1.0)
                            .min_w(0.0)
                            .gap(4.0)
                            .child(label(message.preview(), 12.0, INK).truncate())
                            .child(label(
                                super::times::chat_date(message.timestamp),
                                10.0,
                                MUTED,
                            )),
                    )
                    .child(icon_button(
                        &media::DOWNLOAD,
                        "Download attachment",
                        move |s, tx| save_file(s, tx, download.clone()),
                    )),
            );
        }
    }
    if s.gallery_loading {
        gallery = gallery.child(row().w_full().center().child(label(
            super::locale::tr("Loading…"),
            12.0,
            MUTED,
        )));
    }
    body = body.child(gallery);
    let card = glass(
        col()
            .w(656.0)
            .max_w(656.0)
            .h_full()
            .max_h(820.0)
            .min_h(0.0)
            .child(header)
            .child(
                col()
                    .w_full()
                    .grow(1.0)
                    .min_h(0.0)
                    .scroll(ScrollSpec {
                        stick_bottom: false,
                    })
                    .scroll_edge_fade(14.0)
                    // The files and voice lists end at the bottom of this scroll; the media
                    // grid has its own and loads from there.
                    .on_at_bottom(|s: &mut State, at| {
                        if at && s.gallery_next.is_some() {
                            s.load_gallery();
                        }
                    })
                    .child(body),
            )
            .on_press(|_: &mut State| {}),
    );
    Some(
        el().key("chat-details-modal")
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .padding(28.0)
            .center()
            .fill(Color::BLACK.alpha(0.66))
            .backdrop_blur(16.0)
            .role("dialog")
            .label(super::locale::tr("Chat details"))
            .on_press(|s: &mut State| {
                s.details = false;
                s.gallery.clear();
            })
            .child(card),
    )
}

pub(super) fn contact_modal(s: &State) -> Option<El> {
    let contact = s.contact.as_ref()?;
    let key = format!(
        "profile:{}:{}",
        contact.sender, contact.sender_photo_version
    );
    let picture = if let Some(image) = s.images.get(&key).filter(|i| i.error().is_none()) {
        el().size(160.0, 160.0)
            .radius(40.0)
            .clip_rounded(40.0)
            .pixels_fit(image.pixels(), SurfaceFit::Fill)
    } else {
        media::avatar(
            s,
            &contact.sender,
            &contact.sender_name,
            contact.sender_photo_version,
            160.0,
        )
    };
    let picture = picture.on_secondary_press({
        let (jid, name) = (contact.sender.clone(), contact.sender_name.clone());
        move |s: &mut State, p, _| s.profile_menu(jid.clone(), name.clone(), p)
    });
    let open = contact.sender.clone();
    let mut body = col()
        .w(380.0)
        .max_w(440.0)
        .padding(24.0)
        .gap(20.0)
        .child(
            row()
                .center_y()
                .child(title(super::locale::tr("Contact"), 17.0))
                .child(el().grow(1.0))
                .child(icon_button(&glyphs::CLOSE, "Close contact", |s, _| {
                    s.contact = None
                })),
        )
        .child(el().w_full().center().child(picture))
        .child(title(&contact.sender_name, 24.0).wrap());
    if !contact.sender_phone.is_empty() {
        body = body.child(copy_row(&contact.sender_phone, "Copy phone number"));
    } else {
        body = body.child(label(
            super::locale::tr("Phone number unavailable"),
            12.0,
            MUTED,
        ));
    }
    body = body
        .child(button(&super::locale::tr("Open chat"), move |s, _| {
            s.contact = None;
            s.busy = true;
            let _ = s.tx.send(super::Command::Open(open.clone()));
        }))
        .on_press(|_: &mut State| {});
    Some(
        el().key("contact-profile")
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .center()
            .fill(Color::BLACK.alpha(0.66))
            .backdrop_blur(16.0)
            .role("dialog")
            .label(super::locale::tr("Contact details"))
            .on_press(|s: &mut State| s.contact = None)
            .child(glass(body)),
    )
}
