use super::{Command, State, view::*};
use crate::{assets::AssetRequest, model::Message, stickers::Sticker};
use rune::{Color, El, Icon, Image, ScrollSpec, TextEdit, col, el, icon, row};
use rune_kit::{Placement, TextInput, glyphs};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    ops::Range,
    sync::OnceLock,
};

const SMILE: Icon = Icon {
    name: "emoji-picker",
    view: 24.0,
    body: r#"<circle cx="12" cy="12" r="9" fill="none" stroke="currentColor" stroke-width="1.6"/><path d="M8 14c2 3 6 3 8 0M8 9h.1M16 9h.1" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/>"#,
};
const SYNC: Icon = Icon {
    name: "sync-stickers",
    view: 24.0,
    body: r#"<path d="M20 7v5h-5M4 17v-5h5M6 7a7 7 0 0 1 12-2l2 3M4 16l2 3a7 7 0 0 0 12-2" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"/>"#,
};
const GROUPS: [&str; 9] = [
    "Smileys & Emotion",
    "People & Body",
    "Animals & Nature",
    "Food & Drink",
    "Travel & Places",
    "Activities",
    "Objects",
    "Symbols",
    "Flags",
];
const GROUP_ICONS: [&str; 10] = ["😀", "👋", "🐻", "🍔", "🚗", "⚽", "💡", "🔣", "🏁", "◷"];
const ART: &[u8] = include_bytes!("../../assets/emoji/art.bin");
#[derive(Deserialize)]
struct Emoji {
    text: String,
    name: String,
    search: String,
    group: String,
    offset: usize,
    length: usize,
}
fn emojis() -> &'static [Emoji] {
    static CATALOG: OnceLock<Vec<Emoji>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        serde_json::from_str(include_str!("../../assets/emoji/index.json"))
            .expect("Bundled emoji catalog")
    })
}

pub(super) fn emoji_artwork(value: &str) -> Option<&'static [u8]> {
    let emoji = emojis().iter().find(|emoji| emoji.text == value)?;
    ART.get(emoji.offset..emoji.offset + emoji.length)
}

