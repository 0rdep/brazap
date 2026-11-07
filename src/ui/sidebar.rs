use super::{Section, State, media, view::*};
use crate::model::Chat;
use rune::{Color, Cx, El, GrowthSpec, Icon, ScrollSpec, col, el, icon, row};
use rune_kit::{TextInput, glyphs};

const COLLAPSE: Icon = Icon {
    name: "sidebar-collapse",
    view: 24.0,
    body: r#"<rect x="3" y="4" width="18" height="16" rx="3" fill="none" stroke="currentColor" stroke-width="1.6"/><path d="M9 4v16m8-12-4 4 4 4" fill="none" stroke="currentColor" stroke-width="1.6"/>"#,
};
const EXPAND: Icon = Icon {
    name: "sidebar-expand",
    view: 24.0,
    body: r#"<rect x="3" y="4" width="18" height="16" rx="3" fill="none" stroke="currentColor" stroke-width="1.6"/><path d="M9 4v16m4-12 4 4-4 4" fill="none" stroke="currentColor" stroke-width="1.6"/>"#,
};
pub(super) fn layout(s: &State, _cx: &mut Cx<State>, layer: &mut Vec<El>) -> El {
    let sidebar = if s.sidebar.is_open() {
        expanded(s)
    } else {
        compact(s)
    };
    let sidebar = col()
        .h_full()
        .child(sidebar.grow(1.0).min_h(0.0))
        .child(super::preferences::footer(s));
    let main = if s.preferences.open {
        super::preferences::view(s)
    } else if s.section == Section::Calls {
        calls(s)
    } else {
        match &s.selected {
            Some(jid) => conversation(s, jid, layer),
            None => empty_chat(),
        }
    };
    let mut body = row()
        .w_full()
        .h_full()
        .padding(16.0)
        .gap(0.0)
        .child(panel(sidebar).instant_changes());
    if s.sidebar.is_open() {
        body = body.child(
            el().w(12.0)
                .h_full()
                .center()
                .cursor(rune::Cursor::ResizeColumn)
                .help(super::locale::tr(
                    "Drag to resize the sidebar · Double-click to reset",
                ))
                .on_drag(|s: &mut State, d, tx| {
                    if d.phase == rune::Phase::Down {
                        d.data = s.sidebar.width();
                    }
                    if d.cancelled() {
                        s.sidebar.set_width(d.data);
                    } else {
                        s.sidebar.set_width(d.data + d.dx);
                    }
                    tx.instant();
                })
                .on_double_press(|s: &mut State| s.sidebar.reset_width())
                .child(el().w(2.0).h(46.0).radius(1.0).fill(BORDER)),
        );
    } else {
        body = body.child(el().w(12.0));
    }
    body.child(panel(main).grow(1.0).min_w(0.0).h_full())
}

fn navigation(s: &State, section: Section, small: bool) -> El {
    let (glyph, name) = if section == Section::Messages {
        (&CHAT, "Messages")
    } else {
        (&PHONE, "Calls")
    };
    let selected = !s.preferences.open && s.section == section;
    let mut button = row()
        .h(38.0)
        .gap(8.0)
        .center()
        .radius(11.0)
        .role("tab")
        .label(super::locale::tr(name))
        .help(super::locale::tr(name))
        .fill(if selected {
            MINT.alpha(0.12)
        } else {
            Color::TRANSPARENT
        })
        .hover(Color::WHITE.alpha(0.045))
        .on_press(move |s: &mut State| {
            s.preferences.open = false;
            s.section = section;
            s.attachments_open = false;
        })
        .child(icon(glyph, 18.0).color(if selected { MINT } else { MUTED }));
    if small {
        button = button.w(40.0);
    } else {
        button = button.grow(1.0).child(label(
            super::locale::tr(name),
            12.0,
            if selected { INK } else { MUTED },
        ));
    }
    button
}

fn compact(s: &State) -> El {
    col()
        .w(68.0)
        .h_full()
        .padding(12.0)
        .gap(16.0)
        .center_y()
        .child(logo(s, 40.0))
        .child(navigation(s, Section::Messages, true))
        .child(navigation(s, Section::Calls, true))
        .child(el().grow(1.0))
        .child(icon_button(&EXPAND, "Expand sidebar", |s, tx| {
            tx.instant();
            s.sidebar.toggle();
        }))
}

