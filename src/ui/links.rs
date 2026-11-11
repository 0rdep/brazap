use super::{State, view::*};
use crate::{assets::AssetRequest, model::Message};
use rune::{El, Link, Span, SurfaceFit, col, el};

pub(super) fn text(value: &str, size: f32) -> El {
    rich_text(value, size, Vec::new(), Vec::new())
}

const MENTION_SCHEME: &str = "brazap-mention:";

/// Replace `@number` tags with the person's name. Returns the new text and,
/// for every replaced tag, its byte range and the mention it stands for.
pub(super) fn expand_mentions(
    text: &str,
    mentions: &[crate::model::Mention],
) -> (String, Vec<(std::ops::Range<usize>, usize)>) {
    let mut out = String::with_capacity(text.len());
    let mut tags = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find('@') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        // Longest tag first so `@1234` never swallows part of `@12345`.
        let found = mentions
            .iter()
            .enumerate()
            .filter_map(|(index, m)| {
                let token = m.token();
                let tail = rest.get(token.len()..)?;
                (rest.starts_with(&token) && !tail.starts_with(|c: char| c.is_alphanumeric()))
                    .then_some((index, token.len()))
            })
            .max_by_key(|(_, len)| *len);
        let Some((index, len)) = found else {
            out.push('@');
            rest = &rest[1..];
            continue;
        };
        let mention = &mentions[index];
        let shown = if mention.me {
            super::locale::tr("@me")
        } else if mention.name.is_empty() {
            rest[..len].to_owned()
        } else {
            format!("@{}", mention.name.trim_start_matches('@'))
        };
        tags.push((out.len()..out.len() + shown.len(), index));
        out.push_str(&shown);
        rest = &rest[len..];
    }
    out.push_str(rest);
    (out, tags)
}

/// Message body with `@number` tags replaced by the person's name. Tags of
/// other people in groups open their profile; the account's own tag reads
/// "@me" and does nothing.
pub(super) fn message_text(message: &Message, size: f32) -> El {
    let group = message.chat.ends_with("@g.us");
    let (text, tags) = expand_mentions(&message.text, &message.mentions);
    let spans = tags.iter().map(|(range, _)| range.clone()).collect();
    let targets = tags
        .into_iter()
        .filter_map(|(range, index)| {
            let mention = &message.mentions[index];
            (group && !mention.me && !mention.name.is_empty())
                .then(|| (range, format!("{MENTION_SCHEME}{}", mention.jid)))
        })
        .collect();
    let mentions = message.mentions.clone();
    rich_text(&text, size, spans, targets).on_link(move |s: &mut State, url, tx| {
        if let Some(jid) = url.strip_prefix(MENTION_SCHEME) {
            if let Some(mention) = mentions.iter().find(|m| m.jid == jid) {
                s.open_mention(mention);
            }
        } else if let Err(error) = tx.open_url(url) {
            s.error = Some(error.to_string());
        }
    })
}

fn rich_text(
    value: &str,
    size: f32,
    highlights: Vec<std::ops::Range<usize>>,
    mut targets: Vec<(std::ops::Range<usize>, String)>,
) -> El {
    let urls = crate::links::detect(value);
    let mut spans = highlights
        .iter()
        .map(|range| Span::new(range.start, range.end).color(MINT))
        .collect::<Vec<_>>();
    spans.extend(
        urls.iter()
            .map(|(range, _)| Span::new(range.start, range.end).color(MINT)),
    );
    targets.extend(urls);
    let mut body = label(value, size, INK).wrap().selectable(true).spans(spans);
    for (range, url) in targets {
        body = body.link(Link::new(range, url));
    }
    body.on_link(|s: &mut State, url, tx| {
        if let Err(error) = tx.open_url(url) {
            s.error = Some(error.to_string());
        }
    })
}

pub(super) fn preview(s: &State, message: &Message) -> Option<El> {
    let preview = message.link_preview.as_ref()?;
    let url = preview.url.clone();
    let host = url
        .split("://")
        .nth(1)
        .unwrap_or(&url)
        .split('/')
        .next()
        .unwrap_or(&url)
        .to_owned();
    let key = AssetRequest::LinkPreview {
        chat: message.chat.clone(),
        sender: message.sender.clone(),
        id: message.id.clone(),
    }
    .key();
    let image = s.images.get(&key).filter(|image| image.error().is_none());
    // A card with only the site name tells the reader nothing; the link above already shows it.
    if image.is_none() && preview.title.trim().is_empty() && preview.description.trim().is_empty() {
        return None;
    }
    let mut card = col()
        .w_full()
        .max_w(320.0)
        .gap(6.0)
        .padding(10.0)
        .radius(10.0)
        .fill(MINT.alpha(0.065))
        .border(1.0, MINT.alpha(0.12))
        .role("link")
        .label(&preview.url)
        .help(&preview.url)
        .on_press_tx(move |s: &mut State, tx| {
            if let Err(error) = tx.open_url(&url) {
                s.error = Some(error.to_string());
            }
        });
    if let Some(image) = image {
        card = card.child(
            el().w_full()
                .h(140.0)
                .clip_rounded(7.0)
                .pixels_fit(image.pixels(), SurfaceFit::Fill),
        );
    }
    if !preview.title.is_empty() {
        card = card.child(title(&preview.title, 13.0).wrap());
    }
    if !preview.description.is_empty() {
        card = card.child(
            label(
                preview.description.chars().take(220).collect::<String>(),
                11.0,
                MUTED,
            )
            .wrap(),
        );
    }
    Some(card.child(label(host, 11.0, MINT).truncate()))
}