#[derive(Clone, Copy, Default, PartialEq)]
pub(super) enum Tab {
    #[default]
    Emoji,
    Stickers,
}
#[derive(Default)]
pub(super) struct Panel {
    pub open: bool,
    pub for_attachment: bool,
    tab: Tab,
    query: TextEdit,
    rendered_query: String,
    category: usize,
    filter: usize,
    stickers: Vec<Sticker>,
    recent: Vec<String>,
    pub loading: bool,
    synced: bool,
    pub error: Option<String>,
    target: Option<Message>,
    pub images: HashMap<usize, Image>,
    pub animation_keys: Vec<String>,
}
impl Panel {
    fn emoji_indices(&self) -> Vec<usize> {
        let query = self.query.text.trim().to_lowercase();
        if self.category == 9 && query.is_empty() {
            return self
                .recent
                .iter()
                .filter_map(|text| emojis().iter().position(|e| &e.text == text))
                .collect();
        }
        emojis()
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                if !query.is_empty() {
                    e.text == query || query.split_whitespace().all(|term| e.search.contains(term))
                } else {
                    self.category >= GROUPS.len() || e.group == GROUPS[self.category]
                }
            })
            .map(|(i, _)| i)
            .collect()
    }
    fn sticker_indices(&self) -> Vec<usize> {
        let query = self.query.text.trim().to_lowercase();
        self.stickers
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                (self.filter == 2
                    || (self.filter == 0 && s.recent_at > 0)
                    || (self.filter == 1 && s.favorite != 0))
                    && (query.is_empty()
                        || s.label.to_lowercase().contains(&query)
                        || s.emojis.contains(&query))
            })
            .map(|(i, _)| i)
            .collect()
    }
}
impl State {
    pub(super) fn open_picker(&mut self, tab: Tab, target: Option<Message>) {
        self.picker.open = true;
        self.picker.for_attachment = false;
        self.picker.tab = tab;
        self.picker.target = target;
        self.picker.query.clear();
        self.attachments_open = false;
        self.reaction_menu = None;
        if self.picker.recent.is_empty() && self.picker.category == 9 {
            self.picker.category = 0;
        }
        self.load_picker_catalog(tab == Tab::Stickers && !self.picker.synced);
        self.warm_picker(0..12);
    }
    pub(super) fn load_picker_catalog(&mut self, sync: bool) {
        if self.picker.loading {
            return;
        }
        self.picker.loading = true;
        self.picker.error = None;
        let _ = self.tx.send(Command::Stickers(sync));
    }
    pub(super) fn receive_stickers(&mut self, sync: bool, result: Result<Value, String>) {
        self.picker.loading = false;
        match result {
            Ok(value) => {
                self.picker.stickers =
                    serde_json::from_value(value["stickers"].clone()).unwrap_or_default();
                self.picker.recent =
                    serde_json::from_value(value["recent_emojis"].clone()).unwrap_or_default();
                self.picker.synced |= sync;
                self.warm_picker(0..12);
            }
            Err(error) => self.picker.error = Some(error),
        }
    }
    pub(super) fn warm_picker(&mut self, range: Range<usize>) -> bool {
        if !self.picker.open {
            return false;
        }
        if self.picker.tab == Tab::Stickers {
            let items: Vec<_> = self
                .picker
                .sticker_indices()
                .into_iter()
                .skip(range.start * 4)
                .take((range.len() + 2) * 4)
                .map(|i| self.picker.stickers[i].hash.clone())
                .collect();
            self.picker.animation_keys =
                items.iter().map(|hash| format!("sticker:{hash}")).collect();
            for hash in items {
                self.request_asset(format!("sticker:{hash}"), AssetRequest::Sticker { hash });
            }
            self.update_animations();
            return false;
        }
        self.picker.animation_keys.clear();
        let Some(cache) = &self.image_cache else {
            return false;
        };
        let indices = self.picker.emoji_indices();
        let visible: HashSet<_> = indices
            .into_iter()
            .skip(range.start * 8)
            .take((range.len() + 2) * 8)
            .collect();
        let mut changed = false;
        self.picker.images.retain(|i, _| visible.contains(i));
        for i in visible {
            if let std::collections::hash_map::Entry::Vacant(slot) = self.picker.images.entry(i) {
                let e = &emojis()[i];
                slot.insert(cache.load(ART[e.offset..e.offset + e.length].to_vec()));
                changed = true;
            }
        }
        changed
    }
    pub(super) fn update_picker_query(&mut self, tx: &mut rune::Tx) -> bool {
        if !self.picker.open || self.picker.rendered_query == self.picker.query.text {
            return false;
        }
        self.picker.rendered_query = self.picker.query.text.clone();
        self.warm_picker(0..12);
        tx.scroll_to_y("emoji-grid", 0.0);
        tx.scroll_to_y("sticker-grid", 0.0);
        tx.instant();
        true
    }
    fn pick_emoji(&mut self, index: usize) {
        let text = emojis()[index].text.clone();
        self.picker.recent.retain(|s| s != &text);
        self.picker.recent.insert(0, text.clone());
        self.picker.recent.truncate(32);
        let _ = self.tx.send(Command::RememberEmoji(text.clone()));
        if let Some(target) = self.picker.target.take() {
            self.picker.open = false;
            self.send_reaction(target, text);
        } else if self.picker.for_attachment {
            if let Some(panel) = &mut self.attachments
                && !panel.sending
                && let Some(item) = panel.items.get_mut(panel.selected)
            {
                item.caption.insert(&text);
            }
        } else {
            self.draft.insert(&text);
        }
    }
}

pub(super) fn trigger(s: &State, layer: &mut Vec<El>) -> El {
    trigger_for(s, layer, false)
}

pub(super) fn attachment_trigger(s: &State, layer: &mut Vec<El>) -> El {
    trigger_for(s, layer, true)
}

