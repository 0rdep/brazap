use super::{Command, State, locale, view::*};
use crate::model::{Chat, MessageCursor, SearchHit, SearchResults};
use rune::{Color, El, ScrollSpec, TextEdit, Tx, col, el, icon, row};
use rune_kit::{TextInput, glyphs};
use std::time::Duration;

const DEBOUNCE: f64 = 0.18;

#[derive(Clone)]
pub(super) struct Request {
    pub query: String,
    pub chat: Option<String>,
    generation: u64,
    pub cursor: Option<MessageCursor>,
}

pub(super) struct Panel {
    pub edit: TextEdit,
    pub open: bool,
    pub results: SearchResults,
    pub selected: Option<usize>,
    pub error: Option<String>,
    observed: String,
    generation: u64,
    due: Option<f64>,
    in_flight: Option<u64>,
}

impl Default for Panel {
    fn default() -> Self {
        Self {
            edit: TextEdit::new(""),
            open: false,
            results: Default::default(),
            selected: None,
            error: None,
            observed: String::new(),
            generation: 0,
            due: None,
            in_flight: None,
        }
    }
}

impl Panel {
    pub fn clear(&mut self) {
        self.edit.clear();
        self.observed.clear();
        self.generation = self.generation.wrapping_add(1);
        self.results = Default::default();
        self.selected = None;
        self.error = None;
        self.due = None;
        // Leave a running request tracked until it completes: rapid edits must
        // not queue parallel archive scans. Its generation makes it harmless.
    }

    pub fn searching(&self) -> bool {
        !self.edit.text.trim().is_empty()
            && (self.due.is_some()
                || self.in_flight.is_some()
                || self.observed != self.edit.text.trim())
    }

    fn tick(&mut self, chat: Option<String>, elapsed: f64, tx: &mut Tx) -> (bool, Option<Request>) {
        let query = self.edit.text.trim();
        let changed = query != self.observed;
        if changed {
            self.observed = query.to_owned();
            self.generation = self.generation.wrapping_add(1);
            self.results = Default::default();
            self.selected = None;
            self.error = None;
            self.due = (!query.is_empty()).then_some(elapsed + DEBOUNCE);
            if self.due.is_some() {
                tx.wake_after(Duration::from_secs_f64(DEBOUNCE));
            }
        }
        if self.in_flight.is_none() && self.due.is_some_and(|at| elapsed >= at) {
            self.due = None;
            self.in_flight = Some(self.generation);
            return (
                true,
                Some(Request {
                    query: self.observed.clone(),
                    chat,
                    generation: self.generation,
                    cursor: None,
                }),
            );
        }
        (changed, None)
    }

    pub fn receive(&mut self, request: &Request, result: Result<SearchResults, String>) {
        if self.in_flight == Some(request.generation) {
            self.in_flight = None;
        }
        if self.generation != request.generation || self.edit.text.trim() != request.query {
            return;
        }
        match result {
            Ok(results) if request.cursor.is_some() => {
                self.results.next = results.next;
                for hit in results.messages {
                    if !self
                        .results
                        .messages
                        .iter()
                        .any(|old| same_message(old, &hit))
                    {
                        self.results.messages.push(hit);
                    }
                }
            }
            Ok(results) => self.results = results,
            Err(error) => self.error = Some(error),
        }
    }
}

fn same_message(a: &SearchHit, b: &SearchHit) -> bool {
    a.message.chat == b.message.chat
        && a.message.id == b.message.id
        && (a.message.from_me == b.message.from_me)
        && (!a.message.chat.ends_with("@g.us") || a.message.sender == b.message.sender)
}

pub(super) fn poll(s: &mut State, elapsed: f64, tx: &mut Tx) -> bool {
    let mut changed = false;
    if s.section == super::Section::Messages {
        let (update, request) = s.search.tick(None, elapsed, tx);
        changed |= update;
        if let Some(request) = request {
            let _ = s.tx.send(Command::Search(request));
        }
    }
    if s.chat_search.open {
        let (update, request) = s.chat_search.tick(s.selected.clone(), elapsed, tx);
        changed |= update;
        if let Some(request) = request {
            let _ = s.tx.send(Command::Search(request));
        }
    }
    changed
}

