use super::{Section, State, media, view::*};
use crate::model::Chat;
use rune::{Color, Cx, El, GrowthSpec, Icon, ScrollSpec, col, el, icon, row};
use rune_kit::{TextInput, glyphs};

const ARCHIVE: Icon = Icon {
    name: "archive",
    view: 24.0,
    body: r#"<rect x="3" y="4" width="18" height="5" rx="1.5" fill="none" stroke="currentColor" stroke-width="1.6"/><path d="M5 9v9a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V9m-9 4h4" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/>"#,
};
/// The kind chips' scroll viewport, the same whether they switch tabs or filter a search.
const CHIPS: &str = "kind-chips";
const CHEVRON_LEFT: Icon = Icon {
    name: "chips-chevron-left",
    view: 24.0,
    body: r#"<path d="m14.5 6-6 6 6 6" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>"#,
};
const CHEVRON_RIGHT: Icon = Icon {
    name: "chips-chevron-right",
    view: 24.0,
    body: r#"<path d="m9.5 6 6 6-6 6" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>"#,
};
const BACK: Icon = Icon {
    name: "back",
    view: 24.0,
    body: r#"<path d="M19 12H5m6-6-6 6 6 6" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"/>"#,
};

pub(super) const GROUP_ICON: Icon = GROUP;
const GROUP: Icon = Icon {
    name: "group",
    view: 24.0,
    body: r#"<circle cx="9" cy="8" r="3.2" fill="none" stroke="currentColor" stroke-width="1.6"/><path d="M3 19c.6-3.3 3-5 6-5s5.4 1.7 6 5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"/><path d="M15.5 5.2a3 3 0 0 1 0 5.6M17.5 14.3c1.8.6 3 2.2 3.4 4.7" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"/>"#,
};
const CHANNEL: Icon = Icon {
    name: "channel",
    view: 24.0,
    body: r#"<path d="M4 10v4h3l6 4V6L7 10H4Z" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linejoin="round"/><path d="M16.5 9a4 4 0 0 1 0 6M19 6.5a7.5 7.5 0 0 1 0 11" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"/>"#,
};

const COMMUNITY: Icon = Icon {
    name: "community",
    view: 24.0,
    body: r#"<circle cx="12" cy="7" r="2.6" fill="none" stroke="currentColor" stroke-width="1.6"/><circle cx="5.5" cy="16" r="2.4" fill="none" stroke="currentColor" stroke-width="1.6"/><circle cx="18.5" cy="16" r="2.4" fill="none" stroke="currentColor" stroke-width="1.6"/><path d="M10.3 9.2 7 13.8m6.7-4.6 3.3 4.6M8 16h8" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"/>"#,
};
const ALL: Icon = Icon {
    name: "all-chats",
    view: 24.0,
    body: r#"<path d="M4 5.5A1.5 1.5 0 0 1 5.5 4h9A1.5 1.5 0 0 1 16 5.5v6a1.5 1.5 0 0 1-1.5 1.5H9l-4 3v-3.2A1.5 1.5 0 0 1 4 11.5Z" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linejoin="round"/><path d="M19 9h.5A1.5 1.5 0 0 1 21 10.5v6a1.5 1.5 0 0 1-1 1.4V21l-4-3h-4.5a1.5 1.5 0 0 1-1.4-1" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/>"#,
};
const BROADCAST: Icon = Icon {
    name: "broadcast-list",
    view: 24.0,
    body: r#"<circle cx="12" cy="12" r="2" fill="none" stroke="currentColor" stroke-width="1.6"/><path d="M8.5 8.5a5 5 0 0 0 0 7m7-7a5 5 0 0 1 0 7M5.6 5.6a9 9 0 0 0 0 12.8m12.8-12.8a9 9 0 0 1 0 12.8" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"/>"#,
};

pub(super) use crate::model::ChatKind;