fn trigger_for(s: &State, layer: &mut Vec<El>, attachment: bool) -> El {
    let key = if attachment {
        "attachment-emoji-trigger"
    } else {
        "expression-picker-trigger"
    };
    let label_ = if attachment {
        "Emoji"
    } else {
        "Emojis and stickers"
    };
    let button = icon_button(&SMILE, label_, move |s, _| {
        if s.recording.is_some()
            || (attachment
                && !s
                    .attachments
                    .as_ref()
                    .is_some_and(|p| p.caption_available()))
            || (s.attachments.is_some() && !attachment)
            || s.attachments.as_ref().is_some_and(|p| p.sending)
        {
            return;
        }
        if s.picker.open && s.picker.for_attachment == attachment {
            s.picker.open = false;
            s.picker.images.clear();
        } else {
            s.open_picker(Tab::Emoji, None);
            s.picker.for_attachment = attachment;
        }
    })
    .key(key);
    if s.picker.open && s.picker.for_attachment == attachment {
        layer.push(
            el().key("expression-picker-scrim")
                .absolute(0.0, 0.0)
                .w_full()
                .h_full()
                .on_press(|s: &mut State| {
                    s.picker.open = false;
                    s.picker.images.clear();
                }),
        );
        let content = panel(view(s))
            .radius(16.0)
            .role("dialog")
            .label(super::locale::tr(label_))
            .focus_return(key)
            .on_press(|_: &mut State| {})
            .on_key(|s: &mut State, event, _| {
                if event.key == rune::Key::Escape {
                    s.picker.open = false;
                    s.picker.images.clear();
                    true
                } else {
                    false
                }
            });
        layer.push(rune_kit::popover::anchored(
            kit(),
            content,
            key,
            Placement::TopStart,
            8.0,
        ));
    }
    button
}

