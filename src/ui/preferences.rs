use super::{Command, State, view::*};
use crate::settings::Settings;
use rune::{Color, El, Icon, ScrollSpec, col, el, icon, row};
use rune_kit::{Switch, glyphs};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const MINUS: Icon = Icon {
    name: "minus",
    view: 24.0,
    body: r#"<path d="M5 12h14" stroke="currentColor" stroke-width="1.8"/>"#,
};
pub(super) const SETTINGS: Icon = Icon {
    name: "settings-gear",
    view: 24.0,
    // Eight even teeth around the centre.
    body: r#"<path d="M10.15 4.84 10.42 2.53A9.6 9.6 0 0 1 13.58 2.53L13.85 4.84A7.4 7.4 0 0 1 15.76 5.62L17.57 4.18A9.6 9.6 0 0 1 19.82 6.43L18.38 8.24A7.4 7.4 0 0 1 19.16 10.15L21.47 10.42A9.6 9.6 0 0 1 21.47 13.58L19.16 13.85A7.4 7.4 0 0 1 18.38 15.76L19.82 17.57A9.6 9.6 0 0 1 17.57 19.82L15.76 18.38A7.4 7.4 0 0 1 13.85 19.16L13.58 21.47A9.6 9.6 0 0 1 10.42 21.47L10.15 19.16A7.4 7.4 0 0 1 8.24 18.38L6.43 19.82A9.6 9.6 0 0 1 4.18 17.57L5.62 15.76A7.4 7.4 0 0 1 4.84 13.85L2.53 13.58A9.6 9.6 0 0 1 2.53 10.42L4.84 10.15A7.4 7.4 0 0 1 5.62 8.24L4.18 6.43A9.6 9.6 0 0 1 6.43 4.18L8.24 5.62A7.4 7.4 0 0 1 10.15 4.84Z" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linejoin="round"/><circle cx="12" cy="12" r="3" fill="none" stroke="currentColor" stroke-width="1.6"/>"#,
};
pub(super) const LOGOUT: Icon = Icon {
    name: "logout",
    view: 24.0,
    body: r#"<path d="M9 4H4v16h5m5-13 5 5-5 5m-6-5h11" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"/>"#,
};

#[derive(Clone, Copy, Default, PartialEq)]
pub(super) enum Page {
    #[default]
    Appearance,
    Notifications,
    Privacy,
    Transcription,
    Storage,
    Mcp,
    Debug,
    About,
}
impl Page {
    fn name(self) -> &'static str {
        match self {
            Self::Appearance => "Appearance",
            Self::Notifications => "Notifications",
            Self::Privacy => "Privacy",
            Self::Transcription => "Transcription",
            Self::Storage => "Storage",
            Self::Mcp => "MCP",
            Self::Debug => "Debug",
            Self::About => "About",
        }
    }
}
#[derive(Default)]
pub(super) struct Panel {
    pub open: bool,
    pub page: Page,
    pub value: Settings,
    pub loading: bool,
    pub saving: bool,
    save_value: Option<Settings>,
    pub settings_revision: u64,
    pub report: Value,
    pub privacy: BTreeMap<String, String>,
    pub privacy_pending: bool,
    pub error: Option<String>,
    pub logout: bool,
    pub logging_out: bool,
    pub confirm_cache: bool,
    /// The speech model whose Delete was pressed once and awaits confirmation.
    pub speech_delete: Option<&'static str>,
    /// `BRAZAP_MCP` override reported by the running service.
    pub mcp_override: Option<bool>,
}
impl State {
    pub(super) fn open_preferences(&mut self) {
        self.preferences.open = true;
        self.preferences.speech_delete = None;
        if !self.preferences.saving {
            self.preferences.value = self.snapshot.settings.clone();
        }
        self.preferences.loading = true;
        self.preferences.error = None;
        self.close_viewer();
        self.details = false;
        self.contact = None;
        self.stop_voice();
        self.request_preference("get", json!({}));
    }
    fn request_preference(&self, action: &'static str, args: Value) {
        let _ = self.tx.send(Command::Preferences(action, args));
    }

