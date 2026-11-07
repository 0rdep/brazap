use crate::settings::AppearanceSettings;
use rune::TextStyle;
thread_local! {static APPEARANCE:std::cell::RefCell<AppearanceSettings>=std::cell::RefCell::new(AppearanceSettings::default());}
pub(super) fn configure(value: &AppearanceSettings) {
    APPEARANCE.with(|s| *s.borrow_mut() = value.clone());
}
pub(super) fn style(size: f32, weight: f32) -> TextStyle {
    APPEARANCE.with(|s| {
        let a = s.borrow();
        let family = super::fonts::family(&a.font);
        TextStyle::new(size * a.font_size as f32 / 13.0, weight).family(family)
    })
}
/// Fixed rows and controls must grow with enlarged text instead of clipping
/// or letting the label paint over the next row.
pub(super) fn scale(points: f32) -> f32 {
    APPEARANCE.with(|settings| points * (settings.borrow().font_size as f32 / 13.0).max(1.0))
}
fn catalog(language: &str) -> &'static std::collections::HashMap<String, String> {
    use std::{collections::HashMap, sync::OnceLock};
    static EN: OnceLock<HashMap<String, String>> = OnceLock::new();
    static PT: OnceLock<HashMap<String, String>> = OnceLock::new();
    static ES: OnceLock<HashMap<String, String>> = OnceLock::new();
    let (slot, source) = match language {
        "pt-BR" => (&PT, include_str!("../../locales/pt-BR.json")),
        "es" => (&ES, include_str!("../../locales/es.json")),
        _ => (&EN, include_str!("../../locales/en.json")),
    };
    slot.get_or_init(|| serde_json::from_str(source).expect("Bundled locale catalog"))
}

#[cfg(target_os = "windows")]
pub(crate) fn in_language(value: &str, language: &str) -> String {
    catalog(language)
        .get(value)
        .or_else(|| catalog("en").get(value))
        .cloned()
        .unwrap_or_else(|| value.into())
}

pub(super) fn has_key(key: &str) -> bool {
    catalog("en").contains_key(key)
}

pub(super) fn tr(value: &str) -> String {
    APPEARANCE.with(|settings| {
        catalog(&settings.borrow().language)
            .get(value)
            .or_else(|| catalog("en").get(value))
            .cloned()
            .unwrap_or_else(|| value.to_owned())
    })
}

pub(super) fn format(key: &str, values: &[(&str, String)]) -> String {
    let mut text = tr(key);
    for (key, value) in values {
        text = text.replace(&format!("{{{key}}}"), value);
    }
    text
}

pub(super) fn kit_index() -> usize {
    APPEARANCE.with(|s| {
        let a = s.borrow();
        let font = match a.font.as_str() {
            "Inter" => 1,
            "Segoe UI" => 2,
            "Arial" => 3,
            "monospace" => 4,
            _ => 0,
        };
        font * 9 + (a.font_size.clamp(11, 19) - 11) as usize
    })
}
