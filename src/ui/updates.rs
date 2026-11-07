use super::{Command, State, locale, view::*};
use rune::{Color, El, Icon, LoopSpec, icon, row};

const UPDATE: Icon = Icon {
    name: "install-update",
    view: 24.0,
    body: r#"<path d="M12 16V4m-4 4 4-4 4 4M5 15v5h14v-5" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"/>"#,
};

pub(super) fn prompt(s: &State) -> Option<El> {
    let update = &s.snapshot.updates;
    if !update.newer_than(env!("CARGO_PKG_VERSION")) {
        return None;
    }
    let ready = update.ready_for(env!("CARGO_PKG_VERSION"));
    let downloading = update.status == "downloading" || update.status == "checking";
    let action = if ready {
        "Restart and update"
    } else if downloading {
        "Downloading update…"
    } else {
        "Retry update"
    };
    let small = !s.sidebar.is_open();
    let mut glyph = icon(&UPDATE, if small { 19.0 } else { 18.0 }).color(MINT);
    if downloading && !ready {
        glyph = glyph.looping(LoopSpec::rotation(1.6));
    }
    let mut button = row()
        .key("sidebar-update")
        .w_full()
        .h(locale::scale(42.0))
        .px(if small { 0.0 } else { 10.0 })
        .gap(10.0)
        .center_y()
        .radius(11.0)
        .fill(MINT.alpha(0.1))
        .border(1.0, MINT.alpha(0.25))
        .role(if downloading && !ready {
            "status"
        } else {
            "button"
        })
        .label(locale::tr(action))
        .help(if ready {
            locale::format(
                "Restart to install Brazap {version}.",
                &[("version", update.latest.clone().unwrap_or_default())],
            )
        } else {
            locale::tr(action)
        })
        .child(glyph);
    if small {
        button = button
            .size(36.0, 36.0)
            .px(0.0)
            .center()
            .fill(Color::WHITE.alpha(0.035))
            .border(1.0, CONTROL_BORDER)
            .press(Color::WHITE.alpha(0.09))
            .motion(rune::tok::motion::SNAPPY);
    } else {
        button = button.child(
            label(locale::tr(action), 12.0, MINT)
                .grow(1.0)
                .min_w(0.0)
                .truncate(),
        );
    }
    let hover = if small {
        Color::WHITE.alpha(0.05)
    } else {
        MINT.alpha(0.08)
    };
    if ready {
        button =
            button.hover(hover).on_press_tx(|s: &mut State, tx| {
                match crate::updates::install(&s.config) {
                    Ok(()) => tx.quit(),
                    Err(error) => {
                        log::warn!("Could not install update: {error:#}");
                        s.error = Some("Could not install the update. Try again.".into());
                    }
                }
            });
    } else if !downloading {
        button = button.hover(hover).on_press(|s: &mut State| {
            s.snapshot.updates.status = "downloading".into();
            let _ = s.tx.send(Command::Update);
        });
    }
    Some(button)
}
