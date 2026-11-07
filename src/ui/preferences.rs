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
    name: "settings",
    view: 24.0,
    body: r#"<path d="m9 3-.7 3-2.6 1L3 6l-2 4 2.5 2L3 15l2 4 3-.5 2 2.5h4l1-3 3-1 2.5 1 2-4-2.5-2V9l-2-4-3 .5L14 3Z" transform="translate(1 0) scale(.9)" fill="none" stroke="currentColor" stroke-width="1.6"/><circle cx="12" cy="12" r="3" fill="none" stroke="currentColor" stroke-width="1.6"/>"#,
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
    Mcp,
    Updates,
    Debug,
    About,
}
impl Page {
    fn name(self) -> &'static str {
        match self {
            Self::Appearance => "Appearance",
            Self::Notifications => "Notifications",
            Self::Privacy => "Privacy",
            Self::Mcp => "MCP",
            Self::Updates => "Updates",
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
    pub update: Value,
    pub notice: Option<String>,
    pub error: Option<String>,
    pub logout: bool,
    pub logging_out: bool,
    pub confirm_cache: bool,
}
impl State {
    pub(super) fn open_preferences(&mut self) {
        self.preferences.open = true;
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
    pub(super) fn edit_preferences(&mut self, change: impl FnOnce(&mut Settings)) {
        let previous = self.preferences.value.clone();
        change(&mut self.preferences.value);
        if previous == self.preferences.value {
            return;
        }
        self.preferences.error = None;
        self.preferences.notice = None;
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
        self.preferences.notice = None;
        self.preferences.error = None;
        if page == Page::Privacy {
            self.preferences.privacy_pending = true;
            self.request_preference("privacy", json!({}));
        }
        if page == Page::Debug || page == Page::Updates {
            self.request_preference("get", json!({}));
        }
    }
    pub(super) fn receive_preferences(&mut self, action: &str, result: Result<Value, String>) {
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
                        self.preferences.notice = Some("Saved".into());
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
        match action {
            "get" => self.preferences.loading = false,
            "logout" => self.preferences.logging_out = false,
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
                    self.preferences.update = value["updates"].clone();
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
                }
                "update_check" | "update_download" => self.preferences.update = value,
                "clear" => {
                    self.images.clear();
                    self.animations.clear();
                    self.avatars.clear();
                    self.avatar_versions.clear();
                    self.image_order.clear();
                    self.image_failures.clear();
                    self.preferences.confirm_cache = false;
                    self.preferences.notice = Some(
                        "Cache cleared. Saved messages and original attachments were kept.".into(),
                    );
                    self.request_preference("get", json!({}));
                }
                "logout" => {
                    self.preferences.logout = false;
                    self.preferences.open = false;
                    self.authenticated = false;
                    self.selected = None;
                    self.timeline.clear();
                    self.snapshot = Default::default();
                    self.avatars.clear();
                    self.images.clear();
                    self.animations.clear();
                }
                _ => {}
            },
        }
    }
}

