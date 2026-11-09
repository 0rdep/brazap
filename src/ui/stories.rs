use super::{Command, State, media, view::*};
use crate::{
    assets::AssetRequest,
    model::{Message, StatusUpdate},
};
use rune::{Color, El, Icon, SurfaceFit, Tx, col, el, icon, row};
use rune_kit::glyphs;

pub(super) const STATUS: Icon = Icon {
    name: "status-updates",
    view: 24.0,
    body: r#"<path d="M12 3a9 9 0 0 1 7.8 4.5M21 12a9 9 0 0 1-4.5 7.8M12 21a9 9 0 0 1-7.8-4.5M3 12a9 9 0 0 1 4.5-7.8" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"/>"#,
};

pub(super) struct Playback {
    pub(super) reply: rune::TextEdit,
    request: Option<(String, String)>,
    replying: bool,
    reply_error: bool,
    replied: bool,
    queue: Vec<StatusUpdate>,
    index: usize,
    elapsed: f64,
    last_tick: Option<f64>,
    paused: bool,
    marked: bool,
}

fn author(s: &State, sender: &str) -> String {
    s.snapshot
        .chats
        .iter()
        .find(|c| c.matches_jid(sender))
        .map_or_else(|| sender.to_owned(), |c| c.jid.clone())
}

fn authors(s: &State) -> Vec<StatusUpdate> {
    let mut rows: Vec<StatusUpdate> = Vec::new();
    for update in &s.snapshot.statuses {
        if update.message.timestamp <= crate::model::now() - 86400 {
            continue;
        }
        let key = author(s, &update.message.sender);
        if let Some(row) = rows
            .iter_mut()
            .find(|r| author(s, &r.message.sender) == key)
        {
            let viewed = row.viewed && update.viewed;
            if update.message.timestamp > row.message.timestamp {
                *row = update.clone();
            }
            row.viewed = viewed;
        } else {
            rows.push(update.clone());
        }
    }
    rows.sort_by_key(|r| (r.viewed, std::cmp::Reverse(r.message.timestamp)));
    rows
}

impl State {
    pub(super) fn request_status_avatars(&mut self) {
        for status in authors(self) {
            self.request_avatar(&status.message.sender, status.message.sender_photo_version);
        }
    }
    pub(super) fn close_story(&mut self) {
        self.story = None;
        self.close_viewer();
        self.stop_voice();
    }

    pub(super) fn open_story(&mut self, sender: &str) {
        self.draft.focused = false;
        self.search.edit.focused = false;
        self.chat_search.edit.focused = false;
        let key = author(self, sender);
        let mut queue: Vec<_> = self
            .snapshot
            .statuses
            .iter()
            .filter(|s| author(self, &s.message.sender) == key)
            .cloned()
            .collect();
        let index = queue.iter().position(|s| !s.viewed).unwrap_or(0);
        for person in authors(self) {
            let other = author(self, &person.message.sender);
            if other != key && !person.viewed {
                let updates: Vec<_> = self
                    .snapshot
                    .statuses
                    .iter()
                    .filter(|s| author(self, &s.message.sender) == other)
                    .cloned()
                    .collect();
                let first_unread = updates.iter().position(|s| !s.viewed).unwrap_or(0);
                queue.extend(updates.into_iter().skip(first_unread));
            }
        }
        if queue.is_empty() {
            return;
        }
        self.story = Some(Playback {
            reply: rune::TextEdit::new(""),
            request: None,
            replying: false,
            reply_error: false,
            replied: false,
            queue,
            index,
            elapsed: 0.0,
            last_tick: None,
            paused: false,
            marked: false,
        });
        self.show_story();
    }

    fn show_story(&mut self) {
        let Some(story) = &mut self.story else {
            return;
        };
        story.elapsed = 0.0;
        story.last_tick = None;
        story.marked = false;
        story.reply = rune::TextEdit::new("");
        story.request = None;
        story.replying = false;
        story.reply_error = false;
        story.replied = false;
        let message = story.queue[story.index].message.clone();
        self.request_avatar(&message.sender, message.sender_photo_version);
        match message.kind.as_str() {
            "image" | "video" => self.open_image(message),
            "audio" => self.play_voice(message),
            _ => {
                self.close_viewer();
                self.stop_voice();
            }
        }
    }

