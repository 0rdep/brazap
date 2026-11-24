//! `@` mentions in the composer: a list of the group's members appears while
//! typing `@name`; picking one inserts `@Name`, and sending turns it into the
//! `@<number>` WhatsApp expects plus the member's address in `mentioned_jid`.
use super::{Command, State, media, view::*};
use crate::model::{Member, Mention};
use rune::{Color, El, Span, col, el, row};

/// Members listed at once; the list scrolls.
const SHOWN: usize = 300;
/// The "everyone" entry: its tag is translated (`@todos`, `@all`), and sending it
/// tags every member, which notifies each of them.
const EVERYONE: &str = "everyone";
/// How WhatsApp writes the tag for everyone in the message text.
pub(super) const EVERYONE_TOKEN: &str = "@all";

fn everyone() -> Member {
    Member {
        jid: EVERYONE.into(),
        name: super::locale::tr("everyone_tag"),
        phone: String::new(),
    }
}

impl State {
    /// Where mentions are typed: an attachment's focused caption, else the
    /// composer; with the key its picked tags are kept under.
    pub(super) fn mention_field(&self) -> Option<(String, &rune::TextEdit)> {
        let chat = self.selected.as_ref()?;
        if let Some(item) = self
            .attachments
            .as_ref()
            .and_then(|panel| panel.items.iter().find(|item| item.caption.focused))
        {
            return Some((caption_key(item.id()), &item.caption));
        }
        Some((chat.clone(), &self.draft))
    }

    fn mention_field_mut(&mut self) -> Option<&mut rune::TextEdit> {
        if let Some(item) = self
            .attachments
            .as_mut()
            .and_then(|panel| panel.items.iter_mut().find(|item| item.caption.focused))
        {
            return Some(&mut item.caption);
        }
        Some(&mut self.draft)
    }

    /// The `@query` being typed right before the caret in a group, and where it starts.
    pub(super) fn mention_query(&self) -> Option<(usize, String)> {
        let chat = self.selected.as_deref()?;
        let (key, edit) = self.mention_field()?;
        if !chat.ends_with("@g.us")
            || !edit.focused
            || edit.anchor != edit.focus
            || edit.preedit().is_some()
            || (key == chat && self.editing.is_some())
        {
            return None;
        }
        let before = edit.text.get(..edit.focus)?;
        let at = before.rfind('@')?;
        let query = &before[at + 1..];
        let starts_word = before[..at]
            .chars()
            .next_back()
            .is_none_or(char::is_whitespace);
        (starts_word
            && query.chars().count() <= 30
            && !query.contains(char::is_whitespace)
            && self.mention_dismissed != Some(at))
        .then(|| (at, query.to_lowercase()))
    }

    /// Members matching the query: names starting with it first.
    pub(super) fn mention_candidates(&self) -> Vec<Member> {
        let Some((_, query)) = self.mention_query() else {
            return Vec::new();
        };
        // Members still loading, or none besides this account: "everyone" is
        // offered all the same.
        let members = self
            .selected
            .as_ref()
            .and_then(|c| self.members.get(c))
            .map(Vec::as_slice)
            .unwrap_or_default();
        let digits: String = query.chars().filter(char::is_ascii_digit).collect();
        let tag = super::locale::tr("everyone_tag").to_lowercase();
        let offer_everyone = tag.starts_with(&query)
            || ["all", "everyone", "todos"]
                .iter()
                .any(|t| t.starts_with(&query));
        let mut found: Vec<(bool, &Member)> = members
            .iter()
            .filter_map(|m| {
                let name = m.name.to_lowercase();
                let starts = name.starts_with(&query)
                    || name.split_whitespace().any(|w| w.starts_with(&query));
                let matches = query.is_empty()
                    || starts
                    || name.contains(&query)
                    || (!digits.is_empty() && m.phone.contains(&digits));
                matches.then_some((!starts, m))
            })
            .collect();
        found.sort_by_key(|(later, _)| *later);
        // "Everyone" always comes first while it matches.
        offer_everyone
            .then(everyone)
            .into_iter()
            .chain(found.into_iter().take(SHOWN).map(|(_, m)| m.clone()))
            .collect()
    }

    /// Ask for the group's members the first time `@` is typed there.
    pub(super) fn poll_mentions(&mut self) {
        let Some((at, query)) = self.mention_query() else {
            self.mention_seen = None;
            return;
        };
        if self.mention_seen.as_ref() != Some(&(at, query.clone())) {
            self.mention_seen = Some((at, query));
            self.mention_index = 0;
        }
        if let Some(chat) = self.selected.clone()
            && !self.members.contains_key(&chat)
            && self.members_pending.insert(chat.clone())
        {
            let _ = self.tx.send(Command::Members(chat));
        }
    }

