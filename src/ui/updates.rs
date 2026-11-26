use super::{Command, State, locale, view::*};
use rune::{El, Icon, LoopSpec, el, icon};

const UPDATE: Icon = Icon {
    name: "install-update",
    view: 24.0,
    body: r#"<path d="M12 16V4m-4 4 4-4 4 4M5 15v5h14v-5" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"/>"#,
};

/// The update action in the sidebar header, beside Settings.
pub(super) fn prompt(s: &State) -> Option<El> {
    let update = &s.snapshot.updates;
    if !update.newer_than(env!("CARGO_PKG_VERSION")) {
        return None;
    }
    let ready = update.ready_for(env!("CARGO_PKG_VERSION"));
    // Package-managed and read-only installs never download; they link to the release.
    let manual = update.manual && !ready;
    let downloading = !manual && (update.status == "downloading" || update.status == "checking");
    let action = if ready {
        "Restart and update"
    } else if manual {
        "Download update"
    } else if downloading {
        "Downloading update…"
    } else {
        "Retry update"
    };
    let mut glyph = icon(&UPDATE, 18.0).color(MINT);
    if downloading && !ready {
        glyph = glyph.looping(LoopSpec::rotation(1.6));
    }
    // A header button beside Settings, tinted as the one action that needs attention;
    // the tooltip has the details.
    let mut button = el()
        .key("sidebar-update")
        .size(34.0, 34.0)
        .center()
        .radius(11.0)
        .fill(MINT.alpha(0.12))
        .border(1.0, MINT.alpha(0.32))
        .hover(MINT.alpha(0.2))
        .motion(rune::tok::motion::SNAPPY)
        .cursor(rune::Cursor::Pointer)
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
        } else if manual {
            locale::format(
                "Brazap {version} is available. Download it from the release page.",
                &[("version", update.latest.clone().unwrap_or_default())],
            )
        } else {
            locale::tr(action)
        })
        .child(glyph);
    if ready {
        button = button.on_press_tx(
            |s: &mut State, tx| match crate::updates::install(&s.config) {
                Ok(()) => tx.quit(),
                Err(error) => {
                    log::warn!("Could not install update: {error:#}");
                    s.error = Some("Could not install the update. Try again.".into());
                }
            },
        );
    } else if manual {
        let url = update
            .release_url
            .clone()
            .unwrap_or_else(|| "https://github.com/0rdep/brazap/releases/latest".into());
        button = button.on_press_tx(move |s: &mut State, tx| {
            if let Err(error) = tx.open_url(&url) {
                s.error = Some(error.to_string());
            }
        });
    } else if !downloading {
        button = button.on_press(|s: &mut State| {
            s.snapshot.updates.status = "downloading".into();
            let _ = s.tx.send(Command::Update);
        });
    }
    Some(button)
}
