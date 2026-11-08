// SPDX-License-Identifier: GPL-3.0-or-later
//! Daily schedule: applies profiles at set times.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Entry {
    pub time: String, // "HH:MM"
    pub profile: String,
    pub enabled: bool,
    /// days of the week it applies to (bit 0 = Monday ... bit 6 = Sunday)
    #[serde(default = "all_days")]
    pub days: u8,
    /// optional end time ("HH:MM"): when reached, goes back to what was set before
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<String>,
}

pub const ALL_DAYS: u8 = 0b111_1111;

fn all_days() -> u8 {
    ALL_DAYS
}

/// Day of the week (0 = Monday) from a day number since 0001-01-01 (`num_days_from_ce`, 1 = Monday).
pub fn weekday_of(days_from_ce: i64) -> u32 {
    (days_from_ce - 1).rem_euclid(7) as u32
}

impl Entry {
    pub fn on_day(&self, weekday: u32) -> bool {
        self.days & (1 << (weekday % 7)) != 0
    }
}

/// Accepts "22:30" and also 12-hour forms such as "10:30 PM", "10:30pm" or "10 pm".
pub fn parse_time(s: &str) -> Option<u32> {
    let t = s.trim().to_ascii_lowercase();
    let t = t.replace('.', "");
    let (body, pm) = if let Some(b) = t.strip_suffix("pm") {
        (b.trim(), Some(true))
    } else if let Some(b) = t.strip_suffix("am") {
        (b.trim(), Some(false))
    } else {
        (t.as_str(), None)
    };
    let (h, m) = match body.split_once(':') {
        Some((h, m)) => (h.trim().parse::<u32>().ok()?, m.trim().parse::<u32>().ok()?),
        None if pm.is_some() => (body.parse::<u32>().ok()?, 0),
        None => return None,
    };
    if m >= 60 {
        return None;
    }
    let h = match pm {
        Some(pm) => {
            if h == 0 || h > 12 {
                return None;
            }
            (h % 12) + if pm { 12 } else { 0 }
        }
        None => h,
    };
    (h < 24).then_some(h * 60 + m)
}

/// Text shown to the user: "22:30" or, with `h12`, "10:30 PM".
pub fn label(minutes: u32, h12: bool) -> String {
    if !h12 {
        return format_time(minutes);
    }
    let (h, m) = (minutes / 60, minutes % 60);
    let suffix = if h >= 12 { "PM" } else { "AM" };
    let h = match h % 12 {
        0 => 12,
        x => x,
    };
    format!("{h}:{m:02} {suffix}")
}

/// `label` for a stored "HH:MM" string (returned as is when it cannot be parsed).
pub fn label_str(stored: &str, h12: bool) -> String {
    parse_time(stored).map(|m| label(m, h12)).unwrap_or_else(|| stored.to_string())
}

pub fn format_time(minutes: u32) -> String {
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

/// Normalizes "7:5" -> "07:05"; `None` if invalid.
pub fn normalize(s: &str) -> Option<String> {
    parse_time(s).map(format_time)
}

/// Absolute minutes (days since a fixed epoch * 1440 + minute of the day).
pub fn abs_minutes(days: i64, minute_of_day: u32) -> i64 {
    days * 1440 + minute_of_day as i64
}

/// Entry that fired in the interval (last, now], limited to 3 h so old events
/// are not reapplied after big clock jumps.
pub fn fired<'a>(entries: &'a [Entry], last: i64, now: i64) -> Option<&'a Entry> {
    if now <= last {
        return None;
    }
    let from = last.max(now - 180);
    let mut found: Option<(i64, &Entry)> = None;
    for t in (from + 1)..=now {
        let mod_day = t.rem_euclid(1440) as u32;
        let wd = weekday_of(t.div_euclid(1440));
        for e in entries.iter().filter(|e| e.enabled && e.on_day(wd)) {
            if parse_time(&e.time) == Some(mod_day) {
                found = Some((t, e));
            }
        }
    }
    found.map(|(_, e)| e)
}

/// Entry with an end time that ended in the interval (last, now].
pub fn ended<'a>(entries: &'a [Entry], last: i64, now: i64) -> Option<&'a Entry> {
    if now <= last {
        return None;
    }
    let from = last.max(now - 180);
    let mut found = None;
    for t in (from + 1)..=now {
        let mod_day = t.rem_euclid(1440) as u32;
        let wd = weekday_of(t.div_euclid(1440));
        for e in entries.iter().filter(|e| e.enabled && e.on_day(wd)) {
            if e.end.as_deref().and_then(parse_time) == Some(mod_day) {
                found = Some(e);
            }
        }
    }
    found
}