/// A sidebar screen: one kind of conversation, calls or status.
#[derive(Clone, Copy, PartialEq)]
enum Screen {
    Chats(ChatKind),
    Calls,
    Status,
}
impl Screen {
    const EVERY: [Self; 8] = [
        Self::Chats(ChatKind::All),
        Self::Chats(ChatKind::Chats),
        Self::Chats(ChatKind::Groups),
        Self::Chats(ChatKind::Communities),
        Self::Chats(ChatKind::Channels),
        Self::Chats(ChatKind::Lists),
        Self::Calls,
        Self::Status,
    ];
    fn current(s: &State) -> Self {
        match s.section {
            Section::Messages => Self::Chats(s.chat_kind),
            Section::Calls => Self::Calls,
            Section::Status => Self::Status,
        }
    }
    fn label(self) -> (&'static Icon, &'static str) {
        match self {
            Self::Chats(ChatKind::All) => (&ALL, "All"),
            Self::Chats(ChatKind::Chats) => (&CHAT, "Chats"),
            Self::Chats(ChatKind::Groups) => (&GROUP, "Groups"),
            Self::Chats(ChatKind::Communities) => (&COMMUNITY, "Communities"),
            Self::Chats(ChatKind::Channels) => (&CHANNEL, "Channels"),
            Self::Chats(ChatKind::Lists) => (&BROADCAST, "Broadcast lists"),
            Self::Calls => (&PHONE, "Calls"),
            Self::Status => (&super::stories::STATUS, "Status"),
        }
    }
    /// The rail's label: one short word, so no language wraps it.
    fn short(self) -> &'static str {
        match self {
            Self::Chats(ChatKind::Lists) => "Lists",
            screen => screen.label().1,
        }
    }
    /// Unread, unmuted, unarchived conversations wait on another chat screen.
    fn waiting(self, s: &State) -> bool {
        let Self::Chats(kind) = self else {
            return false;
        };
        s.snapshot
            .chats
            .iter()
            .any(|c| c.unread > 0 && !c.archived && !c.muted && kind.includes(c))
    }
}