    pub(super) fn step_story(&mut self, forward: bool) {
        let Some(story) = &mut self.story else {
            return;
        };
        if forward && story.index + 1 == story.queue.len() {
            self.close_story();
            return;
        }
        story.index = if forward {
            story.index + 1
        } else {
            story.index.saturating_sub(1)
        };
        self.show_story();
    }

    pub(super) fn pause_story(&mut self) {
        if let Some(story) = &mut self.story {
            story.paused = !story.paused;
        }
    }

    pub(super) fn send_status_reply(&mut self, quick: Option<&str>) {
        if self.snapshot.connection.phase != crate::model::Phase::Connected {
            return;
        }
        let Some(story) = &mut self.story else {
            return;
        };
        if story.replying {
            return;
        }
        let m = &story.queue[story.index].message;
        if m.from_me {
            return;
        }
        if let Some(text) = quick {
            story.reply.text = text.into();
        }
        let text = story.reply.text.trim().to_owned();
        if text.is_empty() {
            return;
        }
        let id = story
            .request
            .as_ref()
            .filter(|(_, sent)| sent == &text)
            .map(|(id, _)| id.clone())
            .unwrap_or_else(crate::outbox::new_id);
        story.request = Some((id.clone(), text.clone()));
        story.replying = true;
        story.reply_error = false;
        story.replied = false;
        if self
            .tx
            .send(Command::ReplyStatus(
                m.sender.clone(),
                m.id.clone(),
                text,
                id,
            ))
            .is_err()
        {
            story.replying = false;
            story.reply_error = true;
        }
    }

    pub(super) fn receive_status_reply(&mut self, id: String, result: Result<Message, String>) {
        let Some(story) = &mut self.story else {
            return;
        };
        if story
            .request
            .as_ref()
            .is_none_or(|(pending, _)| pending != &id)
        {
            return;
        }
        story.replying = false;
        match result {
            Ok(_) => {
                story.reply.text.clear();
                story.reply.focused = false;
                story.request = None;
                story.replied = true;
            }
            Err(error) => {
                log::warn!("Could not reply to status: {error}");
                story.reply_error = true;
            }
        }
    }

    pub(super) fn open_quoted_status(&mut self, sender: &str, id: &str) {
        if !self
            .snapshot
            .statuses
            .iter()
            .any(|s| s.message.id == id && s.message.sender == sender)
        {
            self.error = Some("This status has expired or is unavailable".into());
            return;
        }
        self.open_story(sender);
        if let Some(story) = &mut self.story
            && let Some(index) = story.queue.iter().position(|s| s.message.id == id)
        {
            story.index = index;
            self.show_story();
        }
    }
}

pub(super) fn chat_avatar(s: &State, chat: &crate::model::Chat) -> El {
    let unread = s.snapshot.statuses.iter().any(|u| {
        !u.viewed
            && chat.matches_jid(&u.message.sender)
            && u.message.timestamp > crate::model::now() - 86400
    });
    if !unread {
        return media::avatar(s, &chat.jid, &chat.name, chat.photo_version, 38.0);
    }
    let sender = chat.jid.clone();
    el().size(42.0, 42.0)
        .key(format!("status-avatar-{}", chat.jid))
        .radius(21.0)
        .border(2.0, MINT)
        .padding(3.0)
        .center()
        .role("button")
        .label(super::locale::format(
            "View status from {name}",
            &[("name", chat.name.clone())],
        ))
        .on_press(move |s: &mut State| s.open_story(&sender))
        .child(media::avatar(s, &chat.jid, &chat.name, chat.photo_version, 32.0).clip_rounded(16.0))
}

