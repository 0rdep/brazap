use super::{State, locale};
use rune::{AppHandle, Tray, TrayEvent, TrayIcon, TrayItem, TraySpec, Tx};

#[derive(Default)]
pub(super) struct Background {
    pub tray: Option<Tray>,
    language: String,
}

fn menu() -> Vec<TrayItem> {
    vec![
        TrayItem::action("open", locale::tr("Open Brazap")),
        TrayItem::Separator,
        TrayItem::action("quit", locale::tr("Quit Brazap")),
    ]
}

impl Background {
    pub fn attach(&mut self, app: AppHandle<State>, demo: bool) {
        let Ok(logo) = image::load_from_memory(include_bytes!("../../assets/logo.png")) else {
            return;
        };
        // Use the same square crop as the Windows application icon.
        let pixels = logo
            .crop_imm(318, 64, 894, 894)
            .thumbnail(64, 64)
            .into_rgba8();
        let title = if demo { "Brazap — Demo" } else { "Brazap" };
        let spec = TraySpec::new(
            if demo {
                "Brazap.Demo"
            } else {
                "Brazap.Desktop"
            },
            title,
            TrayIcon::rgba(pixels.width(), pixels.height(), pixels.into_raw()),
        )
        .tooltip(title)
        .menu(menu());
        match Tray::new(spec, move |event| match event {
            TrayEvent::Activate { token: Some(token) } => app.activate(token),
            TrayEvent::Activate { token: None } => app.open(),
            TrayEvent::Menu(id) if id == "open" => app.open(),
            TrayEvent::Menu(id) if id == "quit" => app.quit(),
            _ => {}
        }) {
            Ok(tray) => self.tray = Some(tray),
            Err(rune::TrayError::Unsupported) => {}
            Err(error) => log::warn!("Could not create the system tray icon: {error}"),
        }
    }

    pub fn localize(&mut self, language: &str) {
        if self.language != language {
            self.language = language.into();
            if let Some(tray) = &self.tray {
                tray.set_menu(menu());
            }
        }
    }
}

pub(super) fn close(s: &mut State, _: &mut Tx) {
    s.focused = false;
    s.update_viewing();
    s.composer_focus_at = None;
    s.search_focus_at = None;
    s.close_viewer();
    s.stop_voice();
    if let Some(chat) = &s.selected {
        s.drafts.insert(chat.clone(), s.draft.text.clone());
    }
    if s.snapshot
        .live_call
        .as_ref()
        .is_some_and(|call| !call.ended())
    {
        s.minimize_call();
    }
}