    fn begin_logout(&mut self) {
        if self.preferences.logging_out {
            return;
        }
        self.cancel_recording();
        self.cancel_attachments();
        self.stop_voice();
        self.close_viewer();
        self.preferences.logging_out = true;
        self.preferences.logout = false;
        self.preferences.open = false;
        self.preferences.confirm_cache = false;
        self.preferences.error = None;
        self.authenticated = false;
        self.selected = None;
        self.timeline.clear();
        self.loading_chat = false;
        self.history_generation = self.history_generation.wrapping_add(1);
        self.history_focus = false;
        self.loading_older = false;
        self.loading_newer = false;
        self.reply = None;
        self.details = false;
        self.contact = None;
        self.diagnostics.open = false;
        self.picker.open = false;
        self.chat_search.clear();
        self.chat_search.open = false;
        self.qr = None;
        self.qr_payload = None;
        self.phone_mode = false;
        self.startup_error = None;
        self.error = None;
        self.snapshot = crate::model::Snapshot {
            settings: self.preferences.value.clone(),
            ..Default::default()
        };
        // request_asset skips avatars whose version is recorded as loaded, so
        // the versions go with the images or a new pairing never loads them.
        self.avatars.clear();
        self.avatar_versions.clear();
        self.images.clear();
        self.image_order.clear();
        self.image_failures.clear();
        self.animations.clear();
        self.update_viewing();
        self.request_preference("logout", json!({}));
    }
    pub(super) fn edit_preferences(&mut self, change: impl FnOnce(&mut Settings)) {
        let previous = self.preferences.value.clone();
        change(&mut self.preferences.value);
        if previous == self.preferences.value {
            return;
        }
        self.preferences.error = None;
        self.sync_floating();
        if !self.preferences.saving {
            self.save_preferences();
        }
    }
    fn save_preferences(&mut self) {
        self.preferences.saving = true;
        self.preferences.save_value = Some(self.preferences.value.clone());
        self.request_preference("save", json!(self.preferences.value));
    }
    fn preferences_page(&mut self, page: Page) {
        self.preferences.page = page;
        self.preferences.speech_delete = None;
        self.preferences.error = None;
        if page == Page::Privacy {
            self.preferences.privacy_pending = true;
            self.request_preference("privacy", json!({}));
        }
        if matches!(page, Page::Debug | Page::Storage) {
            self.request_preference("get", json!({}));
        }
    }
    pub(super) fn receive_preferences(&mut self, action: &str, result: Result<Value, String>) {
        if action == "logout" {
            self.preferences.logging_out = false;
            if let Err(error) = result {
                self.error = Some(error);
            }
            // The background worker has cleared its selected chat as well.
            // Keep any QR that arrived while logout was finishing.
            let _ = self.tx.send(Command::Refresh);
            return;
        }
        if action == "save" {
            self.preferences.saving = false;
            let Some(saved) = self.preferences.save_value.take() else {
                return;
            };
            let newer = self.preferences.value != saved;
            match result {
                Ok(value) => {
                    self.preferences.settings_revision = self
                        .preferences
                        .settings_revision
                        .max(value["revision"].as_u64().unwrap_or(0));
                    self.snapshot.settings = saved;
                    if !newer {
                        self.notice = Some(("Settings saved".into(), None));
                    }
                }
                Err(error) => {
                    self.preferences.error = Some(error);
                    if !newer {
                        self.preferences.value = self.snapshot.settings.clone();
                    }
                }
            }
            if newer {
                self.save_preferences();
            }
            self.sync_floating();
            return;
        }
        if action == "speech_model" {
            match result {
                // The next snapshot shows the download or the new selection.
                Ok(_) => {
                    let _ = self.tx.send(Command::Refresh);
                }
                Err(error) => self.preferences.error = Some(error),
            }
            return;
        }
        match action {
            "get" => self.preferences.loading = false,
            "privacy" | "privacy_set" => self.preferences.privacy_pending = false,
            _ => {}
        }
        match result {
            Err(e) => {
                self.preferences.error = Some(e);
            }
            Ok(value) => match action {
                "get" => {
                    self.preferences.report = value["storage"].clone();
                    self.preferences.mcp_override = value["mcp"]["override"].as_bool();
                    if !self.preferences.saving
                        && value["revision"].as_u64().unwrap_or(0)
                            >= self.preferences.settings_revision
                    {
                        if let Ok(settings) = serde_json::from_value(value["settings"].clone()) {
                            self.preferences.value = settings;
                        }
                    }
                }
                "privacy" | "privacy_set" => {
                    if let Ok(p) = serde_json::from_value(value) {
                        self.preferences.privacy = p;
                    }
                    if action == "privacy_set" {
                        self.notice = Some(("Settings saved".into(), None));
                    }
                }
                "clear" => {
                    self.images.clear();
                    self.animations.clear();
                    self.avatars.clear();
                    self.avatar_versions.clear();
                    self.image_order.clear();
                    self.image_failures.clear();
                    self.preferences.confirm_cache = false;
                    self.notice = Some((
                        "Cache cleared. Saved messages and original attachments were kept.".into(),
                        None,
                    ));
                    self.request_preference("get", json!({}));
                }
                _ => {}
            },
        }
    }
}