pub(super) fn list(s: &State) -> El {
    let mut list = col()
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .gap(6.0)
        .scroll(rune::ScrollSpec {
            stick_bottom: false,
        });
    let rows = authors(s);
    if rows.is_empty() {
        return list.child(
            label(super::locale::tr("No recent status updates"), 13.0, MUTED)
                .wrap()
                .padding(12.0),
        );
    }
    for update in rows {
        let m = &update.message;
        let sender = m.sender.clone();
        let name = if m.from_me {
            super::locale::tr("My status")
        } else {
            m.sender_name.clone()
        };
        list = list.child(
            row()
                .key(format!("status-author-{sender}"))
                .w_full()
                .gap(12.0)
                .padding(10.0)
                .center_y()
                .radius(12.0)
                .hover(Color::WHITE.alpha(0.04))
                .role("button")
                .label(super::locale::format(
                    "View status from {name}",
                    &[("name", name.clone())],
                ))
                .on_press(move |s: &mut State| s.open_story(&sender))
                .child(
                    el().size(46.0, 46.0)
                        .radius(23.0)
                        .border(2.0, if update.viewed { MUTED } else { MINT })
                        .center()
                        .child(media::avatar(s, &m.sender, &name, 0, 38.0).clip_rounded(19.0)),
                )
                .child(
                    col()
                        .gap(5.0)
                        .grow(1.0)
                        .min_w(0.0)
                        .child(title(name, 13.0).truncate())
                        .child(label(super::times::chat_date(m.timestamp), 11.0, MUTED)),
                ),
        );
    }
    list
}

pub(super) fn empty() -> El {
    col().w_full().h_full().center().gap(18.0)
        .child(icon(&STATUS,56.0).color(MINT))
        .child(title(super::locale::tr("Status"),24.0))
        .child(label(super::locale::tr("Select a status to view photos, videos, and updates. Statuses disappear after 24 hours."),14.0,MUTED).max_w(360.0).wrap().center())
}

fn image_key(m: &Message) -> String {
    AssetRequest::Image {
        chat: m.chat.clone(),
        sender: m.sender.clone(),
        id: m.id.clone(),
    }
    .key()
}

pub(super) fn poll(s: &mut State, elapsed: f64, tx: &mut Tx) -> bool {
    let Some(story) = &s.story else {
        return false;
    };
    let m = story.queue[story.index].message.clone();
    if m.timestamp <= crate::model::now() - 86400 {
        s.step_story(true);
        return true;
    }
    // A contact locked since opening must disappear from the active viewer too.
    if !s
        .snapshot
        .statuses
        .iter()
        .any(|u| u.message.id == m.id && u.message.sender == m.sender)
    {
        s.close_story();
        return true;
    }
    let paused = story.paused
        || !s.focused
        || story.reply.focused
        || !story.reply.text.is_empty()
        || story.replying;
    let player = if m.kind == "video" {
        s.video.as_ref()
    } else {
        s.voice.as_ref().and_then(|v| v.player.as_ref())
    };
    let status = player.map(|p| p.status());
    if let Some(player) = player
        && let Some(status) = &status
        && !status.loading
        && status.error.is_none()
        && !status.ended
        && status.playing == paused
    {
        player.toggle();
    }
    let failed = if m.kind == "image" {
        s.image_failures.contains_key(&image_key(&m))
    } else if m.kind == "video" {
        s.video_error.is_some() || status.as_ref().is_some_and(|p| p.error.is_some())
    } else {
        s.voice.as_ref().is_some_and(|v| v.error.is_some())
            || status.as_ref().is_some_and(|p| p.error.is_some())
    };
    let ready = match m.kind.as_str() {
        "image" => s.images.contains_key(&image_key(&m)),
        "video" | "audio" => status
            .as_ref()
            .is_some_and(|p| !p.loading && p.error.is_none()),
        _ => true,
    };
    let story = s.story.as_mut().unwrap();
    let delta = story
        .last_tick
        .map_or(0.0, |last| (elapsed - last).clamp(0.0, 0.25));
    story.last_tick = Some(elapsed);
    if failed && !paused {
        story.elapsed += delta;
        if story.elapsed >= 3.0 {
            s.step_story(true);
        }
        tx.wake_after(std::time::Duration::from_millis(100));
        return true;
    }
    if !ready || paused {
        return false;
    }
    if !story.marked {
        story.marked = true;
        for update in &mut s.snapshot.statuses {
            if update.message.sender == m.sender && update.message.id == m.id {
                update.viewed = true;
            }
        }
        let _ =
            s.tx.send(Command::ViewStatus(m.sender.clone(), m.id.clone()));
    }
    story.elapsed += delta;
    let done = if let Some(status) = status {
        status.ended
    } else {
        story.elapsed >= text_duration(&m)
    };
    if done {
        s.step_story(true);
    }
    tx.wake_after(std::time::Duration::from_millis(100));
    true
}