    pub(super) fn pick_mention(&mut self, member: Member) {
        let Some((at, _)) = self.mention_query() else {
            return;
        };
        let label = format!("@{}", shown_name(&member));
        let Some((key, _)) = self.mention_field() else {
            return;
        };
        if let Some(edit) = self.mention_field_mut() {
            edit.set_selection(at, edit.focus);
            edit.insert(&format!("{label} "));
        }
        let mentions = self.draft_mentions.entry(key).or_default();
        mentions.retain(|(shown, _)| *shown != label);
        mentions.push((label, member));
        self.mention_seen = None;
    }

    /// The draft as sent: each picked `@Name` still in the text becomes `@<number>`.
    pub(super) fn outgoing_text(&mut self, chat: &str, text: &str) -> (String, Vec<Mention>, bool) {
        let mut text = text.to_owned();
        let mut mentions = Vec::new();
        let mut everyone = false;
        for (label, member) in self.draft_mentions.remove(chat).unwrap_or_default() {
            if member.jid == EVERYONE {
                // WhatsApp's tag for everyone is `@all` whatever the language; phones
                // show it translated, as Brazap does.
                if whole_tag(&text, &label) {
                    text = replace_tag(&text, &label, EVERYONE_TOKEN);
                    everyone = true;
                }
                continue;
            }
            let mention = Mention {
                jid: member.jid.clone(),
                name: member.name.clone(),
                phone: member.phone.clone(),
                me: false,
            };
            let token = mention.token();
            let mut found = false;
            let mut from = 0;
            while let Some(i) = text[from..].find(&label).map(|i| i + from) {
                let end = i + label.len();
                // A whole tag only: "@Ana" must not match inside "@Anabela".
                if text[end..]
                    .chars()
                    .next()
                    .is_none_or(|c| !c.is_alphanumeric())
                {
                    text.replace_range(i..end, &token);
                    from = i + token.len();
                    found = true;
                } else {
                    from = end;
                }
            }
            if found && !mentions.iter().any(|m: &Mention| m.jid == mention.jid) {
                mentions.push(mention);
            }
        }
        (text, mentions, everyone)
    }

    /// Picked `@Name` tags in a field's text, highlighted like mentions in messages.
    pub(super) fn mention_ranges(&self, key: &str, text: &str) -> Vec<std::ops::Range<usize>> {
        let Some(mentions) = self.draft_mentions.get(key) else {
            return Vec::new();
        };
        let mut ranges = Vec::new();
        for (label, _) in mentions {
            let mut from = 0;
            while let Some(i) = text[from..].find(label.as_str()).map(|i| i + from) {
                ranges.push(i..i + label.len());
                from = i + label.len();
            }
        }
        ranges
    }
}

/// The key a caption's picked tags are kept under.
pub(super) fn caption_key(id: u64) -> String {
    format!("caption:{id}")
}

/// Whether the mention list belongs to the field kept under `key`.
pub(super) fn popup_for(s: &State, key: &str) -> Option<El> {
    if s.mention_field().is_some_and(|(field, _)| field == key) {
        popup(s)
    } else {
        None
    }
}