/// The selected model's loading state or error, under the model picker.
fn transcription_status(s: &State) -> Option<String> {
    if !s.preferences.value.transcription.enabled {
        return None;
    }
    let engine = &s.snapshot.transcription;
    let selected =
        crate::transcription::SpeechModel::find(&s.preferences.value.transcription.model);
    // Settings saved a moment ago reach the snapshot on the next poll.
    if engine.status != "unsupported" && engine.model != selected.id {
        return None;
    }
    let model = || ("model", selected.title.to_owned());
    Some(match engine.status.as_str() {
        "loading" => super::locale::format("Loading {model}…", &[model()]),
        "error" | "unsupported" => super::locale::tr(
            engine
                .error
                .as_deref()
                .unwrap_or("Voice transcription is unavailable."),
        ),
        "idle" if !engine.downloaded && engine.progress.is_none() => super::locale::format(
            "{model} downloads the first time a voice message is transcribed.",
            &[model()],
        ),
        _ => return None,
    })
}

fn section(name: &str, description: &str) -> El {
    col()
        .w_full()
        .gap(8.0)
        .child(title(super::locale::tr(name), 22.0))
        .child(label(super::locale::tr(description), 12.0, MUTED).wrap())
}
fn card(body: El) -> El {
    body.w_full()
        .padding(18.0)
        .radius(14.0)
        .fill(Color::WHITE.alpha(0.025))
        .border(1.0, BORDER)
}
pub(super) fn toggle(
    _s: &State,
    name: &str,
    description: &str,
    on: bool,
    change: impl Fn(&mut Settings) + 'static,
) -> El {
    card(
        row()
            .gap(18.0)
            .center_y()
            .child(
                col()
                    .grow(1.0)
                    .min_w(0.0)
                    .gap(5.0)
                    .child(title(super::locale::tr(name), 13.0))
                    .child(label(super::locale::tr(description), 11.0, MUTED).wrap()),
            )
            .child(
                Switch::new(on)
                    .label(super::locale::tr(name))
                    .hide_label()
                    .on_toggle(move |s: &mut State| s.edit_preferences(&change))
                    .build(kit()),
            ),
    )
}
fn choice(name: &str, selected: bool, action: impl Fn(&mut State) + 'static) -> El {
    label(
        super::locale::tr(name),
        12.0,
        if selected { MINT } else { INK },
    )
    .px(12.0)
    .py(9.0)
    .radius(9.0)
    .fill(if selected {
        MINT.alpha(0.12)
    } else {
        Color::WHITE.alpha(0.03)
    })
    .border(1.0, if selected { MINT.alpha(0.3) } else { BORDER })
    .role("button")
    .label(super::locale::tr(name))
    .hover(Color::WHITE.alpha(0.06))
    .on_press(action)
}

const TRASH: Icon = Icon {
    name: "speech-model-delete",
    view: 24.0,
    body: r#"<path d="M4 7h16M10 11v6m4-6v6M6 7l1 13h10l1-13M9 7V4h6v3" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"/>"#,
};
const DOWNLOAD: Icon = Icon {
    name: "speech-model-download",
    view: 24.0,
    body: r#"<path d="M12 4v11m-5-5 5 5 5-5M5 20h14" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/>"#,
};
const DANGER: Color = Color {
    r: 0.95,
    g: 0.33,
    b: 0.33,
    a: 1.0,
};

/// A download size the way people read it: `85 MB`.
fn model_size(bytes: u64) -> String {
    format!("{} MB", (bytes as f64 / 1_000_000.0).round())
}

fn speech_action(s: &mut State, id: &'static str, action: &'static str) {
    s.preferences.speech_delete = None;
    s.preferences.error = None;
    s.request_preference(
        "speech_model",
        json!({"name": "speech_model", "arguments": {"id": id, "action": action}}),
    );
}

/// A small text button in a model row.
fn model_button(name: String, color: Color, action: impl Fn(&mut State) + 'static) -> El {
    row()
        .h(28.0)
        .px(11.0)
        .gap(5.0)
        .center()
        .radius(8.0)
        .fill(Color::WHITE.alpha(0.04))
        .border(1.0, CONTROL_BORDER)
        .role("button")
        .label(name.clone())
        .hover(Color::WHITE.alpha(0.08))
        .press(Color::WHITE.alpha(0.12))
        .motion(rune::tok::motion::SNAPPY)
        .on_press(action)
        .child(label(name, 11.0, color).font(super::locale::style(11.0, 600.0)))
}

