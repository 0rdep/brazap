use super::{Command, State, media, view::*};
use crate::model::{Chat, Message};
use rune::{Color, El, Icon, ScrollSpec, TextEdit, col, el, icon, row};
use rune_kit::{TextInput, glyphs};
use serde_json::json;

const REACT: Icon = Icon {
    name: "react-to-message",
    view: 24.0,
    body: r#"<circle cx="12" cy="12" r="9" fill="none" stroke="currentColor" stroke-width="1.6"/><path d="M8 14c2 3 6 3 8 0M8 9h.1M16 9h.1" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/>"#,
};
const REPLY: Icon = Icon {
    name: "reply-to-message",
    view: 24.0,
    body: r#"<path d="m10 5-7 6 7 6v-4c6 0 9 2 11 6-1-9-4-10-11-10Z" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linejoin="round"/>"#,
};
const FORWARD: Icon = Icon {
    name: "forward-message",
    view: 24.0,
    body: r#"<path d="m14 5 7 6-7 6v-4c-6 0-9 2-11 6 1-9 4-10 11-10Z" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linejoin="round"/>"#,
};
const CHECK: Icon = Icon {
    name: "selected-message",
    view: 24.0,
    body: r#"<path d="m6 12 4 4 8-8" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"/>"#,
};

pub(super) struct Forwarding {
    pub source: String,
    pub selected: Vec<Message>,
    pub picker: bool,
    pub search: TextEdit,
    pub destination: Option<String>,
    pub sending: bool,
    pub error: Option<String>,
}

impl State {
    pub(super) fn toggle_forward(&mut self, message: &Message) {
        if !message.actionable() {
            return;
        }
        if let Some(flow) = &mut self.forwarding {
            if flow.sending {
                return;
            }
            if let Some(index) = flow
                .selected
                .iter()
                .position(|m| m.id == message.id && m.sender == message.sender)
            {
                flow.selected.remove(index);
            } else if flow.selected.len() < 50 {
                flow.selected.push(message.clone());
            } else {
                flow.error = Some("You can forward up to 50 messages at once.".into());
            }
        }
    }

    pub(super) fn begin_forward(&mut self, message: Message) {
        self.reply = None;
        self.reaction_menu = None;
        self.attachments_open = false;
        self.forwarding = Some(Forwarding {
            source: message.chat.clone(),
            selected: vec![message],
            picker: false,
            search: TextEdit::new(""),
            destination: None,
            sending: false,
            error: None,
        });
    }

    fn forward_selected(&mut self) {
        if let Some(flow) = &mut self.forwarding
            && let Some(destination) = &flow.destination
            && !flow.selected.is_empty()
            && !flow.sending
        {
            flow.sending = true;
            flow.error = None;
            let _ = self.tx.send(Command::Forward(
                flow.source.clone(),
                destination.clone(),
                flow.selected.iter().map(Message::cursor).collect(),
            ));
        }
    }

    pub(super) fn send_reaction(&mut self, message: Message, emoji: String) {
        let emoji = if message
            .reactions
            .iter()
            .any(|r| r.sender == "self" && r.emoji == emoji)
        {
            String::new()
        } else {
            emoji
        };
        self.reaction_menu = None;
        self.tool(
            "react_message",
            json!({"chat":message.chat,"target":message.cursor(),"emoji":emoji}),
        );
    }
}

fn action(glyph: &'static Icon, name: &'static str, f: impl Fn(&mut State) + 'static) -> El {
    el().size(29.0, 29.0)
        .center()
        .radius(10.0)
        .role("button")
        .label(super::locale::tr(name))
        .help(super::locale::tr(name))
        .fill(PANEL)
        .border(1.0, BORDER)
        .hover(MINT.alpha(0.15))
        .on_press(f)
        .child(icon(glyph, 16.0).color(MUTED))
}

