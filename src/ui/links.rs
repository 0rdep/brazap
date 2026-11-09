use super::{State, view::*};
use crate::{assets::AssetRequest, model::Message};
use rune::{El, Link, Span, SurfaceFit, col, el};

pub(super) fn text(value: &str, size: f32) -> El {
    let ranges = crate::links::detect(value);
    let mut body = label(value, size, INK).wrap().selectable(true).spans(
        ranges
            .iter()
            .map(|(range, _)| Span::new(range.start, range.end).color(MINT))
            .collect::<Vec<_>>(),
    );
    for (range, url) in ranges {
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
    if let Some(image) = s.images.get(&key) {
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