/// One catalog model: its name and role, size and languages, what it is
/// good at, and its state on the right: download, progress with cancel,
/// use and delete, or in use. Mirrors Hex's model picker list
/// (`rune_ui/picker/list.rs`), compacted for a settings card.
fn model_row(s: &State, model: &'static crate::transcription::SpeechModel) -> El {
    let engine = &s.snapshot.transcription;
    let state = engine.models.iter().find(|m| m.id == model.id);
    let downloaded = state.is_some_and(|m| m.downloaded);
    let progress = state.and_then(|m| m.progress);
    let selected = s.preferences.value.transcription.model == model.id;
    let size = model_size(model.bytes);
    let languages = if model.id == "parakeet-v3" {
        super::locale::format(
            "{count} European languages",
            &[("count", model.languages.to_string())],
        )
    } else {
        super::locale::format(
            "{count} languages",
            &[("count", model.languages.to_string())],
        )
    };
    let mut copy = col()
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .gap(3.0)
        .child(
            row()
                .gap(8.0)
                .center_y()
                .child(title(model.title, 13.0))
                .child(label(
                    super::locale::tr(model.role),
                    11.0,
                    if selected { MINT } else { MUTED },
                )),
        )
        .child(label(format!("{size} · {languages}"), 11.0, MUTED))
        .child(label(super::locale::tr(model.detail), 11.0, INK.alpha(0.7)).wrap());
    if let Some(credit) = model.attribution {
        copy = copy.child(label(super::locale::tr(credit), 10.0, MUTED).wrap());
    }
    if let Some(error) = state.and_then(|m| m.error.as_deref()) {
        copy = copy.child(label(super::locale::tr(error), 11.0, DANGER).wrap());
    }
    let id = model.id;
    let mut actions = row().gap(8.0).center_y();
    if let Some(percent) = progress {
        let fraction = percent.min(100) as f32 / 100.0;
        actions = actions
            .child(
                col()
                    .w(120.0)
                    .gap(5.0)
                    .child(
                        row()
                            .gap(4.0)
                            .child(label(format!("{percent}%"), 11.0, INK))
                            .child(label(
                                super::locale::format("of {size}", &[("size", size.clone())]),
                                11.0,
                                MUTED,
                            )),
                    )
                    .child(
                        el().w_full()
                            .h(5.0)
                            .radius(999.0)
                            .fill(Color::WHITE.alpha(0.08))
                            .child(
                                el().w_frac(fraction)
                                    .h_full()
                                    .radius(999.0)
                                    .fill(MINT)
                                    .motion(rune::tok::motion::SNAPPY),
                            ),
                    ),
            )
            .child(
                el().size(26.0, 26.0)
                    .center()
                    .radius(8.0)
                    .role("button")
                    .label(super::locale::tr("Cancel download"))
                    .help(super::locale::tr("Cancel download"))
                    .hover(Color::WHITE.alpha(0.08))
                    .on_press(move |s: &mut State| speech_action(s, id, "cancel"))
                    .child(icon(&glyphs::CLOSE, 14.0).color(MUTED)),
            );
    } else if selected {
        // The selected model downloads with the first voice message, so it never
        // needs a download button of its own.
        let mut status = row()
            .gap(7.0)
            .center_y()
            .child(
                el().size(7.0, 7.0)
                    .radius(4.0)
                    .fill(if downloaded { MINT } else { MUTED }),
            )
            .child(label(super::locale::tr("In use"), 12.0, INK));
        if !downloaded {
            status = status.help(super::locale::tr(
                "Downloads with the first voice message to transcribe",
            ));
        }
        actions = actions.child(status);
    } else if downloaded {
        if s.preferences.speech_delete == Some(id) {
            actions = actions.child(model_button(
                super::locale::format("Delete {size}?", &[("size", size.clone())]),
                DANGER,
                move |s: &mut State| speech_action(s, id, "delete"),
            ));
        } else {
            actions = actions.child(
                el().size(28.0, 28.0)
                    .center()
                    .radius(8.0)
                    .role("button")
                    .label(super::locale::tr("Delete"))
                    .help(super::locale::tr("Delete this model from this computer"))
                    .hover(Color::WHITE.alpha(0.08))
                    .on_press(move |s: &mut State| s.preferences.speech_delete = Some(id))
                    .child(icon(&TRASH, 15.0).color(MUTED)),
            );
        }
        actions = actions.child(model_button(
            super::locale::tr("Use"),
            MINT,
            move |s: &mut State| speech_action(s, id, "choose"),
        ));
    } else {
        let name = super::locale::format("Download {size}", &[("size", size.clone())]);
        actions = actions.child(
            row()
                .h(28.0)
                .px(11.0)
                .gap(5.0)
                .center()
                .radius(8.0)
                .fill(Color::WHITE.alpha(0.04))
                .border(1.0, CONTROL_BORDER)
                .role("button")
                .label(name.clone())
                .hover(Color::WHITE.alpha(0.08))
                .press(Color::WHITE.alpha(0.12))
                .motion(rune::tok::motion::SNAPPY)
                .on_press(move |s: &mut State| speech_action(s, id, "choose"))
                .child(icon(&DOWNLOAD, 13.0).color(MINT))
                .child(label(name, 11.0, MINT).font(super::locale::style(11.0, 600.0))),
        );
    }
    row()
        .key(format!("speech-model-{id}"))
        .w_full()
        .py(10.0)
        .gap(14.0)
        .center_y()
        .child(copy)
        .child(actions)
}

