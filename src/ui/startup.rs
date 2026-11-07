use super::{State, locale, view::*};
use crate::model::Phase;
use rune::{Color, El, LoopSpec, col, el};
use serde_json::json;

pub(super) fn loading(s: &State) -> bool {
    !s.authenticated && (s.snapshot.connection.initial_sync || (!s.phone_mode && s.qr.is_none()))
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
    let message = if connection.syncing {
        connection.sync_progress.filter(|p| *p < 100).map_or_else(
            || locale::tr("Syncing messages…"),
            |progress| {
                locale::format(
                    "Syncing messages… {progress}%",
                    &[("progress", progress.to_string())],
                )
            },
        )
    } else if connection.initial_sync || connection.has_session {
        locale::tr("Connecting to WhatsApp…")
    } else {
        locale::tr("Starting Brazap…")
    };
    let mut sky = el().size(600.0, 460.0);
    // Native presentation tracks share Rune's clock and stop under reduced
    // motion. They only exist during startup; the chat view has no idle loop.
    // Small independent flight paths keep layer allocations bounded. The
    // transparent artwork is decoded only during loading, at display scale.
    for (i, (art, x, y, size, period)) in [
        (0, 28.0, 30.0, 104.0, 14.0),
        (1, 276.0, -30.0, 72.0, 19.0),
        (2, 500.0, 60.0, 92.0, 23.0),
        (2, 10.0, 332.0, 76.0, 17.0),
        (0, 286.0, 408.0, 68.0, 21.0),
        (1, 500.0, 340.0, 92.0, 18.0),
    ]
    .into_iter()
    .enumerate()
    {
        let Some(image) = s.startup_art.get(art) else {
            continue;
        };
        let picture = el()
            .size(size, size)
            .pixels_fit(image.pixels(), rune::SurfaceFit::Fit);
        let flight = el()
            .key(format!("loading-doodle-{i}"))
            .absolute(x, y)
            .size(size, size);
        sky = sky.child(if reduced_motion {
            flight.child(picture.opacity(0.3))
        } else {
            let delay = i as f64 * 0.7;
            let (from, to) = if i % 2 == 0 {
                ([-0.45, 0.3], [0.45, -0.3])
            } else {
                ([0.35, -0.3], [-0.35, 0.3])
            };
            flight
                .looping(LoopSpec {
                    delay,
                    ..LoopSpec::slide(period, from, to)
                })
                // Both tracks share their epoch and period: each flight fades
                // completely before it wraps to the beginning of its path.
                .child(picture.looping(LoopSpec::opacity(period, delay, 0.0, 0.44)))
        });
    }
    let mut brand = col()
        .w_full()
        .h_full()
        .center()
        .gap(16.0)
        .child(logo(s, 112.0))
        .child(title("brazap", 30.0))
        .child(label(message, 12.0, MUTED));
    if let Some(notice) = connection.notice.as_deref() {
        brand = brand.child(
            label(locale::tr(notice), 12.0, INK)
                .max_w(380.0)
                .wrap()
                .center(),
        );
    }
    if connection.phase == Phase::Error {
        brand = brand.child(button("Try again", |s, _| s.tool("reconnect", json!({}))).w(180.0));
    } else if !connection.has_session && !connection.initial_sync {
        brand = brand.child(
            el().h(36.0)
                .px(14.0)
                .center()
                .role("button")
                .label(locale::tr("Link with phone number"))
                .hover(Color::WHITE.alpha(0.035))
                .radius(10.0)
                .on_press(|s: &mut State| s.phone_mode = true)
                .child(label(locale::tr("Link with phone number"), 12.0, MUTED)),
        );
    }
    el().w_full().h_full().center().child(sky.child(brand))
}
