use std::{collections::HashMap, sync::OnceLock};

use phonenumber::{Mode, metadata::DATABASE};
use rune::{Color, El, Image, Key, ScrollSpec, TextEdit, col, el, icon, row};
use rune_kit::{Placement, Popover, TextInput, glyphs};
use serde::Deserialize;
use serde_json::json;

use super::{State, locale, picker, view::*};

#[derive(Deserialize)]
struct Country {
    id: String,
    dial: String,
    example: String,
}

fn countries() -> &'static [Country] {
    static COUNTRIES: OnceLock<Vec<Country>> = OnceLock::new();
    COUNTRIES.get_or_init(|| {
        serde_json::from_str(include_str!("../../assets/phone/countries.json"))
            .expect("Bundled phone country catalog")
    })
}

fn country(id: &str) -> &'static Country {
    countries().iter().find(|c| c.id == id).unwrap_or_else(|| {
        countries()
            .iter()
            .find(|c| c.id == "BR")
            .expect("Brazil in country catalog")
    })
}

fn name(country: &Country) -> String {
    locale::tr(&format!("country.{}", country.id))
}

fn digits(value: &str) -> String {
    value.chars().filter(char::is_ascii_digit).collect()
}

fn flag_text(id: &str) -> String {
    id.bytes()
        .filter_map(|b| char::from_u32(0x1f1e6 + u32::from(b - b'A')))
        .collect()
}

pub(super) struct Picker {
    id: String,
    open: bool,
    search: TextEdit,
    active: usize,
    flags: HashMap<String, Image>,
}

impl Default for Picker {
    fn default() -> Self {
        Self {
            id: "BR".into(),
            open: false,
            search: TextEdit::new(""),
            active: 0,
            flags: HashMap::new(),
        }
    }
}

impl Picker {
    fn filtered(&self) -> Vec<&'static Country> {
        let query = self.search.text.trim().to_lowercase();
        let mut result: Vec<_> = countries()
            .iter()
            .filter(|c| {
                query.is_empty()
                    || name(c).to_lowercase().contains(&query)
                    || c.id.to_lowercase().contains(&query)
                    || format!("+{}", c.dial).contains(&query)
            })
            .collect();
        result.sort_by_key(|c| (c.id != "BR", name(c)));
        result
    }

    pub(super) fn close(&mut self) {
        self.open = false;
        self.search.focused = false;
        self.flags.retain(|id, _| id == &self.id);
    }
}

impl State {
    pub(super) fn load_phone_flags(&mut self, all: bool) {
        let Some(cache) = &self.image_cache else {
            return;
        };
        for c in countries()
            .iter()
            .filter(|c| all || c.id == self.phone_country.id)
        {
            if !self.phone_country.flags.contains_key(&c.id)
                && let Some(bytes) = picker::emoji_artwork(&flag_text(&c.id))
            {
                self.phone_country
                    .flags
                    .insert(c.id.clone(), cache.load(bytes.to_vec()));
            }
        }
    }

    fn choose_country(&mut self, id: &str) {
        self.phone_country.id = country(id).id.clone();
        self.phone_country.close();
        self.format_phone_input();
        self.error = None;
    }

    pub(super) fn format_phone_input(&mut self) {
        if self.phone.preedit.is_some() {
            return;
        }
        let mut raw = self.phone.text.clone();
        let mut caret_digits = digits(&raw[..self.phone.focus.min(raw.len())]).len();
        let mut anchor_digits = digits(&raw[..self.phone.anchor.min(raw.len())]).len();
        if raw.trim_start().starts_with('+')
            && let Ok(number) = phonenumber::parse(None, &raw)
            && let Some(id) = number.country().id()
        {
            self.phone_country.id = id.as_ref().into();
            let prefix = number.code().value().to_string().len();
            caret_digits = caret_digits.saturating_sub(prefix);
            anchor_digits = anchor_digits.saturating_sub(prefix);
            raw = number.national().to_string();
            self.load_phone_flags(false);
        }
        let formatted = mask(country(&self.phone_country.id), &digits(&raw));
        if self.phone.text != formatted {
            self.phone.set_selection(0, self.phone.text.len());
            self.phone.insert(&formatted);
            let position = |count| {
                if count == 0 {
                    return 0;
                }
                formatted
                    .char_indices()
                    .filter(|(_, c)| c.is_ascii_digit())
                    .nth(count - 1)
                    .map_or(formatted.len(), |(i, _)| i + 1)
            };
            self.phone
                .set_selection(position(anchor_digits), position(caret_digits));
        }
    }

