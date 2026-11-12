//! Phone numbers and WhatsApp click-to-chat links (`wa.me`, `api.whatsapp.com/send`,
//! `whatsapp://send`), turned into the number a chat can be opened with.
use phonenumber::Mode;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PhoneTarget {
    /// The number in international form without `+`, as WhatsApp addresses users.
    pub digits: String,
    /// The number formatted for people, such as `+55 11 98091-2212`.
    pub display: String,
    /// The message a link asks to start the chat with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// The region (such as `BR`) of an international number given as digits, used to read
/// numbers typed without a country code.
pub fn region_of(digits: &str) -> Option<String> {
    let number = phonenumber::parse(None, format!("+{digits}")).ok()?;
    number.country().id().map(|id| id.as_ref().to_owned())
}

/// Reads a typed phone number or a WhatsApp link. Typed numbers without a country code
/// use `region`; links always carry the full international number.
pub fn parse(input: &str, region: Option<&str>) -> Option<PhoneTarget> {
    let input = input.trim();
    if let Some((phone, text)) = link(input) {
        let mut target = number(&format!("+{phone}"), None, false)?;
        target.text = text.filter(|text| !text.trim().is_empty());
        return Some(target);
    }
    if input.len() > 32
        || !input
            .chars()
            .all(|c| c.is_ascii_digit() || " +-().".contains(c))
    {
        return None;
    }
    let digits = input.chars().filter(char::is_ascii_digit).count();
    if !(8..=15).contains(&digits) {
        return None;
    }
    number(input, region, true)
}

fn number(raw: &str, region: Option<&str>, strict: bool) -> Option<PhoneTarget> {
    let region = if raw.starts_with('+') {
        None
    } else {
        Some(region?.parse().ok()?)
    };
    let parsed = phonenumber::parse(region, raw).ok()?;
    // Typed text is only treated as a number when it is one; links name the number explicitly.
    if strict && !phonenumber::is_valid(&parsed) {
        return None;
    }
    let e164 = parsed.format().mode(Mode::E164).to_string();
    let digits = e164.trim_start_matches('+').to_owned();
    if !(8..=15).contains(&digits.len()) {
        return None;
    }
    Some(PhoneTarget {
        digits,
        display: parsed.format().mode(Mode::International).to_string(),
        text: None,
    })
}

/// The phone digits and optional message of a click-to-chat link.
fn link(input: &str) -> Option<(String, Option<String>)> {
    let lower = input.to_ascii_lowercase();
    let (rest, app) = if let Some(rest) = lower.strip_prefix("whatsapp://") {
        (rest, true)
    } else {
        let rest = lower
            .strip_prefix("https://")
            .or_else(|| lower.strip_prefix("http://"))
            .unwrap_or(&lower);
        (rest.strip_prefix("www.").unwrap_or(rest), false)
    };
    // Query values keep their original case; only the scheme, host and path are compared.
    let offset = input.len() - rest.len();
    let (path, query) = match rest.find('?') {
        Some(at) => (&rest[..at], Some(&input[offset + at + 1..])),
        None => (rest, None),
    };
    let path = path
        .split('#')
        .next()
        .unwrap_or_default()
        .trim_end_matches('/');
    let param = |name: &str| {
        query?.split('&').find_map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (key.eq_ignore_ascii_case(name)).then(|| decode(value))
        })
    };
    let phone = if let Some(phone) = path.strip_prefix("wa.me/") {
        phone.to_owned()
    } else if (app && path == "send")
        || matches!(
            path,
            "api.whatsapp.com/send" | "web.whatsapp.com/send" | "whatsapp.com/send"
        )
    {
        param("phone")?
    } else {
        return None;
    };
    let phone = phone
        .chars()
        .filter(|c| !" +-().".contains(*c))
        .collect::<String>();
    if !(8..=15).contains(&phone.len()) || !phone.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((phone, param("text")))
}

fn decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'+' => out.push(b' '),
            b'%' if at + 2 < bytes.len() => {
                let hex = |b: u8| (b as char).to_digit(16);
                match (hex(bytes[at + 1]), hex(bytes[at + 2])) {
                    (Some(high), Some(low)) => {
                        out.push((high * 16 + low) as u8);
                        at += 2;
                    }
                    _ => out.push(b'%'),
                }
            }
            byte => out.push(byte),
        }
        at += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