fn search_input(s: &State) -> El {
    let mut input = TextInput::new(&s.search.edit)
        .leading_icon(&glyphs::SEARCH)
        .placeholder(super::locale::tr(if s.section == Section::Messages {
            "Search chats and messages"
        } else {
            "Search calls"
        }))
        .width(None)
        .on_edit(|s: &mut State| &mut s.search.edit);
    if !s.search.edit.text.is_empty() {
        input = input.trailing(
            el().key("clear-search")
                .size(24.0, 24.0)
                .center()
                .role("button")
                .label(super::locale::tr("Clear search"))
                .help(super::locale::tr("Clear search"))
                .on_press(|s: &mut State| s.search.clear())
                .child(icon(&glyphs::CLOSE, 14.0).color(MUTED)),
        );
    }
    input
        .build(kit())
        .h(38.0)
        .px(12.0)
        .hover(Color::TRANSPARENT)
        .press(Color::TRANSPARENT)
}

fn expanded(s: &State) -> El {
    let mut panel = col()
        .w(s.sidebar.width())
        .h_full()
        .padding(16.0)
        .gap(16.0)
        .child(
            row()
                .center_y()
                .child(brand(s))
                .child(el().grow(1.0))
                .child(icon_button(&COLLAPSE, "Collapse sidebar", |s, tx| {
                    tx.instant();
                    s.sidebar.toggle();
                })),
        )
        .child(
            row()
                .gap(6.0)
                .child(navigation(s, Section::Messages, false))
                .child(navigation(s, Section::Calls, false)),
        )
        .child(search_input(s));
    if s.section == Section::Calls {
        return panel.child(
            col()
                .gap(10.0)
                .padding(10.0)
                .child(title(super::locale::tr("Call history"), 15.0))
                .child(
                    label(
                        super::locale::tr("Your voice and video calls, synced from WhatsApp."),
                        12.0,
                        MUTED,
                    )
                    .wrap(),
                ),
        );
    }
    panel = panel.child(row().center_y().child(title(
        super::locale::tr(&(if s.show_archived { "Archived" } else { "Chats" })),
        15.0,
    )));
    let archived = s.snapshot.chats.iter().filter(|c| c.archived).count();
    if archived > 0 || s.show_archived {
        panel = panel.child(
            label(
                super::locale::tr(
                    &(if s.show_archived {
                        "Back to chats".into()
                    } else {
                        super::locale::format(
                            "Archived · {count}",
                            &[("count", archived.to_string())],
                        )
                    }),
                ),
                12.0,
                MUTED,
            )
            .role("button")
            .help(super::locale::tr("Show archived chats"))
            .on_press(|s: &mut State| s.show_archived = !s.show_archived),
        );
    }
    let count = s.visible_chats().len();
    if !s.search.edit.text.trim().is_empty() {
        return panel.child(search_results(s));
    }
    if count == 0 {
        return panel.child(
            label(
                super::locale::tr("Your chats are syncing from WhatsApp."),
                12.0,
                MUTED,
            )
            .padding(12.0)
            .wrap(),
        );
    }
    panel.child(
        col()
            .key("chats")
            .grow(1.0)
            .min_h(0.0)
            .w_full()
            .scroll(ScrollSpec {
                stick_bottom: false,
            })
            .child(
                col()
                    .key("chat-rows")
                    .growth(GrowthSpec::default())
                    .virtual_rows(
                        count,
                        76.0,
                        |s: &State, i| s.visible_chats()[i].jid.clone().into(),
                        |s: &State, i| chat_row(s, s.visible_chats()[i]),
                    )
                    .on_visible_rows(|s: &mut State, range| {
                        let visible: Vec<_> = s
                            .visible_chats()
                            .into_iter()
                            .skip(range.start)
                            .take(range.len())
                            .map(|c| (c.jid.clone(), c.photo_version))
                            .collect();
                        for (jid, version) in visible {
                            s.request_avatar(&jid, version);
                        }
                        false
                    }),
            ),
    )
}