pub(super) fn toolbar(message: &Message) -> El {
    let react = message.clone();
    let reply = message.clone();
    let forward = message.clone();
    row()
        .key(format!("message-actions-{}-{}", message.sender, message.id))
        .gap(4.0)
        .group_opacity(0.0, 1.0)
        .child(action(&REACT, "React with an emoji", move |s| {
            s.reaction_text.clear();
            s.reaction_menu = Some(react.clone());
        }))
        .child(action(&REPLY, "Reply to message", move |s| {
            s.reply = Some(reply.clone());
            s.focus_composer();
            s.reaction_menu = None;
            s.attachments_open = false;
        }))
        .child(action(&FORWARD, "Forward message", move |s| {
            s.picker.open = false;
            s.begin_forward(forward.clone())
        }))
}

fn checkbox(s: &State, message: &Message) -> El {
    let selected = s.forwarding.as_ref().is_some_and(|f| {
        f.selected
            .iter()
            .any(|m| m.id == message.id && m.sender == message.sender)
    });
    let mut check = el()
        .size(25.0, 25.0)
        .radius(8.0)
        .center()
        .border(1.0, if selected { MINT } else { MUTED })
        .fill(if selected { MINT } else { Color::TRANSPARENT });
    if selected {
        check = check.child(icon(&CHECK, 18.0).color(Color::BLACK));
    }
    check
}

pub(super) fn selection_row(s: &State, message: &Message, content: El) -> El {
    let selected = s.forwarding.as_ref().is_some_and(|flow| {
        flow.selected
            .iter()
            .any(|m| m.id == message.id && m.sender == message.sender)
    });
    let target = message.clone();
    row()
        .w_full()
        .gap(12.0)
        .center_y()
        .radius(12.0)
        .fill(if selected {
            MINT.alpha(0.05)
        } else {
            Color::TRANSPARENT
        })
        .child(checkbox(s, message))
        .child(content.grow(1.0).min_w(0.0))
        // One selection target covers text, media and empty row space so nested
        // message controls cannot open a player or link while selecting.
        .child(
            el().pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
                .role("checkbox")
                .label(if selected {
                    "Deselect message"
                } else {
                    "Select message"
                })
                .help(super::locale::tr("Select message to forward"))
                .hover(MINT.alpha(0.045))
                .on_press(move |s: &mut State| s.toggle_forward(&target)),
        )
}

pub(super) fn reply_banner(s: &State) -> Option<El> {
    let message = s.reply.as_ref()?;
    Some(
        row()
            .w_full()
            .gap(12.0)
            .padding(11.0)
            .radius(10.0)
            .fill(MINT.alpha(0.08))
            .child(icon(&REPLY, 18.0).color(MINT))
            .child(
                col()
                    .grow(1.0)
                    .min_w(0.0)
                    .gap(4.0)
                    .child(label(
                        if message.from_me {
                            "Replying to you"
                        } else {
                            &message.sender_name
                        },
                        11.0,
                        MINT,
                    ))
                    .child(label(message.preview(), 12.0, MUTED).truncate()),
            )
            .child(icon_button(&glyphs::CLOSE, "Cancel reply", |s, _| {
                s.reply = None
            })),
    )
}

pub(super) fn selection_bar(s: &State) -> Option<El> {
    let flow = s.forwarding.as_ref()?;
    let mut body = col().w_full().padding(18.0).gap(9.0).child(
        row()
            .w_full()
            .center_y()
            .gap(14.0)
            .child(icon_button(
                &glyphs::CLOSE,
                "Exit forwarding · Escape",
                |s, _| {
                    if s.forwarding.as_ref().is_some_and(|f| !f.sending) {
                        s.forwarding = None;
                    }
                },
            ))
            .child(label(format!("{} selected", flow.selected.len()), 13.0, INK).grow(1.0))
            .child(
                icon_button(&FORWARD, "Choose where to forward", |s, _| {
                    if let Some(f) = &mut s.forwarding
                        && !f.selected.is_empty()
                    {
                        f.picker = true;
                        f.destination = None;
                    }
                })
                .opacity(if flow.selected.is_empty() { 0.4 } else { 1.0 }),
            ),
    );
    if let Some(error) = &flow.error {
        body = body.child(label(super::locale::tr(error), 12.0, MUTED).wrap());
    }
    Some(body)
}