fn retention_label(days: u32) -> &'static str {
    match days {
        0 => "Never delete",
        1 => "1 day",
        3 => "3 days",
        7 => "1 week",
        14 => "2 weeks",
        30 => "1 month",
        90 => "3 months",
        _ => "Custom",
    }
}
fn set_retention(settings: &mut Settings, kind: &str, days: u32) {
    let retention = &mut settings.storage.retention;
    match kind {
        "Photos" => retention.photos = days,
        "Videos" => retention.videos = days,
        "Stickers" => retention.stickers = days,
        "Audio" => retention.audio = days,
        "Files" => retention.files = days,
        _ => {}
    }
}

pub(super) fn view(s: &State) -> El {
    // The pages scroll on a short window; Log out stays pinned under them.
    let mut pages = col().w_full().gap(8.0);
    for page in [
        Page::Appearance,
        Page::Notifications,
        Page::Privacy,
        Page::Transcription,
        Page::Storage,
        Page::Mcp,
        Page::Debug,
        Page::About,
    ] {
        pages = pages.child(
            label(
                super::locale::tr(page.name()),
                12.0,
                if page == s.preferences.page {
                    MINT
                } else {
                    MUTED
                },
            )
            .px(12.0)
            .py(12.0)
            .radius(10.0)
            .fill(if page == s.preferences.page {
                MINT.alpha(0.09)
            } else {
                Color::TRANSPARENT
            })
            .role("tab")
            .label(page.name())
            .hover(Color::WHITE.alpha(0.04))
            .on_press(move |s: &mut State| s.preferences_page(page)),
        );
    }
    let navigation = col()
        .w(super::locale::scale(154.0))
        .h_full()
        .padding(16.0)
        .gap(8.0)
        .child(
            col()
                .key("settings-pages")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .scroll_edge_fade(10.0)
                .child(pages),
        )
        .child(el().w_full().h(1.0).my(4.0).fill(BORDER))
        .child(
            row()
                .key("settings-logout")
                .px(12.0)
                .py(12.0)
                .gap(8.0)
                .center_y()
                .radius(10.0)
                .role("button")
                .label(super::locale::tr("Log out"))
                .help(super::locale::tr("Unlink this device from WhatsApp"))
                .hover(DANGER.alpha(0.08))
                .on_press(|s: &mut State| {
                    s.preferences.logout = true;
                    s.preferences.error = None;
                })
                .child(icon(&LOGOUT, 15.0).color(DANGER))
                .child(
                    label(super::locale::tr("Log out"), 12.0, DANGER)
                        .grow(1.0)
                        .min_w(0.0)
                        .truncate(),
                ),
        );
    let p = &s.preferences.value;
    let mut body = col().w_full().max_w(700.0).padding(24.0).gap(14.0);
    match s.preferences.page {
        Page::Appearance => {
            body = body.child(section("Appearance", "Make Brazap feel like home."));
            let mut fonts = col().gap(8.0).child(title(super::locale::tr("Font"), 13.0));
            for (id, name) in [
                ("system", "System"),
                ("Inter", "Inter"),
                ("Segoe UI", "Segoe UI"),
                ("Arial", "Arial"),
                ("monospace", "Monospace"),
            ] {
                if !super::fonts::available(id) {
                    continue;
                }
                fonts = fonts.child(choice(name, p.appearance.font == id, move |s| {
                    s.edit_preferences(|p| p.appearance.font = id.into())
                }));
            }
            body = body.child(card(fonts));
            let size = p.appearance.font_size;
            body = body.child(card(
                row()
                    .center_y()
                    .gap(12.0)
                    .child(title(super::locale::tr("Font size"), 13.0).grow(1.0))
                    .child(icon_button(&MINUS, "Decrease font size", |s, _| {
                        s.edit_preferences(|p| {
                            p.appearance.font_size =
                                p.appearance.font_size.saturating_sub(1).max(11)
                        })
                    }))
                    .child(label(format!("{size} px"), 14.0, INK))
                    .child(icon_button(&PLUS, "Increase font size", |s, _| {
                        s.edit_preferences(|p| {
                            p.appearance.font_size = (p.appearance.font_size + 1).min(19)
                        })
                    })),
            ));
            let mut langs = col()
                .gap(8.0)
                .child(title(super::locale::tr("Language"), 13.0));
            for (id, name) in [
                ("en", "English"),
                ("pt-BR", "Português (Brasil)"),
                ("es", "Español"),
            ] {
                langs = langs.child(choice(name, p.appearance.language == id, move |s| {
                    s.edit_preferences(|p| p.appearance.language = id.into())
                }));
            }
            body = body.child(card(langs));
        }
        Page::Notifications => {
            body = body
                .child(section(
                    "Notifications",
                    "Choose what appears on your desktop.",
                ))
                .child(toggle(
                    s,
                    "Desktop notifications",
                    "Show new message notifications.",
                    p.notifications.enabled,
                    |p| p.notifications.enabled = !p.notifications.enabled,
                ))
                .child(toggle(
                    s,
                    "Notification sound",
                    "Play a sound when a notification arrives.",
                    p.notifications.sound,
                    |p| p.notifications.sound = !p.notifications.sound,
                ))
                .child(toggle(
                    s,
                    "Message previews",
                    "Include the sender and message content in notifications.",
                    p.notifications.preview,
                    |p| p.notifications.preview = !p.notifications.preview,
                ));
            body = body.child(
                label(
                    super::locale::tr(
                        "Windows Focus Assist and system notification settings also apply.",
                    ),
                    11.0,
                    MUTED,
                )
                .wrap(),
            );
        }
        Page::Privacy => {
            body = body
                .child(section(
                    "Privacy",
                    "Local controls apply to this device. WhatsApp settings apply to your account.",
                ))
                .child(toggle(
                    s,
                    "Keep deleted messages visible",
                    "Retain the content and mark messages deleted on WhatsApp.",
                    p.privacy.retain_deleted,
                    |p| p.privacy.retain_deleted = !p.privacy.retain_deleted,
                ))
                .child(toggle(
                    s,
                    "Send read receipts",
                    "Mark messages read on WhatsApp when opening a chat.",
                    p.privacy.send_read_receipts,
                    |p| p.privacy.send_read_receipts = !p.privacy.send_read_receipts,
                ))
                .child(toggle(
                    s,
                    "Send typing indicator",
                    "Let others know when you are writing.",
                    p.privacy.send_typing,
                    |p| p.privacy.send_typing = !p.privacy.send_typing,
                ))
                .child(toggle(
                    s,
                    "Announce online presence",
                    "Report this device as online while the app window is in use.",
                    p.privacy.show_online,
                    |p| p.privacy.show_online = !p.privacy.show_online,
                ));
            body = body.child(title(super::locale::tr("WhatsApp account"), 17.0));
            if s.preferences.privacy_pending {
                body = body.child(label(super::locale::tr("Loading…"), 12.0, MUTED));
            }
            for (category, current) in &s.preferences.privacy {
                let (name, options): (&str, &[&str]) = match category.as_str() {
                    "last" => ("Last seen", &["all", "contacts", "none"]),
                    "online" => ("Online", &["all", "match_last_seen"]),
                    "profile" => ("Profile photo", &["all", "contacts", "none"]),
                    "status" => ("About", &["all", "contacts", "none"]),
                    "groupadd" => ("Who can add me to groups", &["all", "contacts", "none"]),
                    "readreceipts" => ("Account read receipts", &["all", "none"]),
                    "calladd" => ("Who can call me", &["all", "known", "contacts"]),
                    "messages" => ("Who can message me", &["all", "contacts"]),
                    "defense" => ("Strict account protection", &["off", "on_standard"]),
                    _ => continue,
                };
                let mut line = col().gap(10.0).child(title(super::locale::tr(name), 13.0));
                if current == "contact_blacklist" {
                    line = line.child(
                        label(
                            super::locale::tr(
                                "My contacts except… (manage exclusions on your phone)",
                            ),
                            11.0,
                            MUTED,
                        )
                        .wrap(),
                    );
                }
                for option in options {
                    let (cat, val) = (category.clone(), option.to_string());
                    let display = match *option {
                        "all" => "Everyone",
                        "contacts" => "My contacts",
                        "none" => "Nobody",
                        "match_last_seen" => "Same as last seen",
                        "known" => "Known contacts",
                        "off" => "Off",
                        "on_standard" => "On",
                        _ => option,
                    };
                    line = line.child(choice(display, current == option, move |s| {
                        if s.preferences.privacy_pending {
                            return;
                        }
                        s.preferences.privacy_pending = true;
                        s.preferences.error = None;
                        s.request_preference("privacy_set", json!({"category":cat,"value":val}));
                    }));
                }
                body = body.child(card(line));
            }
        }
        Page::Transcription => {
            body = body
                .child(section(
                    "Transcription",
                    "Voice messages are converted to text on this computer. Audio never leaves your device.",
                ))
                .child(toggle(
                    s,
                    "Transcribe voice messages",
                    "Convert voice messages on screen and new ones you receive to text. The speech model downloads the first time.",
                    p.transcription.enabled,
                    |p| p.transcription.enabled = !p.transcription.enabled,
                ));
            if p.transcription.enabled {
                let mut models = col()
                    .gap(4.0)
                    .child(title(super::locale::tr("Speech model"), 13.0))
                    .child(
                        label(
                            super::locale::tr("Larger models understand more but transcribe more slowly. A model you choose downloads first; the current one keeps working until it is ready. Only the model in use is kept on this computer."),
                            11.0,
                            MUTED,
                        )
                        .wrap(),
                    );
                for (index, model) in crate::transcription::MODELS.iter().enumerate() {
                    if index > 0 {
                        models = models.child(el().w_full().h(1.0).fill(BORDER));
                    }
                    models = models.child(model_row(s, model));
                }
                if let Some(status) = transcription_status(s) {
                    models = models.child(label(status, 11.0, MUTED).wrap());
                }
                body = body.child(card(models)).child(toggle(
                    s,
                    "Show re-transcribe button",
                    "Add a button to transcribed voice messages to transcribe them again with the selected model.",
                    p.transcription.retranscribe,
                    |p| p.transcription.retranscribe = !p.transcription.retranscribe,
                ));
            }
        }
        Page::Storage => {
            body = body.child(section(
                "Storage",
                "Choose how long downloaded media stays on this device after you last opened it.",
            ));
            let retention = &p.storage.retention;
            for (name, days) in [
                ("Photos", retention.photos),
                ("Videos", retention.videos),
                ("Stickers", retention.stickers),
                ("Audio", retention.audio),
                ("Files", retention.files),
            ] {
                let mut options = row().gap(8.0).flex_wrap();
                for option in crate::settings::RETENTION_CHOICES {
                    options =
                        options.child(choice(retention_label(option), days == option, move |s| {
                            s.edit_preferences(move |p| set_retention(p, name, option))
                        }));
                }
                body = body.child(card(
                    col()
                        .gap(10.0)
                        .child(title(super::locale::tr(name), 13.0))
                        .child(options),
                ));
            }
            body = body.child(
                label(
                    super::locale::tr("Messages, chat history and their original data are always kept. Removed media is downloaded again when you open it, as long as WhatsApp or your phone still has it."),
                    11.0,
                    MUTED,
                )
                .wrap(),
            );
            let report = &s.preferences.report;
            body = body.child(
                label(
                    super::locale::format(
                        "Database {size} · {free} can be released",
                        &[
                            (
                                "size",
                                bytes(report["database_bytes"].as_u64().unwrap_or(0)),
                            ),
                            (
                                "free",
                                bytes(report["reclaimable_bytes"].as_u64().unwrap_or(0)),
                            ),
                        ],
                    ),
                    11.0,
                    MUTED,
                )
                .wrap(),
            );
        }
        Page::Mcp => {
            body = super::mcp::view(s, body);
        }
        Page::Debug => {
            body = body
                .child(section(
                    "Debug",
                    "Storage, local files, and live memory diagnostics.",
                ))
                .child(button(
                    &super::locale::tr("Open memory diagnostics"),
                    |s, _| {
                        s.diagnostics.open = true;
                        s.diagnostics.next_sample = 0.0;
                    },
                ));
            let report = &s.preferences.report;
            if let Some(items) = report["items"].as_array() {
                for item in items {
                    let kind = item["kind"].as_str().unwrap_or("Other");
                    body = body.child(card(
                        row()
                            .center_y()
                            .gap(12.0)
                            .child(title(super::locale::tr(kind), 13.0).grow(1.0))
                            .child(label(
                                match item["count"].as_u64().unwrap_or(0) {
                                    0 => bytes(item["bytes"].as_u64().unwrap_or(0)),
                                    count => format!(
                                        "{count} · {}",
                                        bytes(item["bytes"].as_u64().unwrap_or(0))
                                    ),
                                },
                                12.0,
                                MUTED,
                            )),
                    ));
                }
            }
            body = body.child(label(
                format!(
                    "SQLite {} · WAL {}",
                    bytes(report["database_bytes"].as_u64().unwrap_or(0)),
                    bytes(report["wal_bytes"].as_u64().unwrap_or(0))
                ),
                12.0,
                MUTED,
            ));
            for (key, name) in [
                ("data_dir", "Open data folder"),
                ("logs_dir", "Open logs folder"),
                ("cache_dir", "Open cache folder"),
            ] {
                let path = report[key].as_str().unwrap_or("").to_owned();
                body = body.child(button(&super::locale::tr(name), move |s, _| {
                    if let Err(e) = open_folder(&path) {
                        s.preferences.error = Some(e.to_string());
                    }
                }));
            }
            body=body.child(button(&super::locale::tr("Clear cache"),|s,_|s.preferences.confirm_cache=true))
                .child(label(super::locale::tr("Clears thumbnails and avatars. Saved messages and original media stay in SQLite."),11.0,MUTED).wrap())
                .child(button(&super::locale::tr("Refresh storage"),|s,_|s.request_preference("get",json!({}))));
        }
        Page::About => {
            body = body
                .child(logo(s, 88.0))
                .child(title("Brazap", 30.0))
                .child(label(
                    super::locale::format(
                        "Version {version}",
                        &[("version", env!("CARGO_PKG_VERSION").into())],
                    ),
                    13.0,
                    MUTED,
                ))
                .child(
                    label(
                        super::locale::tr(
                            "A native WhatsApp client with a local MCP server for your agents.",
                        ),
                        14.0,
                        INK,
                    )
                    .wrap(),
                )
                .child(link("Source code", "https://github.com/0rdep/brazap"))
                .child(link("Rune", "https://github.com/anomalyco/rune"))
                .child(link(
                    "Twemoji · CC BY 4.0",
                    "https://github.com/jdecked/twemoji",
                ))
                .child(link(
                    "Unicode · CLDR",
                    "https://www.unicode.org/license.txt",
                ))
                .child(link(
                    "whatsapp-rust",
                    "https://github.com/oxidezap/whatsapp-rust",
                ))
                .child(
                    label(
                        super::locale::tr(
                            "MIT · Independent project; not affiliated with WhatsApp or Meta.",
                        ),
                        11.0,
                        MUTED,
                    )
                    .wrap(),
                );
        }
    }
    if let Some(error) = &s.preferences.error {
        body = body.child(
            label(
                error,
                12.0,
                Color {
                    r: 0.9,
                    g: 0.3,
                    b: 0.3,
                    a: 1.0,
                },
            )
            .wrap(),
        );
    }
    col()
        .w_full()
        .h_full()
        .child(
            row()
                .h(64.0)
                .px(24.0)
                .center_y()
                .child(title(super::locale::tr("Settings"), 19.0))
                .child(el().grow(1.0))
                .child(icon_button(&glyphs::CLOSE, "Close settings", |s, _| {
                    s.preferences.open = false
                })),
        )
        .child(
            row().grow(1.0).min_h(0.0).w_full().child(navigation).child(
                col()
                    .key(format!("settings-{}", s.preferences.page.name()))
                    .grow(1.0)
                    .min_w(0.0)
                    .h_full()
                    .scroll(ScrollSpec {
                        stick_bottom: false,
                    })
                    .child(body),
            ),
        )
}
fn bytes(n: u64) -> String {
    format!("{:.1} MiB", n as f64 / 1048576.0)
}
fn link(name: &str, url: &'static str) -> El {
    label(super::locale::tr(name), 13.0, MINT)
        .py(8.0)
        .role("link")
        .label(super::locale::tr(name))
        .on_press_tx(move |s: &mut State, tx| {
            if let Err(e) = tx.open_url(url) {
                s.preferences.error = Some(e.to_string());
            }
        })
}
fn open_folder(path: &str) -> anyhow::Result<()> {
    anyhow::ensure!(!path.is_empty(), "Folder is not available yet");
    std::fs::create_dir_all(path)?;
    #[cfg(target_os = "windows")]
    let program = "explorer.exe";
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    let program = "xdg-open";
    std::process::Command::new(program).arg(path).spawn()?;
    Ok(())
}
pub(super) fn confirmation(s: &State) -> Option<El> {
    let logout = s.preferences.logout;
    if !logout && !s.preferences.confirm_cache {
        return None;
    }
    let busy = s.preferences.logging_out;
    let content = col()
        .w(410.0)
        .padding(26.0)
        .gap(18.0)
        .child(title(
            super::locale::tr(if logout { "Log out?" } else { "Clear cache?" }),
            22.0,
        ))
        .child(
            label(
                super::locale::tr(if logout {
                    "Unlink this device from WhatsApp. Your saved history stays on this computer."
                } else {
                    "Remove cached avatars and previews? They will download again when needed."
                }),
                13.0,
                MUTED,
            )
            .wrap(),
        )
        .child(button(
            &super::locale::tr(if busy {
                "Logging out…"
            } else if logout {
                "Log out"
            } else {
                "Clear cache"
            }),
            move |s, _| {
                if s.preferences.logging_out {
                    return;
                }
                if logout {
                    s.begin_logout();
                } else {
                    s.preferences.confirm_cache = false;
                    s.request_preference("clear", json!({}));
                }
            },
        ))
        .child(button(&super::locale::tr("Cancel"), |s, _| {
            if !s.preferences.logging_out {
                s.preferences.logout = false;
                s.preferences.confirm_cache = false;
            }
        }))
        .child(label(s.preferences.error.as_deref().unwrap_or(""), 12.0, MUTED).wrap())
        .on_press(|_: &mut State| {});
    Some(
        el().key("settings-confirmation")
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .center()
            .fill(Color::BLACK.alpha(0.72))
            .role("dialog")
            .label(super::locale::tr(
                &(if logout { "Log out" } else { "Clear cache" }),
            ))
            .on_press(|s: &mut State| {
                if !s.preferences.logging_out {
                    s.preferences.logout = false;
                    s.preferences.confirm_cache = false;
                }
            })
            .child(glass(content)),
    )
}