pub(super) fn matching_chats(s: &State) -> Vec<&Chat> {
    let query = s.search.edit.text.trim().to_lowercase();
    let digits: String = query.chars().filter(char::is_ascii_digit).collect();
    s.snapshot
        .chats
        .iter()
        .filter(|chat| {
            chat.name.to_lowercase().contains(&query)
                || (digits.len() >= 3 && chat.phone.contains(&digits))
                || s.search
                    .results
                    .chats
                    .iter()
                    .any(|hit| chat.matches_jid(&hit.jid))
        })
        .collect()
}

impl State {
    pub(super) fn open_chat_search(&mut self, tx: &mut Tx) {
        if self.selected.is_none() {
            return;
        }
        self.chat_search.open = true;
        self.picker.open = false;
        self.attachments_open = false;
        self.composer_focus_at = None;
        self.search.edit.focused = false;
        self.draft.focused = false;
        self.chat_search.edit.focused = false;
        self.search_focus_at = Some(std::time::Instant::now() + Duration::from_millis(16));
        tx.wake_after(Duration::from_millis(16));
    }

    pub(super) fn focus_chat_search(&mut self) {
        self.search.edit.focused = false;
        self.draft.focused = false;
        self.composer_focus_at = None;
        self.chat_search.edit.focused = true;
        self.chat_search
            .edit
            .set_selection(0, self.chat_search.edit.text.len());
    }

    pub(super) fn close_chat_search(&mut self) {
        self.search_focus_at = None;
        self.chat_search.open = false;
        self.chat_search.edit.focused = false;
        self.focus_composer();
    }

    pub(super) fn choose_search_hit(&mut self, index: usize, tx: &mut Tx) {
        let Some(hit) = self.chat_search.results.messages.get(index).cloned() else {
            return;
        };
        self.chat_search.selected = Some(index);
        self.chat_search.edit.focused = true;
        self.draft.focused = false;
        self.jump_to_quote(
            &crate::model::Quote {
                id: hit.message.id,
                sender: hit.message.sender,
                text: String::new(),
            },
            &hit.message.chat,
            tx,
        );
    }

    pub(super) fn step_search_hit(&mut self, older: bool, tx: &mut Tx) {
        let count = self.chat_search.results.messages.len();
        if count == 0 {
            return;
        }
        let index = match self.chat_search.selected {
            None => 0,
            Some(index) if older => (index + 1).min(count - 1),
            Some(index) => index.saturating_sub(1),
        };
        self.choose_search_hit(index, tx);
    }

    fn more_search_hits(&mut self) {
        if self.chat_search.searching() {
            return;
        }
        let Some(cursor) = self.chat_search.results.next.clone() else {
            return;
        };
        self.chat_search.in_flight = Some(self.chat_search.generation);
        let _ = self.tx.send(Command::Search(Request {
            query: self.chat_search.observed.clone(),
            chat: self.selected.clone(),
            generation: self.chat_search.generation,
            cursor: Some(cursor),
        }));
    }
}