#[derive(Clone)]
enum SearchRow {
    Heading(&'static str),
    Chat(Chat),
    Message(usize),
}

fn search_results(s: &State) -> El {
    let chats = super::search::matching_chats(s);
    let empty = chats.is_empty() && s.search.results.messages.is_empty();
    let mut rows = Vec::new();
    if !chats.is_empty() {
        rows.push(SearchRow::Heading("Chats"));
        rows.extend(chats.into_iter().cloned().map(SearchRow::Chat));
    }
    if !s.search.results.messages.is_empty() {
        rows.push(SearchRow::Heading("Messages"));
        rows.extend((0..s.search.results.messages.len()).map(SearchRow::Message));
    }
    let rows = std::sync::Arc::new(rows);
    let keys = rows.clone();
    let list = col()
        .w_full()
        .growth(GrowthSpec::default())
        .instant_changes()
        .virtual_rows(
            rows.len(),
            78.0,
            move |s: &State, i| match &keys[i] {
                SearchRow::Heading(name) => format!("search-heading-{name}").into(),
                SearchRow::Chat(chat) => format!("search-chat-{}", chat.jid).into(),
                SearchRow::Message(index) => {
                    let m = &s.search.results.messages[*index].message;
                    format!("search-message-{}-{}-{}", m.chat, m.sender, m.id).into()
                }
            },
            move |s: &State, i| match &rows[i] {
                SearchRow::Heading(name) => {
                    label(super::locale::tr(name), 11.0, MINT).padding(10.0)
                }
                SearchRow::Chat(chat) => chat_row(s, chat),
                SearchRow::Message(index) => search_message_row(s, *index),
            },
        );
    let mut panel = col().key("search-results").grow(1.0).min_h(0.0).w_full();
    if s.search.searching() {
        panel = panel.child(label(super::locale::tr("Searching…"), 11.0, MUTED).padding(10.0));
    } else if let Some(error) = &s.search.error {
        panel = panel.child(
            label(super::locale::tr(error), 12.0, MUTED)
                .wrap()
                .padding(10.0),
        );
    } else if empty {
        panel = panel.child(
            label(
                super::locale::tr("No matching chats or messages"),
                12.0,
                MUTED,
            )
            .padding(10.0),
        );
    }
    if s.search.results.next.is_some() {
        panel = panel.child(
            label(
                super::locale::tr("Showing the latest 100 matching messages"),
                11.0,
                MUTED,
            )
            .wrap()
            .padding(10.0),
        );
    }
    panel.child(
        col()
            .key(format!("search-scroll-{}", s.search.edit.text.trim()))
            .grow(1.0)
            .min_h(0.0)
            .w_full()
            .scroll(ScrollSpec {
                stick_bottom: false,
            })
            .child(list),
    )
}

fn search_message_row(s: &State, index: usize) -> El {
    let hit = &s.search.results.messages[index];
    let mut chat = hit.chat.clone();
    chat.preview = hit.excerpt.clone();
    chat.preview_kind = hit.message.kind.clone();
    chat.preview_status = hit.message.status.clone();
    chat.preview_from_me = hit.message.from_me;
    chat.updated_at = hit.message.timestamp;
    chat.unread = 0;
    let hit = hit.clone();
    chat_row(s, &chat)
        .fill(if s.highlighted.as_deref() == Some(&hit.message.id) {
            MINT.alpha(0.10)
        } else {
            Color::TRANSPARENT
        })
        .on_press(move |s: &mut State| s.jump(hit.clone()))
}

fn chat_row(s: &State, chat: &Chat) -> El {
    let selected = s.selected.as_deref() == Some(&chat.jid);
    let jid = chat.jid.clone();
    let mut preview = row().grow(1.0).min_w(0.0).gap(4.0).center_y();
    let is_call = matches!(
        chat.preview_kind.as_str(),
        "call" | "voice_call" | "video_call"
    );
    if chat.preview_from_me && !is_call {
        preview = preview.child(status_icon(&chat.preview_status, 12.0));
    }
    if let Some(glyph) = kind_icon(&chat.preview_kind) {
        preview = preview.child(icon(glyph, 13.0).color(MUTED));
    }
    preview = preview.child(
        label(
            if is_call {
                super::locale::tr(&chat.preview)
            } else {
                chat.preview.clone()
            },
            11.0,
            MUTED,
        )
        .grow(1.0)
        .min_w(0.0)
        .truncate(),
    );
    if chat.unread > 0 {
        preview = preview.child(
            el().min_w(17.0)
                .h(17.0)
                .px(3.0)
                .center()
                .radius(8.5)
                .fill(MINT)
                .child(label(
                    if chat.unread > 99 {
                        "99+".into()
                    } else {
                        chat.unread.to_string()
                    },
                    9.0,
                    Color::BLACK,
                )),
        );
    }
    row()
        .w_full()
        .h(76.0)
        .min_h(76.0)
        .max_h(76.0)
        .px(10.0)
        .gap(10.0)
        .center_y()
        .radius(13.0)
        .fill(if selected {
            MINT.alpha(0.10)
        } else {
            Color::TRANSPARENT
        })
        .hover(Color::WHITE.alpha(0.035))
        .role("button")
        .label(&chat.name)
        .on_press_tx(move |s: &mut State, tx| {
            s.select(jid.clone());
            tx.instant();
            tx.scroll_to_bottom_instant("messages");
        })
        .child(media::avatar(
            s,
            &chat.jid,
            &chat.name,
            chat.photo_version,
            38.0,
        ))
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .gap(7.0)
                .child(
                    row()
                        .gap(6.0)
                        .center_y()
                        .child(
                            title(&chat.name, 13.0)
                                .grow(1.0)
                                .min_w(0.0)
                                .truncate()
                                .help_when_truncated(&chat.name),
                        )
                        .child(label(
                            super::times::chat_date(chat.updated_at),
                            9.0,
                            if selected { MINT } else { MUTED },
                        )),
                )
                .child(preview),
        )
}