pub(super) fn layout(s: &State, _cx: &mut Cx<State>, layer: &mut Vec<El>) -> El {
    let contents = col()
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .child(list_pane(s).w_full());
    let branding = row()
        .key("sidebar-brand")
        .w_full()
        .h(36.0)
        .pl(10.0)
        .pr(4.0)
        .gap(8.0)
        .center_y()
        .child(brand(s, true).grow(1.0).min_w(0.0))
        .child(header_actions(s));
    let sidebar = col()
        .key("sidebar-shell")
        .w(s.sidebar.width())
        .h_full()
        .gap(8.0)
        .child(branding)
        .child(panel(contents).clip_rounded(22.0).grow(1.0).min_h(0.0));
    let main = if s.preferences.open {
        super::preferences::view(s)
    } else if s.section == Section::Calls {
        calls(s)
    } else if s.section == Section::Status {
        super::stories::feed(s)
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
        .motion(rune::tok::motion::GLIDE)
        .child(sidebar);
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
    body.child(panel(main).key("main-panel").grow(1.0).min_w(0.0).h_full())
}

/// What choosing `screen` does, from the tabs or the header.
fn show(s: &mut State, screen: Screen) {
    if s.search.open {
        s.close_search();
    }
    s.close_story();
    s.preferences.open = false;
    s.attachments_open = false;
    match screen {
        Screen::Chats(kind) => {
            s.section = Section::Messages;
            if s.chat_kind != kind {
                s.chat_kind = kind;
                s.show_archived = false;
            }
        }
        Screen::Calls => s.section = Section::Calls,
        Screen::Status => {
            s.section = Section::Status;
            s.request_status_avatars();
        }
    }
}

/// One chip per kind of chat above the list, scrolling sideways. They choose the screen,
/// and while searching they filter the results instead (from "Everything").
fn kind_chips(s: &State, searching: bool) -> El {
    let current = if searching {
        Some(s.search_kind)
    } else if !s.preferences.open && s.section == Section::Messages {
        Some(s.chat_kind)
    } else {
        None
    };
    let mut chips = row()
        .key("kind-chip-row")
        .gap(6.0)
        .pr(10.0)
        .on_drag(drag_chips());
    for screen in Screen::EVERY {
        let Screen::Chats(kind) = screen else {
            continue;
        };
        let (glyph, _) = screen.label();
        let selected = current == Some(kind);
        let name = if searching && kind == ChatKind::All {
            super::locale::tr("Everything")
        } else {
            super::locale::tr(screen.short())
        };
        let key = format!("chip-{}", screen.short());
        let mut chip = row()
            .key(key.clone())
            .h(30.0)
            .px(12.0)
            .gap(6.0)
            .center_y()
            .radius(15.0)
            .fill(if selected {
                MINT.alpha(0.15)
            } else {
                Color::TRANSPARENT
            })
            .border(
                1.0,
                if selected {
                    MINT.alpha(0.4)
                } else {
                    Color::WHITE.alpha(0.08)
                },
            )
            .hover(if selected {
                MINT.alpha(0.19)
            } else {
                Color::WHITE.alpha(0.05)
            })
            .motion(rune::tok::motion::GLIDE)
            .cursor(rune::Cursor::Pointer)
            .role("tab")
            .label(name.clone())
            .on_press_tx(move |s: &mut State, tx| {
                if searching {
                    s.search_kind = kind;
                } else {
                    show(s, screen);
                }
                // A chip chosen half hidden under an edge slides fully into view.
                tx.scroll_into_view_in(CHIPS, key.clone());
            })
            // Held and dragged, a chip scrolls the row instead of choosing itself.
            .on_drag(drag_chips());
        chip = chip
            .child(icon(glyph, 15.0).color(if selected { MINT } else { MUTED }))
            .child(label(name, 12.0, if selected { INK } else { MUTED }));
        if !searching && screen.waiting(s) {
            chip = chip.child(el().size(6.0, 6.0).radius(3.0).fill(MINT));
        }
        chips = chips.child(chip);
    }
    let offset = s.chip_scroll.clone();
    let [left, right] = s.chip_edges.clone();
    let strip = col()
        .key(CHIPS)
        .w_full()
        .min_w(0.0)
        .scroll_x()
        .scroll_edge_fade(18.0)
        // Clear of the arrow button over each edge.
        .scroll_margin(34.0)
        .on_scroll_offset(move |position| {
            let [x, max] = [position.offset[0], position.max[0]];
            offset.set([x, max]);
            // The arrows show while chips are hidden past their edge.
            left.set((x / 12.0).clamp(0.0, 1.0));
            right.set(((max - x) / 12.0).clamp(0.0, 1.0));
        })
        .child(chips);
    el().key("kind-chips-frame")
        .w_full()
        .min_w(0.0)
        // A mouse wheel turns the chips sideways; there is nothing to scroll vertically.
        .on_wheel(move |s: &mut State, wheel, tx| {
            if wheel.dx != 0.0 || wheel.dy == 0.0 {
                return false;
            }
            let [x, max] = s.chip_scroll.get();
            tx.scroll_to_instant(CHIPS, (x - wheel.dy).clamp(0.0, max), 0.0);
            true
        })
        .child(strip)
        .child(chip_arrow(s, false))
        .child(chip_arrow(s, true))
}

/// Scrolls the chips with the pointer while a press on them is held and dragged.
fn drag_chips() -> impl Fn(&mut State, &mut rune::Drag, &mut rune::Tx) {
    move |s, d, tx| {
        let [x, max] = s.chip_scroll.get();
        if d.phase == rune::Phase::Down {
            d.data = x;
            return;
        }
        let to = if d.cancelled() { d.data } else { d.data - d.dx };
        tx.scroll_to_instant(CHIPS, to.clamp(0.0, max), 0.0);
    }
}

/// The round arrow over an edge of the chips, shown while more chips are hidden past it.
/// Faded out it takes no presses, so the chip under it stays reachable.
fn chip_arrow(s: &State, forward: bool) -> El {
    let name = super::locale::tr(if forward {
        "Scroll right"
    } else {
        "Scroll left"
    });
    let (left, right) = if forward {
        (None, Some(0.0))
    } else {
        (Some(0.0), None)
    };
    el().key(if forward {
        "chips-forward"
    } else {
        "chips-back"
    })
    .pin(left, Some(2.0), right, None)
    .size(26.0, 26.0)
    .center()
    .radius(13.0)
    .fill(Color {
        r: 0.03,
        g: 0.045,
        b: 0.06,
        a: 0.96,
    })
    .border(1.0, CONTROL_BORDER)
    .shadow(4.0, 12.0, Color::BLACK.alpha(0.35))
    .hover(Color {
        r: 0.06,
        g: 0.085,
        b: 0.1,
        a: 1.0,
    })
    .live_opacity(&s.chip_edges[usize::from(forward)])
    .cursor(rune::Cursor::Pointer)
    .role("button")
    .label(name.clone())
    .on_press_tx(move |s: &mut State, tx| {
        let [x, max] = s.chip_scroll.get();
        let step = if forward { 140.0 } else { -140.0 };
        tx.scroll_to(CHIPS, (x + step).clamp(0.0, max), 0.0);
    })
    .child(
        icon(
            if forward {
                &CHEVRON_RIGHT
            } else {
                &CHEVRON_LEFT
            },
            14.0,
        )
        .color(INK),
    )
}

/// The chips with the search button beside them. The search opens its field above them,
/// and the same chips then filter the results; they keep their place and scroll throughout.
fn list_header(s: &State, searching: bool) -> El {
    // The field's slot grows from nothing to its height as the field fades in, pushing the
    // chips and the list down with it, and shrinks back as the field fades out.
    let mut slot = col().key("search-slot").w_full().clip_rounded(0.0);
    if searching {
        slot = slot.child(search_field(s).pb(10.0));
    }
    let header = col().key("list-header").w_full().child(slot);
    let mut bar = row()
        .key("chips-bar")
        .w_full()
        .gap(6.0)
        .center_y()
        .child(kind_chips(s, searching).grow(1.0).min_w(0.0));
    if !searching && (s.section == Section::Messages || s.preferences.open) {
        bar = bar.child(
            el().key("open-search")
                .size(32.0, 32.0)
                .center()
                .radius(16.0)
                .fill(Color::WHITE.alpha(0.035))
                .border(1.0, CONTROL_BORDER)
                .hover(Color::WHITE.alpha(0.07))
                .cursor(rune::Cursor::Pointer)
                .role("button")
                .label(super::locale::tr("Search chats and messages"))
                .help(super::locale::tr("Search chats and messages (Ctrl+K)"))
                .on_press(|s: &mut State| s.open_search())
                .transition(
                    rune::Transition::scale(0.6)
                        .allow_small_scale()
                        .hold_layout()
                        .springs(rune::tok::motion::pair::SNAP),
                )
                .child(icon(&glyphs::SEARCH, 15.0).color(MUTED)),
        );
    }
    header.child(bar)
}

/// The search field with its back button. It fades in growing from the search button's corner
/// as its slot opens, and fades out as the slot closes.
fn search_field(s: &State) -> El {
    let back = el()
        .key("close-search")
        .size(34.0, 34.0)
        .center()
        .radius(10.0)
        .role("button")
        .label(super::locale::tr("Close search"))
        .help(super::locale::tr("Close search"))
        .hover(Color::WHITE.alpha(0.06))
        .cursor(rune::Cursor::Pointer)
        .on_press(|s: &mut State| s.close_search())
        .child(icon(&BACK, 18.0).color(MUTED));
    row()
        .key("search-field")
        .w_full()
        .gap(4.0)
        .center_y()
        .transition(rune::Transition::scale_anchor(
            0.94,
            rune::Vec2::new(1.0, 1.0),
        ))
        .child(back)
        .child(search_input(s).grow(1.0).min_w(0.0))
}

/// A header button beside the logo; a selected screen is tinted.
fn header_button(
    key: &str,
    glyph: &'static Icon,
    name: &str,
    selected: bool,
    action: impl Fn(&mut State) + 'static,
) -> El {
    el().key(format!("header-{key}"))
        .size(34.0, 34.0)
        .center()
        .radius(11.0)
        .fill(if selected {
            MINT.alpha(0.15)
        } else {
            Color::WHITE.alpha(0.035)
        })
        .border(
            1.0,
            if selected {
                MINT.alpha(0.35)
            } else {
                CONTROL_BORDER
            },
        )
        .hover(if selected {
            MINT.alpha(0.19)
        } else {
            Color::WHITE.alpha(0.06)
        })
        .motion(rune::tok::motion::SNAPPY)
        .cursor(rune::Cursor::Pointer)
        .role("button")
        .label(super::locale::tr(name))
        .help(super::locale::tr(name))
        .on_press(action)
        .child(icon(glyph, 18.0).color(if selected { MINT } else { MUTED }))
}

/// Calls, Status, the update action and Settings, beside the logo.
fn header_actions(s: &State) -> El {
    let mut actions = row().gap(6.0).center_y();
    for screen in [Screen::Calls, Screen::Status] {
        let (glyph, name) = screen.label();
        let selected = !s.preferences.open && Screen::current(s) == screen;
        actions = actions.child(header_button(
            screen.short(),
            glyph,
            name,
            selected,
            move |s| show(s, screen),
        ));
    }
    if let Some(update) = super::updates::prompt(s) {
        actions = actions.child(update);
    }
    actions.child(header_button(
        "settings",
        &super::preferences::SETTINGS,
        "Settings",
        s.preferences.open,
        |s| s.open_preferences(),
    ))
}

fn search_input(s: &State) -> El {
    let mut input = TextInput::new(&s.search.edit)
        .leading_icon(&glyphs::SEARCH)
        .placeholder(super::locale::tr(if s.section == Section::Messages {
            "Search"
        } else {
            "Search calls"
        }))
        .width(None)
        .height(super::locale::scale(38.0))
        .on_edit(|s: &mut State| &mut s.search.edit)
        .on_key(|s: &mut State, event, _| {
            if event.key == rune::Key::Escape && s.search.open {
                s.close_search();
                return true;
            }
            // Enter opens a typed number when no existing chat already matches it.
            if event.key == rune::Key::Enter
                && let Some(target) = super::search::phone_target(s)
                && super::search::chat_for_number(s, &target).is_none()
            {
                s.open_number();
                return true;
            }
            false
        });
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
        .help(super::locale::tr(if s.section == Section::Messages {
            "Search chats and messages"
        } else {
            "Search calls"
        }))
        .px(12.0)
        .hover(Color::TRANSPARENT)
        .press(Color::TRANSPARENT)
}

fn list_pane(s: &State) -> El {
    let searching = s.searching_chats();
    // The body slides down as the header grows, while what it holds lands at once.
    let body = col()
        .key("list-body")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .child(list_body(s, searching).instant_changes());
    col()
        .h_full()
        .padding(14.0)
        .pt(14.0)
        .gap(12.0)
        .child(list_header(s, searching))
        .child(body)
}

/// The chats, stories, calls or search results under the header.
fn list_body(s: &State, searching: bool) -> El {
    let mut panel = col().w_full().h_full().gap(12.0);
    if s.section == Section::Status {
        return panel.child(super::stories::list(s));
    }
    if s.section == Section::Calls {
        panel = panel.child(search_input(s));
        return panel.child(
            col().gap(10.0).padding(10.0).child(
                label(
                    super::locale::tr("Your voice and video calls, synced from WhatsApp."),
                    12.0,
                    MUTED,
                )
                .wrap(),
            ),
        );
    }
    if searching {
        if s.search.edit.text.trim().is_empty() {
            return panel.child(
                label(
                    super::locale::tr(
                        "Search chats, groups, communities, channels, lists and messages.",
                    ),
                    12.0,
                    MUTED,
                )
                .padding(10.0)
                .wrap(),
            );
        }
        return panel.child(search_results(s));
    }
    if let Some(entry) = archive_entry(s) {
        panel = panel.child(entry);
    }
    let count = s.visible_chats().len();
    if count == 0 {
        return panel.child(
            label(
                super::locale::tr(if s.snapshot.chats.is_empty() {
                    "Your chats are syncing from WhatsApp."
                } else {
                    match s.chat_kind {
                        ChatKind::All | ChatKind::Chats => "No chats here yet.",
                        ChatKind::Groups => "No groups yet.",
                        ChatKind::Communities => "No communities yet.",
                        ChatKind::Channels => "No channels yet.",
                        ChatKind::Lists => "No broadcast lists yet.",
                    }
                }),
                12.0,
                MUTED,
            )
            .padding(12.0)
            .wrap(),
        );
    }
    if s.chat_kind == ChatKind::Communities {
        return panel.child(communities(s));
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
                    .key(format!("chat-rows-{}", super::locale::scale(76.0)))
                    .growth(GrowthSpec::default())
                    .virtual_rows(
                        count,
                        super::locale::scale(76.0),
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
enum CommunityRow {
    Heading(String),
    Group(Chat),
}

/// A community's groups listed together under its name, most recent first.
fn communities(s: &State) -> El {
    let mut order: Vec<&str> = Vec::new();
    let mut groups: std::collections::HashMap<&str, Vec<&Chat>> = Default::default();
    for chat in s.visible_chats() {
        if !groups.contains_key(chat.community.as_str()) {
            order.push(chat.community.as_str());
        }
        groups
            .entry(chat.community.as_str())
            .or_default()
            .push(chat);
    }
    let mut rows = Vec::new();
    for community in order {
        let mut members = groups[community].clone();
        // Announcements first, then the groups by recent activity.
        members.sort_by_key(|c| !c.announcement);
        let name = members
            .iter()
            .map(|c| c.community_name.trim())
            .find(|n| !n.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| super::locale::tr("Community"));
        rows.push(CommunityRow::Heading(name));
        rows.extend(members.iter().map(|c| CommunityRow::Group((*c).clone())));
    }
    let rows = std::sync::Arc::new(rows);
    let keys = rows.clone();
    let avatars = rows.clone();
    let list = col()
        .w_full()
        .growth(GrowthSpec::default())
        .virtual_rows(
            rows.len(),
            super::locale::scale(76.0),
            move |_: &State, i| match &keys[i] {
                CommunityRow::Heading(name) => format!("community-{i}-{name}").into(),
                CommunityRow::Group(chat) => format!("community-chat-{}", chat.jid).into(),
            },
            move |s: &State, i| match &rows[i] {
                CommunityRow::Heading(name) => row()
                    .w_full()
                    .gap(8.0)
                    .px(10.0)
                    .pt(if i == 0 { 2.0 } else { 14.0 })
                    .pb(6.0)
                    .center_y()
                    .child(icon(&COMMUNITY, 14.0).color(MINT))
                    .child(
                        label(name.clone(), 12.0, MINT)
                            .grow(1.0)
                            .min_w(0.0)
                            .truncate(),
                    ),
                CommunityRow::Group(chat) => chat_row(s, chat),
            },
        )
        .on_visible_rows(move |s: &mut State, range| {
            let visible: Vec<_> = avatars
                [range.start.min(avatars.len())..range.end.min(avatars.len())]
                .iter()
                .filter_map(|row| match row {
                    CommunityRow::Group(chat) => Some((chat.jid.clone(), chat.photo_version)),
                    CommunityRow::Heading(_) => None,
                })
                .collect();
            for (jid, version) in visible {
                s.request_avatar(&jid, version);
            }
            false
        });
    col()
        .key("community-chats")
        .grow(1.0)
        .min_h(0.0)
        .w_full()
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .child(list)
}

/// The way into archived chats, or back out of them.
fn archive_entry(s: &State) -> Option<El> {
    let archived: Vec<_> = s
        .snapshot
        .chats
        .iter()
        .filter(|c| c.archived && s.chat_kind.includes(c))
        .collect();
    let back = s.show_archived;
    if archived.is_empty() && !back {
        return None;
    }
    let name = if back { "Back to chats" } else { "Archived" };
    let unread = archived.iter().filter(|c| c.unread > 0).count();
    let mut entry = row()
        .key("archived-entry")
        .w_full()
        .h(super::locale::scale(44.0))
        .px(8.0)
        .gap(12.0)
        .center_y()
        .radius(12.0)
        .role("button")
        .label(super::locale::tr(name))
        .help(super::locale::tr(if back {
            "Back to chats"
        } else {
            "Show archived chats"
        }))
        .hover(Color::WHITE.alpha(0.045))
        .on_press(|s: &mut State| s.show_archived = !s.show_archived)
        .child(
            el().size(32.0, 32.0)
                .radius(10.0)
                .center()
                .fill(MINT.alpha(0.10))
                .border(1.0, MINT.alpha(0.08))
                .child(icon(if back { &BACK } else { &ARCHIVE }, 17.0).color(MINT)),
        )
        .child(
            label(super::locale::tr(name), 13.0, INK)
                .grow(1.0)
                .min_w(0.0)
                .truncate(),
        );
    if !back {
        entry = entry.child(if unread > 0 {
            el().min_w(17.0)
                .h(17.0)
                .px(3.0)
                .center()
                .radius(8.5)
                .fill(MINT)
                .help(super::locale::tr("Unread"))
                .child(label(unread.min(99).to_string(), 9.0, Color::BLACK))
        } else {
            label(archived.len().to_string(), 12.0, MUTED)
        });
    }
    Some(entry)
}

#[derive(Clone)]
enum SearchRow {
    Heading(&'static str),
    Number(crate::phone_links::PhoneTarget),
    Chat(Chat),
    Message(usize),
}

fn search_results(s: &State) -> El {
    let mut chats = super::search::matching_chats(s);
    let mut rows = Vec::new();
    if let Some(target) = super::search::phone_target(s) {
        match super::search::chat_for_number(s, &target) {
            Some(chat) => {
                if !chats.iter().any(|c| c.jid == chat.jid) {
                    chats.insert(0, chat);
                }
            }
            None => rows.push(SearchRow::Number(target)),
        }
    }
    let empty = rows.is_empty() && chats.is_empty() && s.search.results.messages.is_empty();
    // Chats whose names match come first, under their kind, then the messages.
    for screen in Screen::EVERY {
        let Screen::Chats(kind) = screen else {
            continue;
        };
        if kind == ChatKind::All {
            continue;
        }
        let group: Vec<_> = chats
            .iter()
            .filter(|chat| ChatKind::of(chat) == kind)
            .map(|chat| SearchRow::Chat((*chat).clone()))
            .collect();
        if !group.is_empty() {
            rows.push(SearchRow::Heading(screen.label().1));
            rows.extend(group);
        }
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
                SearchRow::Number(target) => format!("search-number-{}", target.digits).into(),
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
                SearchRow::Number(target) => number_row(s, target),
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

/// Starts a chat with a number that has no chat yet, typed or pasted as a WhatsApp link.
fn number_row(s: &State, target: &crate::phone_links::PhoneTarget) -> El {
    let checking = s.busy;
    let available = s.snapshot.connection.phase == crate::model::Phase::Connected || s.config.demo;
    let detail = if checking {
        "Checking WhatsApp…"
    } else if !available {
        "Connect to WhatsApp to message this number"
    } else {
        "Message this number"
    };
    let mut row = row()
        .key(format!("number-row-{}", target.digits))
        .w_full()
        .h(super::locale::scale(76.0))
        .min_h(super::locale::scale(76.0))
        .max_h(super::locale::scale(76.0))
        .px(10.0)
        .gap(10.0)
        .center_y()
        .radius(13.0)
        .role("button")
        .label(super::locale::format(
            "Message {number}",
            &[("number", target.display.clone())],
        ))
        .child(
            el().size(38.0, 38.0)
                .radius(19.0)
                .center()
                .fill(MINT.alpha(0.14))
                .child(icon(&CHAT, 18.0).color(MINT)),
        )
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .gap(7.0)
                .child(title(&target.display, 13.0).truncate())
                .child(
                    label(
                        super::locale::tr(detail),
                        11.0,
                        if available { MINT } else { MUTED },
                    )
                    .truncate(),
                ),
        );
    if available && !checking {
        row = row
            .hover(Color::WHITE.alpha(0.035))
            .on_press(|s: &mut State| s.open_number());
    }
    row
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

/// A chat's name; the placeholders for nameless chats follow the UI language.
/// A community's announcement group reads "Announcements", as on phones.
pub(super) fn chat_title(chat: &Chat) -> String {
    if chat.announcement {
        return super::locale::tr("Announcements");
    }
    match chat.name.as_str() {
        "Unknown contact" | "Unnamed group" | "Broadcast list" => super::locale::tr(&chat.name),
        name => name.to_owned(),
    }
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
            if is_call || matches!(chat.preview_kind.as_str(), "view_once" | "placeholder") {
                super::locale::tr(&chat.preview)
            } else {
                // Previews read like the message: WhatsApp's *markers* are not shown.
                super::formatting::plain(&chat.preview)
            },
            11.0,
            MUTED,
        )
        .grow(1.0)
        .min_w(0.0)
        .truncate(),
    );
    if chat.pinned {
        preview = preview.child(
            icon(&super::context_menu::PIN, 13.0)
                .color(MUTED)
                .help(super::locale::tr("Pinned chat")),
        );
    }
    if chat.muted {
        preview = preview.child(
            icon(&super::context_menu::MUTE, 13.0)
                .color(MUTED)
                .help(super::locale::tr("Muted notifications")),
        );
    }
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
    let context = chat.clone();
    row()
        .key(format!("chat-row-{}", chat.jid))
        .w_full()
        .h(super::locale::scale(76.0))
        .min_h(super::locale::scale(76.0))
        .max_h(super::locale::scale(76.0))
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
        .label(chat_title(chat))
        .on_secondary_press(move |s: &mut State, p, _| s.chat_menu(context.clone(), p))
        .on_press_tx(move |s: &mut State, tx| {
            if s.is_current_chat(&jid) {
                return;
            }
            s.select(jid.clone());
            tx.instant();
            tx.scroll_to_bottom_instant("messages");
        })
        .child(super::stories::chat_avatar(s, chat))
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
                            title(chat_title(chat), 13.0)
                                .grow(1.0)
                                .min_w(0.0)
                                .truncate()
                                .help_when_truncated(chat_title(chat)),
                        )
                        .child(
                            label(
                                super::times::chat_date(chat.updated_at),
                                9.0,
                                if selected { MINT } else { MUTED },
                            )
                            .max_w(104.0)
                            .truncate(),
                        ),
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
                    .key(format!("call-rows-{}", super::locale::scale(82.0)))
                    .growth(GrowthSpec::default())
                    .virtual_rows(
                        count,
                        super::locale::scale(82.0),
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
                                .h(super::locale::scale(82.0))
                                .min_h(super::locale::scale(82.0))
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