fn view(s: &State) -> El {
    let mut search = TextInput::new(&s.picker.query)
        .leading_icon(&glyphs::SEARCH)
        .placeholder(super::locale::tr(if s.picker.tab == Tab::Emoji {
            "Search emoji"
        } else {
            "Search stickers"
        }))
        .width(None)
        .on_edit(|s: &mut State| &mut s.picker.query);
    if !s.picker.query.text.is_empty() {
        search = search.trailing(
            el().size(24.0, 24.0)
                .center()
                .role("button")
                .help(super::locale::tr("Clear search"))
                .on_press(|s: &mut State| s.picker.query.clear())
                .child(icon(&glyphs::CLOSE, 14.0).color(MUTED)),
        );
    }
    let mut body = col()
        .w(364.0)
        .gap(10.0)
        .padding(12.0)
        .child(search.build(kit()).h(36.0).press(Color::TRANSPARENT));
    if s.picker.tab == Tab::Emoji {
        let mut groups = row().w_full().gap(2.0);
        for (i, text) in GROUP_ICONS.iter().enumerate() {
            groups = groups.child(
                label(*text, 19.0, INK)
                    .size(32.0, 32.0)
                    .center()
                    .radius(8.0)
                    .fill(if i == s.picker.category {
                        MINT.alpha(0.16)
                    } else {
                        Color::TRANSPARENT
                    })
                    .role("button")
                    .help(super::locale::tr(
                        &(if i == 9 { "Recent" } else { GROUPS[i] }),
                    ))
                    .on_press_tx(move |s: &mut State, tx| {
                        s.picker.category = i;
                        s.picker.query.clear();
                        s.warm_picker(0..12);
                        tx.scroll_to_y("emoji-grid", 0.0);
                        tx.instant();
                    }),
            );
        }
        body = body.child(groups);
        let indices = s.picker.emoji_indices();
        let count = indices.len();
        let rows = count.div_ceil(8);
        body = body.child(
            col()
                .key("emoji-grid")
                .h(268.0)
                .w_full()
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .child(
                    col()
                        .w_full()
                        .growth(rune::GrowthSpec::default())
                        .virtual_rows(
                            rows,
                            41.0,
                            |_: &State, i| format!("emoji-row-{i}").into(),
                            |s: &State, r| {
                                let indices = s.picker.emoji_indices();
                                let mut line = row().gap(2.0).h(41.0);
                                for &i in indices.iter().skip(r * 8).take(8) {
                                    let mut cell = el()
                                        .size(40.0, 40.0)
                                        .radius(8.0)
                                        .center()
                                        .role("button")
                                        .label(&emojis()[i].name)
                                        .help(&emojis()[i].name)
                                        .hover(MINT.alpha(0.12))
                                        .on_press(move |s: &mut State| s.pick_emoji(i));
                                    cell =
                                        cell.child(if let Some(image) = s.picker.images.get(&i) {
                                            el().size(30.0, 30.0).pixels(image.pixels())
                                        } else {
                                            label(&emojis()[i].text, 27.0, INK)
                                        });
                                    line = line.child(cell);
                                }
                                line
                            },
                        )
                        .on_visible_rows(|s: &mut State, range| s.warm_picker(range)),
                ),
        );
        if count == 0 {
            body = body.child(label(super::locale::tr("No results"), 12.0, MUTED));
        }
    } else {
        let mut filters = row().gap(6.0);
        for (i, name) in ["Recent", "Favorites", "All"].iter().enumerate() {
            filters = filters.child(
                label(
                    super::locale::tr(name),
                    12.0,
                    if s.picker.filter == i { MINT } else { MUTED },
                )
                .px(12.0)
                .py(7.0)
                .radius(10.0)
                .fill(if s.picker.filter == i {
                    MINT.alpha(0.12)
                } else {
                    Color::TRANSPARENT
                })
                .role("button")
                .on_press_tx(move |s: &mut State, tx| {
                    s.picker.filter = i;
                    s.warm_picker(0..12);
                    tx.scroll_to_y("sticker-grid", 0.0);
                    tx.instant();
                }),
            );
        }
        filters = filters.child(icon_button(&SYNC, "Sync stickers", |s, _| {
            s.load_picker_catalog(true)
        }));
        body = body.child(filters);
        let count = s.picker.sticker_indices().len();
        body = body.child(
            col()
                .key("sticker-grid")
                .h(242.0)
                .w_full()
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .child(
                    col()
                        .w_full()
                        .growth(rune::GrowthSpec::default())
                        .virtual_rows(
                            count.div_ceil(4),
                            84.0,
                            |_: &State, i| format!("sticker-row-{i}").into(),
                            |s: &State, r| {
                                let indices = s.picker.sticker_indices();
                                let mut line = row().gap(4.0).h(84.0);
                                for i in indices.into_iter().skip(r * 4).take(4) {
                                    let sticker = &s.picker.stickers[i];
                                    let hash = sticker.hash.clone();
                                    let description = if sticker.label.is_empty() {
                                        "Send sticker"
                                    } else {
                                        &sticker.label
                                    };
                                    let mut cell = el()
                                        .size(80.0, 80.0)
                                        .center()
                                        .radius(10.0)
                                        .hover(MINT.alpha(0.10))
                                        .role("button")
                                        .label(description)
                                        .help(description)
                                        .on_press(move |s: &mut State| {
                                            if s.busy {
                                                return;
                                            }
                                            if let Some(chat) = s.selected.clone() {
                                                s.picker.open = false;
                                                s.picker.images.clear();
                                                s.tool(
                                                    "send_sticker",
                                                    json!({"chat":chat,"hash":hash}),
                                                );
                                            }
                                        });
                                    cell = cell.child(
                                        if let Some(pixels) = s
                                            .animations
                                            .pixels(&format!("sticker:{}", sticker.hash))
                                        {
                                            el().size(72.0, 72.0)
                                                .pixels_fit(pixels, rune::SurfaceFit::Fit)
                                        } else if let Some(image) =
                                            s.images.get(&format!("sticker:{}", sticker.hash))
                                        {
                                            el().size(72.0, 72.0)
                                                .pixels_fit(image.pixels(), rune::SurfaceFit::Fit)
                                        } else {
                                            icon(&super::view::STICKER, 24.0).color(MUTED)
                                        },
                                    );
                                    line = line.child(cell);
                                }
                                line
                            },
                        )
                        .on_visible_rows(|s: &mut State, range| s.warm_picker(range)),
                ),
        );
        if count == 0 {
            body = body.child(
                label(
                    super::locale::tr(if s.picker.loading {
                        "Syncing stickers…"
                    } else {
                        "No stickers here yet. Try Favorites or All."
                    }),
                    11.0,
                    MUTED,
                )
                .wrap(),
            );
        }
        if let Some(error) = &s.picker.error {
            body = body.child(label(super::locale::tr(error), 11.0, MUTED).wrap());
        }
    }
    let mut tabs = row().w_full().center().gap(12.0);
    for (tab, name) in [(Tab::Emoji, "Emoji"), (Tab::Stickers, "Stickers")]
        .into_iter()
        .filter(|(tab, _)| !s.picker.for_attachment || *tab == Tab::Emoji)
    {
        tabs = tabs.child(
            label(
                super::locale::tr(name),
                12.0,
                if s.picker.tab == tab { MINT } else { MUTED },
            )
            .px(20.0)
            .py(9.0)
            .radius(12.0)
            .fill(if s.picker.tab == tab {
                MINT.alpha(0.12)
            } else {
                Color::TRANSPARENT
            })
            .role("tab")
            .on_press(move |s: &mut State| {
                let target = s.picker.target.clone();
                s.open_picker(tab, target);
            }),
        );
    }
    body.child(tabs)
}