pub(super) fn footer(s: &State) -> El {
    let small = !s.sidebar.is_open();
    let item = |glyph: &'static Icon, name: &str, action: fn(&mut State)| {
        let mut v = row()
            .h(super::locale::scale(38.0))
            .gap(10.0)
            .px(10.0)
            .center_y()
            .radius(11.0)
            .role("button")
            .label(super::locale::tr(name))
            .help(super::locale::tr(name))
            .hover(Color::WHITE.alpha(0.05))
            .on_press(action)
            .child(icon(glyph, 18.0).color(MUTED));
        if small {
            v = v.w(40.0).center();
        } else {
            v = v.w_full().child(
                label(super::locale::tr(name), 12.0, MUTED)
                    .truncate()
                    .grow(1.0)
                    .min_w(0.0),
            );
        }
        v
    };
    col()
        .w_full()
        .padding(if small { 14.0 } else { 16.0 })
        .gap(5.0)
        .child(item(&SETTINGS, "Settings", |s| s.open_preferences()))
        .child(item(&LOGOUT, "Log out", |s| {
            s.preferences.logout = true;
            s.preferences.error = None;
        }))
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
fn toggle(
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

pub(super) fn view(s: &State) -> El {
    let mut navigation = col().w(154.0).h_full().padding(16.0).gap(8.0);
    for page in [
        Page::Appearance,
        Page::Notifications,
        Page::Privacy,
        Page::Mcp,
        Page::Updates,
        Page::Debug,
        Page::About,
    ] {
        navigation = navigation.child(
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
                    "Report this device as online while connected.",
                    p.privacy.show_online,
                    |p| p.privacy.show_online = !p.privacy.show_online,
                ))
                .child(title(super::locale::tr("WhatsApp account"), 17.0));
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
            body = body.child(button(
                &super::locale::tr("Refresh WhatsApp settings"),
                |s, _| {
                    s.preferences.privacy_pending = true;
                    s.request_preference("privacy", json!({}));
                },
            ));
        }
        Page::Mcp => {
            body = super::mcp::view(s, body);
        }
        Page::Updates => {
            body = body
                .child(section(
                    "Updates",
                    "Updates are downloaded from Brazap releases on GitHub.",
                ))
                .child(toggle(
                    s,
                    "Automatic updates",
                    "Check periodically and download updates. Restart when you are ready.",
                    p.updates.automatic,
                    |p| p.updates.automatic = !p.updates.automatic,
                ))
                .child(title(format!("Brazap {}", env!("CARGO_PKG_VERSION")), 18.0));
            let update = &s.preferences.update;
            let status = update["status"].as_str().unwrap_or("idle");
            let description = match status {
                "unpublished" => "No release has been published yet.",
                "up_to_date" => "You are up to date.",
                "available" => "A new version is available.",
                "ready" => "Update downloaded. Restart to install.",
                "checking" => "Checking for updates…",
                "error" => "Could not check for updates.",
                _ => "Ready to check for updates.",
            };
            body = body.child(label(super::locale::tr(description), 13.0, INK).wrap());
            if let Some(latest) = update["latest"].as_str() {
                body = body.child(label(
                    super::locale::format("Latest: {version}", &[("version", latest.into())]),
                    12.0,
                    MUTED,
                ));
            }
            if let Some(error) = update["error"].as_str() {
                body = body.child(label(super::locale::tr(error), 12.0, MUTED).wrap());
            }
            body = body.child(button(&super::locale::tr("Check for updates"), |s, _| {
                s.preferences.update = json!({"status":"checking"});
                s.request_preference("update_check", json!({}));
            }));
            if status == "available" {
                body = body.child(button(&super::locale::tr("Download update"), |s, _| {
                    s.preferences.update["status"] = json!("checking");
                    s.request_preference("update_download", json!({}));
                }));
            }
            if status == "ready" {
                body = body.child(button(
                    &super::locale::tr("Restart and install"),
                    |s, tx| match crate::updates::install(&s.config) {
                        Ok(()) => tx.quit(),
                        Err(e) => s.preferences.error = Some(e.to_string()),
                    },
                ));
            }
            body = body.child(link(
                "View releases",
                "https://github.com/0rdep/brazap/releases",
            ));
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
                                format!(
                                    "{} · {}",
                                    item["count"].as_u64().unwrap_or(0),
                                    bytes(item["bytes"].as_u64().unwrap_or(0))
                                ),
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
    if s.preferences.saving {
        body = body.child(label(super::locale::tr("Saving…"), 11.0, MUTED));
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
    if let Some(notice) = &s.preferences.notice {
        body = body.child(label(super::locale::tr(notice), 11.0, MINT).wrap());
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
                    s.preferences.logging_out = true;
                    s.request_preference("logout", json!({}));
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