fn calls(s: &State) -> El {
    let query = s.search.edit.text.to_lowercase();
    let calls: Vec<_> = s
        .snapshot
        .calls
        .iter()
        .filter(|c| query.is_empty() || c.name.to_lowercase().contains(&query))
        .cloned()
        .collect();
    let count = calls.len();
    let mut body = col().w_full().h_full().child(
        row()
            .h(76.0)
            .px(24.0)
            .gap(12.0)
            .center_y()
            .child(icon(&PHONE, 23.0).color(MINT))
            .child(title(super::locale::tr("Calls"), 18.0))
            .child(el().grow(1.0))
            .child(label(
                super::locale::format("{count} calls", &[("count", count.to_string())]),
                12.0,
                MUTED,
            )),
    );
    if count == 0 {
        return body.child(
            col()
                .grow(1.0)
                .w_full()
                .center()
                .gap(12.0)
                .child(title(super::locale::tr("No calls to show"), 22.0))
                .child(label(
                    super::locale::tr("Call history appears here when WhatsApp syncs it."),
                    13.0,
                    MUTED,
                )),
        );
    }
    let keys = calls.clone();
    let visible = calls.clone();
    body = body.child(
        col()
            .key("calls")
            .grow(1.0)
            .min_h(0.0)
            .w_full()
            .scroll(ScrollSpec {
                stick_bottom: false,
            })
            .child(
                col()
                    .key("call-rows")
                    .growth(GrowthSpec::default())
                    .virtual_rows(
                        count,
                        82.0,
                        move |_: &State, i| format!("{}:{}", keys[i].peer, keys[i].id).into(),
                        move |s: &State, i| {
                            let c = &calls[i];
                            let peer = c.peer.clone();
                            let direction = super::locale::tr(match c.from_me {
                                Some(true) => "Outgoing",
                                Some(false) => "Incoming",
                                None => "Call",
                            });
                            let duration = c
                                .duration
                                .filter(|d| *d > 0)
                                .map(|d| format!(" · {}:{:02}", d / 60, d % 60))
                                .unwrap_or_default();
                            let outcome = super::locale::tr(
                                &c.outcome.trim_start_matches("result_").replace('_', " "),
                            );
                            let date = c
                                .timestamp
                                .and_then(|t| chrono::DateTime::from_timestamp(t, 0))
                                .map(|t| {
                                    t.with_timezone(&chrono::Local)
                                        .format("%b %d · %H:%M")
                                        .to_string()
                                })
                                .unwrap_or_default();
                            row()
                                .h(82.0)
                                .min_h(82.0)
                                .w_full()
                                .px(24.0)
                                .gap(14.0)
                                .center_y()
                                .hover(Color::WHITE.alpha(0.025))
                                .role("button")
                                .label(super::locale::format(
                                    "Open conversation with {name}",
                                    &[("name", c.name.clone())],
                                ))
                                .help(super::locale::tr("Open conversation"))
                                .on_press(move |s: &mut State| {
                                    if !peer.is_empty() {
                                        s.select(peer.clone())
                                    }
                                })
                                .child(media::avatar(s, &c.peer, &c.name, c.photo_version, 42.0))
                                .child(
                                    col()
                                        .grow(1.0)
                                        .min_w(0.0)
                                        .gap(5.0)
                                        .child(title(&c.name, 14.0).truncate())
                                        .child(label(
                                            format!(
                                                "{direction} {} · {outcome}{duration}",
                                                if c.video { "video" } else { "voice" }
                                            ),
                                            11.0,
                                            MUTED,
                                        )),
                                )
                                .child(label(date, 11.0, MUTED))
                        },
                    )
                    .on_visible_rows(move |s: &mut State, range| {
                        for c in visible.iter().skip(range.start).take(range.len()) {
                            s.request_avatar(&c.peer, c.photo_version);
                        }
                        false
                    }),
            ),
    );
    body
}