fn text_duration(m: &Message) -> f64 {
    if m.kind == "image" {
        5.0
    } else {
        (m.text.chars().count() as f64 / 18.0).clamp(5.0, 15.0)
    }
}

fn story_channel(value: u32) -> f32 {
    let value = value as f32 / 255.0;
    let linear = if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    };
    linear * 0.5
}

pub(super) fn viewer(s: &State) -> Option<El> {
    let story = s.story.as_ref()?;
    let m = &story.queue[story.index].message;
    let player = if m.kind == "video" {
        s.video.as_ref()
    } else {
        s.voice.as_ref().and_then(|v| v.player.as_ref())
    };
    let status = player.map(|p| p.status());
    let progress = status
        .as_ref()
        .map_or(story.elapsed / text_duration(m), |p| {
            p.duration.map_or(0.0, |d| p.position / d.max(0.1))
        })
        .clamp(0.0, 1.0);
    let mut bars = row().w_full().gap(4.0);
    for (i, update) in story
        .queue
        .iter()
        .enumerate()
        .filter(|(_, u)| author(s, &u.message.sender) == author(s, &m.sender))
    {
        let amount = if i < story.index {
            1.0
        } else if i == story.index {
            progress
        } else {
            0.0
        };
        let _ = update;
        bars = bars.child(
            el().grow(1.0)
                .h(3.0)
                .radius(2.0)
                .fill(Color::WHITE.alpha(0.2))
                .child(
                    el().w_frac(amount as f32)
                        .h_full()
                        .radius(2.0)
                        .fill(Color::WHITE),
                ),
        );
    }
    let name = if m.from_me {
        super::locale::tr("My status")
    } else {
        m.sender_name.clone()
    };
    let mut display = el()
        .grow(1.0)
        .min_h(0.0)
        .w_full()
        .center()
        .on_press(|s: &mut State| s.step_story(true))
        .clip_rounded(16.0)
        .fill(Color {
            r: 0.015,
            g: 0.04,
            b: 0.06,
            a: 1.0,
        });
    match m.kind.as_str() {
        "image" => {
            display = if let Some(image) = s.images.get(&image_key(m)) {
                display.pixels_fit(image.pixels(), SurfaceFit::Fit)
            } else {
                display.child(label(
                    super::locale::tr(if s.image_failures.contains_key(&image_key(m)) {
                        "Status media is unavailable"
                    } else {
                        "Loading image…"
                    }),
                    14.0,
                    MUTED,
                ))
            };
        }
        "video" => {
            display = if let Some(player) = &s.video {
                display.child(player.view())
            } else {
                display.child(label(super::locale::tr("Loading video…"), 14.0, MUTED))
            };
        }
        "audio" => {
            display = display.child(super::audio::view(s, m));
        }
        _ => {
            if let Some(argb) = m.background_argb {
                display = display.fill(Color {
                    r: story_channel((argb >> 16) & 255),
                    g: story_channel((argb >> 8) & 255),
                    b: story_channel(argb & 255),
                    a: 1.0,
                });
            }
            display = display
                .padding(28.0)
                .child(super::links::text(&m.text, 24.0).center());
        }
    }
    let error = if m.kind == "image" {
        s.image_failures
            .get(&image_key(m))
            .and_then(|(_, e)| e.as_ref())
    } else {
        s.video_error
            .as_ref()
            .or_else(|| status.as_ref().and_then(|p| p.error.as_ref()))
    };
    let mut card = col()
        .w_full()
        .max_w(560.0)
        .h_full()
        .gap(14.0)
        .padding(18.0)
        .radius(22.0)
        .fill(PANEL)
        .border(1.0, BORDER)
        .on_press(|_: &mut State| {})
        .child(bars)
        .child(
            row()
                .w_full()
                .gap(10.0)
                .center_y()
                .child(media::avatar(s, &m.sender, &name, 0, 34.0))
                .child(
                    col()
                        .grow(1.0)
                        .min_w(0.0)
                        .child(title(name, 14.0).truncate())
                        .child(label(super::times::chat_date(m.timestamp), 11.0, MUTED)),
                )
                .child(icon_button(
                    if story.paused {
                        &super::audio::PLAY
                    } else {
                        &super::audio::PAUSE
                    },
                    if story.paused {
                        "Resume status"
                    } else {
                        "Pause status"
                    },
                    |s, _| s.pause_story(),
                ))
                .child(icon_button(&glyphs::CLOSE, "Close status", |s, _| {
                    s.close_story()
                })),
        )
        .child(display);
    if matches!(m.kind.as_str(), "image" | "video") && !m.text.is_empty() {
        card = card.child(super::links::text(&m.text, 14.0));
    }
    if let Some(error) = error {
        card = card.child(label(super::locale::tr(error), 12.0, MUTED).wrap());
    }
    if !m.from_me {
        let enabled =
            s.snapshot.connection.phase == crate::model::Phase::Connected && !story.replying;
        let edit = rune::text_input(&story.reply, super::locale::style(13.0, 450.0))
            .key("status-reply-input")
            .w_full()
            .color(INK)
            .placeholder(
                super::locale::tr(if enabled {
                    "Reply to this status…"
                } else if story.replying {
                    "Sending…"
                } else {
                    "Connect to reply to status"
                }),
                MUTED,
            )
            .caret(INK, kit().selection());
        let edit = if enabled {
            edit.on_edit(|s: &mut State| &mut s.story.as_mut().unwrap().reply)
                .on_submit(|s: &mut State, _| s.send_status_reply(None))
        } else {
            edit
        };
        card = card.child(
            row()
                .w_full()
                .gap(10.0)
                .center_y()
                .child(
                    rune::auto_grow(edit, 1, 3)
                        .grow(1.0)
                        .min_w(0.0)
                        .padding(10.0)
                        .radius(12.0)
                        .fill(Color::WHITE.alpha(0.05))
                        .border(1.0, CONTROL_BORDER),
                )
                .child(icon_button(&SEND, "Send status reply", |s, _| {
                    s.send_status_reply(None)
                })),
        );
        let mut quick = row().gap(10.0).center();
        for emoji in ["❤️", "😂", "🔥", "👏", "😮", "😢"] {
            quick = quick.child(
                el().size(36.0, 32.0)
                    .center()
                    .radius(10.0)
                    .hover(Color::WHITE.alpha(0.06))
                    .role("button")
                    .label(super::locale::format(
                        "Reply with {emoji}",
                        &[("emoji", emoji.into())],
                    ))
                    .on_press(move |s: &mut State| s.send_status_reply(Some(emoji)))
                    .child(label(emoji, 21.0, INK)),
            );
        }
        card = card.child(quick);
        if story.reply_error {
            card = card.child(
                label(
                    super::locale::tr("Could not send status reply. Try again."),
                    12.0,
                    MUTED,
                )
                .wrap(),
            );
        }
        if story.replied {
            card = card.child(label(super::locale::tr("Reply sent"), 12.0, MINT));
        }
    }
    card = card.child(
        row()
            .w_full()
            .gap(12.0)
            .child(button("Previous status", |s, _| s.step_story(false)))
            .child(el().grow(1.0))
            .child(button("Next status", |s, _| s.step_story(true))),
    );
    Some(
        el().key("status-viewer")
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .padding(20.0)
            .fill(Color::BLACK.alpha(0.88))
            .backdrop_blur(18.0)
            .center()
            .role("dialog")
            .label(super::locale::tr("Status viewer"))
            .on_press(|s: &mut State| s.close_story())
            .child(card),
    )
}