pub(super) fn reactions(message: &Message) -> Option<El> {
    if message.reactions.is_empty() {
        return None;
    }
    let mut grouped = std::collections::BTreeMap::new();
    for reaction in &message.reactions {
        let entry = grouped.entry(reaction.emoji.clone()).or_insert((0, false));
        entry.0 += 1;
        entry.1 |= reaction.sender == "self";
    }
    let mut items = row().gap(5.0);
    for (emoji, (count, mine)) in grouped {
        let target = message.clone();
        let reaction = emoji.clone();
        items = items.child(
            label(format!("{emoji} {count}"), 12.0, INK)
                .px(8.0)
                .py(4.0)
                .radius(11.0)
                .fill(if mine {
                    MINT.alpha(0.16)
                } else {
                    Color::WHITE.alpha(0.06)
                })
                .border(1.0, if mine { MINT.alpha(0.3) } else { BORDER })
                .role("button")
                .help(if mine {
                    "Remove your reaction"
                } else {
                    "Add this reaction"
                })
                .on_press(move |s: &mut State| {
                    if s.forwarding.is_none() {
                        s.send_reaction(target.clone(), reaction.clone());
                    }
                }),
        );
    }
    Some(items)
}

pub(super) fn reaction_picker(s: &State) -> Vec<El> {
    let Some(message) = &s.reaction_menu else {
        return vec![];
    };
    let mut choices = row().gap(4.0);
    for emoji in ["👍", "❤️", "😂", "😮", "😢", "🙏"] {
        let target = message.clone();
        choices = choices.child(
            label(emoji, 23.0, INK)
                .size(40.0, 42.0)
                .center()
                .radius(10.0)
                .role("button")
                .label(format!("React {emoji}"))
                .hover(MINT.alpha(0.13))
                .on_press(move |s: &mut State| s.send_reaction(target.clone(), emoji.into())),
        );
    }
    let full = message.clone();
    choices = choices.child(icon_button(
        &super::view::PLUS,
        "More emoji",
        move |s, _| s.open_picker(super::picker::Tab::Emoji, Some(full.clone())),
    ));
    let custom = message.clone();
    let body = col().padding(10.0).gap(9.0).child(choices).child(
        row()
            .gap(6.0)
            .child(
                TextInput::new(&s.reaction_text)
                    .placeholder(super::locale::tr("Paste an emoji…"))
                    .width(None)
                    .on_edit(|s: &mut State| &mut s.reaction_text)
                    .build(kit())
                    .grow(1.0)
                    .min_w(0.0),
            )
            .child(button("React", move |s, _| {
                let emoji = s.reaction_text.text.trim().to_owned();
                if !emoji.is_empty() {
                    s.send_reaction(custom.clone(), emoji);
                }
            })),
    );
    vec![
        el().absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .on_press(|s: &mut State| s.reaction_menu = None),
        rune_kit::popover::anchored(
            kit(),
            rune_kit::popover::raised(kit(), body, 14.0),
            &format!("message-actions-{}-{}", message.sender, message.id),
            rune_kit::Placement::Top,
            8.0,
        ),
    ]
}

fn destination(s: &State, chat: &Chat, compact: bool) -> El {
    let selected = s
        .forwarding
        .as_ref()
        .is_some_and(|f| f.destination.as_ref() == Some(&chat.jid));
    let jid = chat.jid.clone();
    let action = move |s: &mut State| {
        if let Some(flow) = &mut s.forwarding
            && !flow.sending
        {
            flow.destination = Some(jid.clone());
        }
    };
    if compact {
        col()
            .w(68.0)
            .gap(7.0)
            .center()
            .padding(4.0)
            .radius(12.0)
            .fill(if selected {
                MINT.alpha(0.12)
            } else {
                Color::TRANSPARENT
            })
            .role("button")
            .label(&chat.name)
            .help(&chat.name)
            .on_press(action)
            .child(
                el().size(46.0, 46.0)
                    .clip_rounded(23.0)
                    .child(media::avatar(
                        s,
                        &chat.jid,
                        &chat.name,
                        chat.photo_version,
                        46.0,
                    )),
            )
            .child(label(&chat.name, 10.0, INK).w_full().truncate())
    } else {
        row()
            .w_full()
            .gap(12.0)
            .padding(10.0)
            .radius(12.0)
            .center_y()
            .fill(if selected {
                MINT.alpha(0.12)
            } else {
                Color::TRANSPARENT
            })
            .hover(Color::WHITE.alpha(0.04))
            .role("button")
            .label(&chat.name)
            .on_press(action)
            .child(media::avatar(
                s,
                &chat.jid,
                &chat.name,
                chat.photo_version,
                40.0,
            ))
            .child(
                col()
                    .grow(1.0)
                    .min_w(0.0)
                    .gap(4.0)
                    .child(label(&chat.name, 13.0, INK).truncate())
                    .child(label(&chat.phone, 11.0, MUTED)),
            )
            .child(if selected {
                icon(&CHECK, 20.0).color(MINT)
            } else {
                el().size(20.0, 20.0)
            })
    }
}