/// `text` with each whole `label` tag replaced by `with`.
pub(super) fn replace_tag(text: &str, label: &str, with: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find(label) {
        let end = i + label.len();
        let whole = rest[end..]
            .chars()
            .next()
            .is_none_or(|c| !c.is_alphanumeric())
            && (i == 0
                || rest[..i]
                    .chars()
                    .next_back()
                    .is_some_and(char::is_whitespace));
        out.push_str(&rest[..i]);
        out.push_str(if whole { with } else { label });
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

/// Whether `label` appears in `text` as a whole tag, not inside a longer word.
fn whole_tag(text: &str, label: &str) -> bool {
    let mut from = 0;
    while let Some(i) = text[from..].find(label).map(|i| i + from) {
        let end = i + label.len();
        if text[end..]
            .chars()
            .next()
            .is_none_or(|c| !c.is_alphanumeric())
        {
            return true;
        }
        from = end;
    }
    false
}

fn shown_name(member: &Member) -> String {
    if member.name.is_empty() {
        member.phone.clone()
    } else {
        member.name.clone()
    }
}

/// Up/Down choose, Enter or Tab insert, Esc closes; other keys reach the composer.
pub(super) fn composer_key(s: &mut State, event: &rune::KeyEvent, tx: &mut rune::Tx) -> bool {
    let candidates = s.mention_candidates();
    if !candidates.is_empty() && !event.mods.shift {
        let last = candidates.len() - 1;
        match event.key {
            rune::Key::Up | rune::Key::Down => {
                s.mention_index = match event.key {
                    rune::Key::Up => s.mention_index.checked_sub(1).unwrap_or(last),
                    _ if s.mention_index >= last => 0,
                    _ => s.mention_index + 1,
                };
                let jid = &candidates[s.mention_index.min(last)].jid;
                tx.scroll_into_view_in("mention-scroll", format!("mention-{jid}"));
                return true;
            }
            rune::Key::Enter | rune::Key::Tab => {
                if !tx.key_repeat() {
                    s.pick_mention(candidates[s.mention_index.min(last)].clone());
                }
                return true;
            }
            rune::Key::Escape => {
                s.mention_dismissed = s.mention_query().map(|(at, _)| at);
                return true;
            }
            _ => {}
        }
    }
    super::message_enter(s, event, tx)
}

/// The member list above the composer while a mention is typed.
pub(super) fn popup(s: &State) -> Option<El> {
    let (_, query) = s.mention_query()?;
    let chat = s.selected.as_ref()?;
    let candidates = s.mention_candidates();
    let mut list = col().key("mention-list").w_full().gap(2.0);
    if candidates.is_empty() {
        let note = if s.members.contains_key(chat) {
            if query.is_empty() {
                return None;
            }
            "No members match"
        } else {
            "Loading members…"
        };
        list = list.child(label(super::locale::tr(note), 12.0, MUTED).padding(10.0));
    }
    let last = candidates.len().saturating_sub(1);
    for (i, member) in candidates.into_iter().enumerate() {
        let selected = i == s.mention_index.min(last);
        let all = member.jid == EVERYONE;
        let name = if all {
            format!("@{}", member.name)
        } else {
            shown_name(&member)
        };
        let mut entry = row()
            .key(format!("mention-{}", member.jid))
            .w_full()
            .px(10.0)
            .py(6.0)
            .gap(10.0)
            .center_y()
            .radius(10.0)
            .fill(if selected {
                MINT.alpha(0.13)
            } else {
                Color::TRANSPARENT
            })
            .hover(Color::WHITE.alpha(0.05))
            .cursor(rune::Cursor::Pointer)
            .role("option")
            .label(name.clone())
            .child(if all {
                el().size(28.0, 28.0)
                    .radius(9.0)
                    .center()
                    .fill(MINT.alpha(0.12))
                    .child(rune::icon(&super::sidebar::GROUP_ICON, 16.0).color(MINT))
            } else {
                media::avatar(s, &member.jid, &name, 0, 28.0)
            })
            .child(label(name, 13.0, INK).grow(1.0).min_w(0.0).truncate());
        if all {
            entry = entry.child(label(super::locale::tr("Mention everyone"), 11.0, MUTED));
        } else if !member.name.is_empty() && !member.phone.is_empty() {
            entry = entry.child(label(member.phone.clone(), 11.0, MUTED));
        }
        list = list.child(entry.on_press(move |s: &mut State| {
            s.pick_mention(member.clone());
            s.draft.focused = true;
        }));
    }
    Some(
        el().key("mention-popup")
            .w_full()
            .max_w(420.0)
            .padding(6.0)
            .radius(14.0)
            .fill(PANEL)
            .border(1.0, BORDER)
            .shadow(10.0, 30.0, Color::BLACK.alpha(0.3))
            .child(
                col()
                    .key("mention-scroll")
                    .w_full()
                    .max_h(super::locale::scale(260.0))
                    .scroll(rune::ScrollSpec {
                        stick_bottom: false,
                    })
                    .child(list),
            ),
    )
}

/// Live styling for the composer: WhatsApp formatting with its markers dimmed,
/// and picked mentions highlighted.
pub(super) fn field_spans(
    s: &State,
    key: &str,
    text: &str,
) -> (Vec<Span>, Vec<(usize, usize, Option<Color>)>) {
    use super::formatting::Style;
    let mentions = s.mention_ranges(key, text);
    let mut protected: Vec<_> = crate::links::detect(text)
        .into_iter()
        .map(|(range, _)| range)
        .collect();
    protected.extend(mentions.iter().cloned());
    let (markers, styles) = super::formatting::in_place(text, &protected);
    let mut spans = Vec::new();
    let mut strikes = Vec::new();
    for (range, style) in styles {
        let span = Span::new(range.start, range.end);
        spans.push(match style {
            Style::Bold => span.bold(),
            Style::Italic => span.italic(),
            Style::Mono => span.mono(),
            Style::Code => span.mono().bg(INK.alpha(0.1), 2.0),
            Style::Quote => span.color(INK.alpha(0.68)),
            Style::Strike => {
                strikes.push((range.start, range.end, None));
                continue;
            }
        });
    }
    spans.extend(
        markers
            .into_iter()
            .map(|range| Span::new(range.start, range.end).color(MUTED)),
    );
    spans.extend(
        mentions
            .into_iter()
            .map(|range| Span::new(range.start, range.end).color(MINT)),
    );
    (spans, strikes)
}
