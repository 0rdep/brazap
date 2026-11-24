use super::locale;
use chrono::{DateTime, Datelike, Local, NaiveDate};

pub(super) fn day(timestamp: i64) -> Option<NaiveDate> {
    (timestamp > 0)
        .then(|| DateTime::from_timestamp(timestamp, 0))
        .flatten()
        .map(|t| t.with_timezone(&Local).date_naive())
}
pub(super) fn day_label(timestamp: i64) -> String {
    let Some(date) = day(timestamp) else {
        return String::new();
    };
    let today = Local::now().date_naive();
    if date == today {
        locale::tr("Today")
    } else if Some(date) == today.pred_opt() {
        locale::tr("Yesterday")
    } else {
        locale::format(
            "{weekday}, {date}",
            &[
                ("weekday", locale::tr(&date.format("%A").to_string())),
                ("date", full_date(date)),
            ],
        )
    }
}
pub(super) fn chat_date(timestamp: i64) -> String {
    let Some(date) = day(timestamp) else {
        return String::new();
    };
    let today = Local::now().date_naive();
    if date == today {
        super::view::clock(timestamp)
    } else if Some(date) == today.pred_opt() {
        locale::tr("Yesterday")
    } else if date < today && (today - date).num_days() < 7 {
        // As WhatsApp does: the weekday within the last week, then a short date,
        // so the date never crowds the chat's name.
        locale::tr(&date.format("%A").to_string())
    } else {
        locale::format(
            "{day}/{month}/{year}",
            &[
                ("day", format!("{:02}", date.day())),
                ("month", format!("{:02}", date.month())),
                ("year", date.year().to_string()),
            ],
        )
    }
}

/// "Deleted on WhatsApp at 14:32", naming the day too when it differs from the
/// day the message was sent.
pub(super) fn deleted_label(sent: i64, deleted: i64) -> String {
    let time = super::view::clock(deleted);
    if time.is_empty() {
        return locale::tr("Deleted on WhatsApp");
    }
    match day(deleted) {
        Some(date) if day(sent) != Some(date) => locale::format(
            "Deleted on WhatsApp on {date} at {time}",
            &[("date", full_date(date)), ("time", time)],
        ),
        _ => locale::format("Deleted on WhatsApp at {time}", &[("time", time)]),
    }
}

fn full_date(date: NaiveDate) -> String {
    locale::format(
        "{day} {month} {year}",
        &[
            ("day", date.day().to_string()),
            ("month", locale::tr(&date.format("%B").to_string())),
            ("year", date.year().to_string()),
        ],
    )
}