/// Latest enabled entry that should be in effect now: the most recent up to `minute_of_day`
/// today or, if there is none, the last one from one of the previous 6 days.
pub fn current<'a>(entries: &'a [Entry], minute_of_day: u32, weekday: u32) -> Option<&'a Entry> {
    for back in 0..7u32 {
        let wd = (weekday + 7 - back) % 7;
        let mut list: Vec<(u32, &Entry)> = entries
            .iter()
            .filter(|e| e.enabled && e.on_day(wd))
            .filter_map(|e| parse_time(&e.time).map(|m| (m, e)))
            .filter(|(m, _)| back > 0 || *m <= minute_of_day)
            .filter(|(m, e)| match e.end.as_deref().and_then(parse_time) {
                Some(end) if end > *m => back == 0 && minute_of_day < end, // already ended
                _ => true,
            })
            .collect();
        list.sort_by_key(|(m, _)| *m);
        if let Some((_, e)) = list.last() {
            return Some(*e);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(t: &str, p: &str, en: bool) -> Entry {
        Entry { time: t.into(), profile: p.into(), enabled: en, days: ALL_DAYS, end: None }
    }

    #[test]
    fn times() {
        assert_eq!(parse_time("22:30"), Some(1350));
        assert_eq!(parse_time("7:05"), Some(425));
        assert_eq!(parse_time("24:00"), None);
        assert_eq!(parse_time("12:60"), None);
        assert_eq!(normalize("7:5").as_deref(), Some("07:05"));
        assert_eq!(parse_time("abc"), None);
    }

    #[test]
    fn fires_once_in_window() {
        let v = vec![e("08:00", "Dia", true), e("22:00", "Noite", true), e("23:00", "Off", false)];
        let day = 20_000;
        let a = abs_minutes(day, 7 * 60 + 59);
        let b = abs_minutes(day, 8 * 60);
        assert_eq!(fired(&v, a, b).map(|x| x.profile.as_str()), Some("Dia"));
        assert!(fired(&v, b, b + 1).is_none());
        assert!(fired(&v, abs_minutes(day, 22 * 60 + 59), abs_minutes(day, 23 * 60)).is_none()); // disabled
        // day rollover
        let v2 = vec![e("00:00", "Madrugada", true)];
        assert!(fired(&v2, abs_minutes(day, 23 * 60 + 59), abs_minutes(day + 1, 0)).is_some());
    }

    #[test]
    fn current_wraps() {
        let v = vec![e("08:00", "Dia", true), e("22:00", "Noite", true)];
        assert_eq!(current(&v, 12 * 60, 2).unwrap().profile, "Dia");
        assert_eq!(current(&v, 23 * 60, 2).unwrap().profile, "Noite");
        assert_eq!(current(&v, 3 * 60, 2).unwrap().profile, "Noite"); // before the first one: yesterday's applies
        assert!(current(&[], 100, 0).is_none());
    }

    #[test]
    fn end_time() {
        let mut a = e("08:00", "Dia", true);
        a.end = Some("10:00".into());
        let v = vec![a];
        let day = 20_000;
        assert!(ended(&v, abs_minutes(day, 599), abs_minutes(day, 600)).is_some());
        assert!(current(&v, 9 * 60, 2).is_some());
        assert!(current(&v, 11 * 60, 2).is_none()); // ended and there is no entry from yesterday in effect
    }

    #[test]
    fn weekdays_filter() {
        // day 1 (0001-01-01) is a Monday
        assert_eq!(weekday_of(1), 0);
        assert_eq!(weekday_of(7), 6);
        let mut a = e("08:00", "Util", true);
        a.days = 0b001_1111; // Mon–Fri
        let v = vec![a];
        let sat = 6; // Saturday
        assert!(current(&v, 12 * 60, sat).is_some()); // Friday's entry still applies
        let day_sat = 20_000 - (20_000 - 1) % 7 + 5; // a Saturday
        assert_eq!(weekday_of(day_sat), 5);
        assert!(fired(&v, abs_minutes(day_sat, 479), abs_minutes(day_sat, 480)).is_none());
        let day_mon = day_sat + 2;
        assert!(fired(&v, abs_minutes(day_mon, 479), abs_minutes(day_mon, 480)).is_some());
    }

    #[test]
    fn twelve_hour_clock() {
        assert_eq!(parse_time("10:30 PM"), Some(22 * 60 + 30));
        assert_eq!(parse_time("10:30pm"), Some(22 * 60 + 30));
        assert_eq!(parse_time("12:05 AM"), Some(5));
        assert_eq!(parse_time("12 pm"), Some(12 * 60));
        assert_eq!(parse_time("13:00 PM"), None);
        assert_eq!(parse_time("22:30"), Some(22 * 60 + 30));
        assert_eq!(label(22 * 60 + 30, true), "10:30 PM");
        assert_eq!(label(0, true), "12:00 AM");
        assert_eq!(label(12 * 60, true), "12:00 PM");
        assert_eq!(label(22 * 60 + 30, false), "22:30");
        assert_eq!(label_str("07:05", true), "7:05 AM");
    }
}