    pub(super) fn request_phone_pairing(&mut self) {
        if self.busy
            || self
                .snapshot
                .connection
                .pair_retry_at
                .is_some_and(|at| at > self.second)
        {
            return;
        }
        let c = country(&self.phone_country.id);
        let number = phonenumber::parse(c.id.parse().ok(), &self.phone.text);
        match number {
            Ok(number) if number.is_valid() && number.code().value().to_string() == c.dial => {
                self.error = None;
                self.tool(
                    "pair_phone",
                    json!({"phone":number.format().mode(Mode::E164).to_string()}),
                );
            }
            _ => self.error = Some("Enter a valid phone number for the selected country.".into()),
        }
    }
}

/// Use the library's country-specific grouping rules on a padded candidate,
/// then retain only real digits. Padding is never written to the input or sent.
fn mask(c: &Country, value: &str) -> String {
    if value.is_empty() {
        return String::new();
    }
    let Some(meta) = DATABASE.by_id(&c.id) else {
        return value.into();
    };
    // Some countries accept a domestic trunk zero that is omitted after the
    // calling code. Keep it visible while typing; parsing handles it on submit.
    let (prefix, value) = if meta.national_prefix() == Some("0") && value.starts_with('0') {
        ("0", &value[1..])
    } else {
        ("", value)
    };
    if value.is_empty() {
        return prefix.into();
    }
    let minimum_length = meta
        .descriptors()
        .general()
        .possible_length()
        .iter()
        .copied()
        .min()
        .unwrap_or(4) as usize;
    let formats = if meta.international_formats().is_empty() {
        meta.formats()
    } else {
        meta.international_formats()
    };
    for format in formats {
        if format
            .leading_digits()
            .last()
            .is_some_and(|r| !r.is_match(value))
        {
            continue;
        }
        for length in value.len().max(minimum_length)..=15 {
            let candidate = format!("{value}{}", "0".repeat(length - value.len()));
            let Some(captures) = format.pattern().captures(&candidate) else {
                continue;
            };
            if captures
                .get(0)
                .is_none_or(|m| m.as_str().len() != candidate.len())
            {
                continue;
            }
            let mut full = String::new();
            captures.expand(format.format(), &mut full);
            // Avoid rules that insert/remove digits (carrier/trunk prefixes).
            if digits(&full) != candidate {
                continue;
            }
            let mut result = String::new();
            let mut used = 0;
            for ch in full.chars() {
                if used == value.len() {
                    if ch == ')' {
                        result.push(ch);
                    }
                    break;
                }
                result.push(ch);
                if ch.is_ascii_digit() {
                    used += 1;
                }
            }
            return format!("{prefix}{result}");
        }
    }
    format!("{prefix}{value}")
}

fn flag(s: &State, c: &Country) -> El {
    let box_ = el().size(24.0, 18.0).center();
    match s.phone_country.flags.get(&c.id) {
        Some(image) => box_.pixels_fit(image.pixels(), rune::SurfaceFit::Fit),
        None => box_.child(label(&c.id, 10.0, MUTED)),
    }
}