pub(super) fn panel(s: &State) -> El {
    let mut input = TextInput::new(&s.chat_search.edit)
        .placeholder(locale::tr("Search this chat"))
        .leading_icon(&glyphs::SEARCH)
        .width(None)
        .height(locale::scale(38.0))
        .on_edit(|s: &mut State| &mut s.chat_search.edit)
        .on_key(|s: &mut State, event, tx| {
            if event.key == rune::Key::Escape {
                s.close_chat_search();
                return true;
            }
            if event.key == rune::Key::Enter {
                s.step_search_hit(!event.mods.shift, tx);
                return true;
            }
            false
        });
    if !s.chat_search.edit.text.is_empty() {
        input = input.trailing(
            el().size(24.0, 24.0)
                .center()
                .role("button")
                .help(locale::tr("Clear search"))
                .label(locale::tr("Clear search"))
                .on_press(|s: &mut State| s.chat_search.clear())
                .child(icon(&glyphs::CLOSE, 14.0).color(MUTED)),
        );
    }
    let count = s.chat_search.results.messages.len();
    let summary = if s.chat_search.searching() {
        locale::tr("Searching…")
    } else if s.chat_search.edit.text.trim().is_empty() {
        locale::tr("Search messages in this chat")
    } else if count == 0 {
        locale::tr("No matching messages")
    } else {
        locale::format(
            "{current} of {total} matches",
            &[
                (
                    "current",
                    s.chat_search
                        .selected
                        .map_or("–".into(), |i| (i + 1).to_string()),
                ),
                (
                    "total",
                    format!(
                        "{count}{}",
                        if s.chat_search.results.next.is_some() {
                            "+"
                        } else {
                            ""
                        }
                    ),
                ),
            ],
        )
    };
    let mut list = col()
        .key("chat-search-results")
        .grow(1.0)
        .min_h(0.0)
        .w_full()
        .scroll(ScrollSpec {
            stick_bottom: false,
        });
    list = list.child(
        col()
            .w_full()
            .growth(rune::GrowthSpec::default())
            .instant_changes()
            .virtual_rows(
                count,
                locale::scale(86.0),
                |s: &State, i| {
                    let m = &s.chat_search.results.messages[i].message;
                    format!("chat-match-{}-{}", m.sender, m.id).into()
                },
                |s: &State, i| result_row(s, i),
            ),
    );
    if let Some(error) = &s.chat_search.error {
        list = list.child(label(locale::tr(error), 12.0, MUTED).wrap().padding(12.0));
    }
    if s.chat_search.results.next.is_some() && !s.chat_search.searching() {
        list = list.child(button("Load older matches", |s, _| s.more_search_hits()));
    }
    col()
        .key("chat-search-panel")
        .w(294.0)
        .h_full()
        .padding(12.0)
        .gap(10.0)
        .border(1.0, BORDER)
        .fill(Color::WHITE.alpha(0.015))
        .instant_changes()
        .child(
            row()
                .gap(8.0)
                .center_y()
                .child(
                    title(locale::tr("Search messages"), 14.0)
                        .grow(1.0)
                        .min_w(0.0)
                        .truncate(),
                )
                .child(icon_button(&glyphs::CLOSE, "Close search", |s, _| {
                    s.close_chat_search()
                })),
        )
        .child(input.build(kit()).press(Color::TRANSPARENT))
        .child(
            row()
                .center_y()
                .gap(5.0)
                .child(label(summary, 11.0, MUTED).grow(1.0).min_w(0.0).truncate())
                .child(icon_button(&glyphs::CHEVRON_UP, "Newer match", |s, tx| {
                    s.step_search_hit(false, tx)
                }))
                .child(icon_button(
                    &glyphs::CHEVRON_DOWN,
                    "Older match",
                    |s, tx| s.step_search_hit(true, tx),
                )),
        )
        .child(list)
}

fn result_row(s: &State, index: usize) -> El {
    let hit = &s.chat_search.results.messages[index];
    let id = hit.message.id.clone();
    let sender = hit.message.sender.clone();
    let author = if hit.message.from_me {
        locale::tr("You")
    } else {
        hit.message.sender_name.clone()
    };
    col()
        .w_full()
        .h(locale::scale(86.0))
        .padding(10.0)
        .gap(5.0)
        .radius(12.0)
        .fill(if s.chat_search.selected == Some(index) {
            MINT.alpha(0.12)
        } else {
            Color::TRANSPARENT
        })
        .hover(Color::WHITE.alpha(0.04))
        .role("button")
        .label(hit.excerpt.clone())
        .on_press_tx(move |s: &mut State, tx| {
            if let Some(index) = s
                .chat_search
                .results
                .messages
                .iter()
                .position(|hit| hit.message.id == id && hit.message.sender == sender)
            {
                s.choose_search_hit(index, tx);
            }
        })
        .child(
            row()
                .gap(6.0)
                .child(title(author, 11.0).truncate().grow(1.0).min_w(0.0))
                .child(label(
                    super::times::chat_date(hit.message.timestamp),
                    10.0,
                    MUTED,
                )),
        )
        .child(label(&hit.excerpt, 12.0, INK).wrap().max_lines(2))
}
