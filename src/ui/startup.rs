use super::{State, locale, view::*};
use crate::model::Phase;
use rune::{Align, Easing, El, LoopSpec, Side, col, el};
use serde_json::json;

// A complete out-and-back curve: identical end positions make the loop seamless.
const FLOAT: Easing = Easing::Piecewise(&[
    [0.0, 0.0],
    [0.0625, 0.0381],
    [0.125, 0.1464],
    [0.1875, 0.3087],
    [0.25, 0.5],
    [0.3125, 0.6913],
    [0.375, 0.8536],
    [0.4375, 0.9619],
    [0.5, 1.0],
    [0.5625, 0.9619],
    [0.625, 0.8536],
    [0.6875, 0.6913],
    [0.75, 0.5],
    [0.8125, 0.3087],
    [0.875, 0.1464],
    [0.9375, 0.0381],
    [1.0, 0.0],
]);

pub(super) fn loading(s: &State) -> bool {
    !s.authenticated
        && ((s.startup_error.is_some() && s.qr.is_none())
            || s.snapshot.connection.initial_sync
            || (!s.phone_mode && s.qr.is_none()))
}

pub(super) fn sync_art(s: &mut State) -> bool {
    if !loading(s) {
        let changed = !s.startup_art.is_empty();
        s.startup_art.clear();
        return changed;
    }
    if !s.startup_art.is_empty() {
        return false;
    }
    let Some(cache) = &s.image_cache else {
        return false;
    };
    for bytes in [
        include_bytes!("../../assets/startup/paper-plane.png").as_slice(),
        include_bytes!("../../assets/startup/envelope.png").as_slice(),
        include_bytes!("../../assets/startup/chat-bubble.png").as_slice(),
        include_bytes!("../../assets/startup/phone.png").as_slice(),
        include_bytes!("../../assets/startup/voice-note.png").as_slice(),
        include_bytes!("../../assets/startup/photo.png").as_slice(),
    ] {
        if let Ok(image) = image::load_from_memory(bytes) {
            let pixels = image.thumbnail(192, 192).into_rgba8();
            s.startup_art
                .push(cache.rgba(pixels.width(), pixels.height(), pixels.into_raw()));
        }
    }
    !s.startup_art.is_empty()
}

pub(super) fn view(s: &State, reduced_motion: bool) -> El {
    let connection = &s.snapshot.connection;
    let mut sky = el().key("startup-scene").size(600.0, 460.0);
    // Native presentation tracks share Rune's clock and stop under reduced
    // motion. They only exist during startup; the chat view has no idle loop.
    // Small independent flight paths keep layer allocations bounded. The
    // transparent artwork is decoded only during loading, at display scale.
    for (i, (art, x, y, size, period)) in [
        (0, 28.0, 30.0, 104.0, 5.6),
        (1, 276.0, -30.0, 72.0, 7.2),
        (2, 500.0, 60.0, 92.0, 8.4),
        (3, 10.0, 332.0, 76.0, 6.0),
        (4, 286.0, 408.0, 68.0, 7.8),
        (5, 500.0, 340.0, 92.0, 6.6),
    ]
    .into_iter()
    .enumerate()
    {
        let Some(image) = s.startup_art.get(art) else {
            continue;
        };
        let picture = el()
            .size(size, size)
            .opacity(0.38)
            .pixels_fit(image.pixels(), rune::SurfaceFit::Fit);
        let flight = el()
            .key(format!("loading-doodle-{i}"))
            .absolute(x, y)
            .size(size, size);
        sky = sky.child(if reduced_motion {
            flight.child(picture)
        } else {
            let (from, to) = if i % 2 == 0 {
                ([-0.45, 0.3], [0.45, -0.3])
            } else {
                ([0.35, -0.3], [-0.35, 0.3])
            };
            flight
                .looping(LoopSpec::slide(period, from, to).easing(FLOAT))
                .child(picture)
        });
    }
    let brand = col()
        .key("startup-brand")
        .center()
        .gap(16.0)
        .child(
            logo(s, 112.0)
                .key("startup-logo")
                .matched(BRAND_HERO)
                .motion(rune::tok::motion::REFLOW),
        )
        // The wordmark is branding, like the logo: loading the user's text-size
        // preference must not resize it just before the QR screen replaces it.
        .child(
            rune::text("brazap")
                .key("startup-wordmark")
                .font(rune::TextStyle::new(30.0, 620.0).family(rune::Family::Named("Inter")))
                .color(INK),
        );
    sky = sky.child(el().w_full().h_full().center().child(brand));
    // Keep actionable failures above the stationary brand; routine progress
    // copy is omitted and can never move the logo when connection state changes.
    let error = s.startup_error.as_deref().or_else(|| {
        (connection.phase == Phase::Error)
            .then_some(connection.notice.as_deref())
            .flatten()
    });
    if let Some(error) = error {
        sky = sky.child(
            col()
                .key("startup-error")
                .w(380.0)
                .gap(12.0)
                .center_y()
                .anchored("startup-brand", Side::Top, Align::Center, 24.0)
                .child(
                    label(locale::tr(error), 12.0, INK)
                        .w_full()
                        .align(Align::Center)
                        .wrap(),
                )
                .child(
                    button("Try again", |s, _| {
                        if s.startup_error.take().is_some() {
                            let _ = s.tx.send(super::Command::Refresh);
                        } else {
                            s.tool("reconnect", json!({}));
                        }
                    })
                    .w(180.0),
                ),
        );
    }
    el().key("loading-screen")
        .w_full()
        .h_full()
        .center()
        .child(sky)
}