pub(super) fn input(s: &State, layer: &mut Vec<El>) -> El {
    let c = country(&s.phone_country.id);
    let trigger = row()
        .h(44.0)
        .w(110.0)
        .px(10.0)
        .gap(7.0)
        .center_y()
        .radius(10.0)
        .fill(PANEL)
        .border(1.0, BORDER)
        .role("button")
        .label(format!(
            "{}: {} +{}",
            locale::tr("Country"),
            name(c),
            c.dial
        ))
        .help(name(c))
        .child(flag(s, c))
        .child(label(format!("+{}", c.dial), 13.0, INK))
        .child(icon(&glyphs::CHEVRON_DOWN, 12.0).color(MUTED))
        .on_press(|s: &mut State| {
            if s.phone_country.open {
                s.phone_country.close();
                return;
            }
            s.phone_country.open = true;
            s.phone_country.search.clear();
            s.phone_country.search.focused = s.phone_country.open;
            s.phone_country.active = 0;
            s.load_phone_flags(s.phone_country.open);
        });
    let mut popover = Popover::new("pairing-country")
        .trigger(trigger)
        .placement(Placement::BottomStart)
        .padding(0.0)
        .open(s.phone_country.open)
        .on_close(|s: &mut State| s.phone_country.close());
    if s.phone_country.open {
        let choices = s.phone_country.filtered();
        let search = TextInput::new(&s.phone_country.search)
            .plain()
            .width(None)
            .placeholder(locale::tr("Search countries or calling codes"))
            .on_edit(|s: &mut State| &mut s.phone_country.search)
            .on_change(|s: &mut State, tx| {
                s.phone_country.active = 0;
                tx.scroll_to_y("country-results", 0.0);
            })
            .on_key(|s: &mut State, event, tx| {
                let choices = s.phone_country.filtered();
                if s.phone_country.search.preedit.is_some() {
                    return false;
                }
                match event.key {
                    Key::Escape => s.phone_country.close(),
                    Key::Enter => {
                        if let Some(c) = choices.get(s.phone_country.active) {
                            s.choose_country(&c.id);
                        }
                    }
                    Key::Down | Key::Up if !choices.is_empty() => {
                        let step = if event.key == Key::Down {
                            1
                        } else {
                            choices.len() - 1
                        };
                        s.phone_country.active = (s.phone_country.active + step) % choices.len();
                        tx.scroll_into_view(format!(
                            "country-results-option-{}",
                            choices[s.phone_country.active].id
                        ));
                    }
                    _ => return false,
                }
                true
            })
            .build(kit());
        let mut rows = col().w_full();
        for (i, c) in choices.iter().enumerate() {
            let id = c.id.clone();
            rows = rows.child(
                row()
                    .key(format!("country-results-option-{}", c.id))
                    .w_full()
                    .h(34.0)
                    .px(8.0)
                    .gap(9.0)
                    .center_y()
                    .radius(8.0)
                    .role("option")
                    .label(format!("{} +{}", name(c), c.dial))
                    .fill(if i == s.phone_country.active {
                        MINT.alpha(0.12)
                    } else {
                        Color::TRANSPARENT
                    })
                    .hover(MINT.alpha(0.12))
                    .on_hover(move |s: &mut State| s.phone_country.active = i)
                    .on_press(move |s: &mut State| s.choose_country(&id))
                    .child(flag(s, c))
                    .child(label(name(c), 12.0, INK).grow(1.0).min_w(0.0).truncate())
                    .child(label(format!("+{}", c.dial), 12.0, MUTED)),
            );
        }
        if choices.is_empty() {
            rows = rows.child(label(locale::tr("No results"), 12.0, MUTED).padding(10.0));
        }
        let list = col()
            .key("country-results")
            .w_full()
            .h((choices.len().clamp(1, 7) * 34) as f32)
            .role("listbox")
            .label(locale::tr("Country"))
            .scroll(ScrollSpec {
                stick_bottom: false,
            })
            .clip_soft(0.0)
            .child(rows);
        popover = popover.child(
            col()
                .w(334.0)
                .padding(8.0)
                .gap(6.0)
                .child(search)
                .child(list),
        );
    }
    row()
        .w_full()
        .gap(8.0)
        .child(popover.build(kit(), layer))
        .child(
            TextInput::new(&s.phone)
                .placeholder(mask(c, &c.example))
                .width(None)
                .on_edit(|s: &mut State| &mut s.phone)
                .on_change(|s: &mut State, _| s.format_phone_input())
                .on_submit(|s: &mut State, _| s.request_phone_pairing())
                .build(kit())
                .h(44.0)
                .grow(1.0)
                .min_w(0.0),
        )
}
