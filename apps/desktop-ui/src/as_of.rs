//! "Parts as of 09:12 yesterday": when an offline team's parts last
//! matched its server (kicad-parts-ed46), in the person's local time.
//!
//! The Teams menu and the part panel's offline note say it. The time sits
//! in a `<time>`, so journeys can blank it out of screenshots (mx-1710c5).

use leptos::prelude::*;
use wasm_bindgen::JsValue;

/// Words that may hold one "as of" time: `before`, the time, then `after`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Said {
    pub(crate) before: String,
    /// When, in milliseconds since 1970; `None` for words with no time.
    pub(crate) as_of_ms: Option<u64>,
    pub(crate) after: String,
}

impl Said {
    /// Words with no time in them.
    pub(crate) fn plain(words: String) -> Self {
        Self {
            before: words,
            as_of_ms: None,
            after: String::new(),
        }
    }

    /// `before`, then "Parts as of" the time, then `after`.
    pub(crate) fn parts_as_of(before: &str, as_of_ms: u64, after: &str) -> Self {
        Self {
            before: format!("{before}Parts as of "),
            as_of_ms: Some(as_of_ms),
            after: format!(".{after}"),
        }
    }

    /// The words in one line, the time as seen at `now_ms`, for tests.
    #[cfg(test)]
    pub(crate) fn text(&self, now_ms: u64) -> String {
        let time = self
            .as_of_ms
            .map(|at| as_of_words(at, now_ms))
            .unwrap_or_default();
        format!("{}{time}{}", self.before, self.after)
    }
}

/// Shows `said`, its time in a `<time>` with the full UTC time for
/// machines.
#[component]
pub(crate) fn SaidView(said: Said) -> impl IntoView {
    let time = said.as_of_ms.map(|at| {
        let date = js_sys::Date::new(&JsValue::from_f64(at as f64));
        let datetime: String = date.to_iso_string().into();
        let shown = as_of_words(at, js_sys::Date::now() as u64);
        view! { <time datetime=datetime>{shown}</time> }
    });
    view! { {said.before}{time}{said.after} }
}

/// `at_ms` in local time, as seen at `now_ms`: `09:12` today, `09:12
/// yesterday`, `09:12 on 3 October` this year, else with the year.
pub(crate) fn as_of_words(at_ms: u64, now_ms: u64) -> String {
    const DAY_MS: u64 = 24 * 60 * 60 * 1000;
    let at = local(at_ms);
    let clock = format!("{:02}:{:02}", at.hour, at.minute);
    let today = local(now_ms);
    let yesterday = local(now_ms.saturating_sub(DAY_MS));
    // A time ahead of this computer's clock reads as today's.
    if at.date() == today.date() || at_ms > now_ms {
        clock
    } else if at.date() == yesterday.date() {
        format!("{clock} yesterday")
    } else if at.year == today.year {
        format!("{clock} on {} {}", at.day, MONTHS[at.month as usize])
    } else {
        format!(
            "{clock} on {} {} {}",
            at.day, MONTHS[at.month as usize], at.year
        )
    }
}

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// A moment as a calendar shows it.
struct Local {
    year: i64,
    /// 0 for January.
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
}

impl Local {
    fn date(&self) -> (i64, u32, u32) {
        (self.year, self.month, self.day)
    }
}

/// `ms` in the person's time zone, as the web view knows it.
#[cfg(target_arch = "wasm32")]
fn local(ms: u64) -> Local {
    let date = js_sys::Date::new(&JsValue::from_f64(ms as f64));
    Local {
        year: i64::from(date.get_full_year()),
        month: date.get_month(),
        day: date.get_date(),
        hour: date.get_hours(),
        minute: date.get_minutes(),
    }
}

/// `ms` in UTC: tests run outside a web view, which alone knows the
/// person's time zone.
#[cfg(not(target_arch = "wasm32"))]
fn local(ms: u64) -> Local {
    let minutes = ms / 60_000;
    let days = (minutes / (24 * 60)) as i64;
    let (year, month, day) = civil_from_days(days);
    Local {
        year,
        month: month - 1,
        day,
        hour: ((minutes / 60) % 24) as u32,
        minute: (minutes % 60) as u32,
    }
}

/// The calendar date `days` after 1970-01-01, as (year, month 1-12, day):
/// Howard Hinnant's `civil_from_days`.
#[cfg(not(target_arch = "wasm32"))]
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-10-24 14:30 UTC.
    const NOW: u64 = 1_792_852_200_000;
    const MINUTE: u64 = 60_000;
    const DAY: u64 = 24 * 60 * MINUTE;

    #[test]
    fn the_time_says_the_day_only_when_it_is_not_today() {
        assert_eq!(as_of_words(NOW - 78 * MINUTE, NOW), "13:12");
        assert_eq!(
            as_of_words(NOW - DAY - 318 * MINUTE, NOW),
            "09:12 yesterday"
        );
        assert_eq!(as_of_words(NOW - 21 * DAY, NOW), "14:30 on 3 October");
        assert_eq!(
            as_of_words(NOW - 365 * DAY, NOW),
            "14:30 on 24 October 2025"
        );
        // Ahead of this computer's clock: read as today's.
        assert_eq!(as_of_words(NOW + 2 * MINUTE, NOW), "14:32");
    }

    #[test]
    fn said_words_hold_the_time_between_them() {
        let said = Said::parts_as_of("Offline. ", NOW - 78 * MINUTE, " The app keeps trying.");
        assert_eq!(
            said.text(NOW),
            "Offline. Parts as of 13:12. The app keeps trying."
        );
        assert_eq!(Said::plain("Signed out.".into()).text(NOW), "Signed out.");
    }
}
