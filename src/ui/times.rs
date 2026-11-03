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
    } else {
        full_date(date)
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