pub(super) fn forward_picker(s: &State) -> Option<El> {
    let flow = s.forwarding.as_ref().filter(|f| f.picker)?;
    let query = flow.search.text.trim().to_lowercase();
    let eligible =
        |chat: &&Chat| !chat.jid.ends_with("@broadcast") && !chat.jid.ends_with("@newsletter");
    let mut body = col().w_full().gap(18.0).padding(22.0);
    body = body.child(
        TextInput::new(&flow.search)
            .placeholder(super::locale::tr("Search name or phone number"))
            .leading_icon(&glyphs::SEARCH)
            .width(None)
            .on_edit(|s: &mut State| match s.forwarding.as_mut() {
                Some(f) => &mut f.search,
                None => &mut s.reaction_text,
            })
            .build(kit()),
    );
    if query.is_empty() {
        let mut recent = row().gap(8.0);
        for chat in s
            .snapshot
            .chats
            .iter()
            .filter(eligible)
            .filter(|c| !c.archived)
            .take(7)
        {
            recent = recent.child(destination(s, chat, true));
        }
        body = body.child(
            col()
                .gap(10.0)
                .child(label(super::locale::tr("Recent chats"), 11.0, MINT))
                .child(recent),
        );
    }
    let digits: String = query.chars().filter(char::is_ascii_digit).collect();
    let matches: Vec<_> = s
        .snapshot
        .chats
        .iter()
        .filter(eligible)
        .filter(|c| {
            query.is_empty()
                || c.name.to_lowercase().contains(&query)
                || (!digits.is_empty() && c.phone.contains(&digits))
        })
        .take(100)
        .collect();
    let mut chats = col().gap(3.0).w_full();
    for chat in matches {
        chats = chats.child(destination(s, chat, false));
    }
    body = body.child(chats.h(280.0).scroll(ScrollSpec {
        stick_bottom: false,
    }));
    if let Some(error) = &flow.error {
        body = body.child(label(super::locale::tr(error), 12.0, MUTED).wrap());
    }
    let content = col()
        .w(620.0)
        .h_full()
        .max_h(760.0)
        .on_press(|_: &mut State| {})
        .child(
            row()
                .padding(20.0)
                .center_y()
                .child(title(super::locale::tr("Forward messages"), 18.0))
                .child(el().grow(1.0))
                .child(icon_button(
                    &glyphs::CLOSE,
                    "Back to selecting messages",
                    |s, _| {
                        if let Some(f) = &mut s.forwarding
                            && !f.sending
                        {
                            f.picker = false;
                        }
                    },
                )),
        )
        .child(body.grow(1.0).min_h(0.0).scroll(ScrollSpec {
            stick_bottom: false,
        }))
        .child(
            row()
                .padding(20.0)
                .center_y()
                .gap(12.0)
                .child(
                    label(
                        super::locale::format(
                            "{count} message(s) selected",
                            &[("count", flow.selected.len().to_string())],
                        ),
                        12.0,
                        MUTED,
                    )
                    .grow(1.0),
                )
                .child(button(
                    if flow.sending {
                        "Forwarding…"
                    } else {
                        "Forward"
                    },
                    |s, _| s.forward_selected(),
                )),
        );
    Some(
        el().absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .padding(28.0)
            .center()
            .fill(Color::BLACK.alpha(0.72))
            .backdrop_blur(18.0)
            .role("dialog")
            .label(super::locale::tr("Choose a chat to forward to"))
            .child(glass(content)),
    )
}
