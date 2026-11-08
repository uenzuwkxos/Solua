// SPDX-License-Identifier: GPL-3.0-or-later
//! Pure helper functions.

/// Splits hours of use into (years, days, hours); year = 365 days.
pub fn duration_parts(hours: u64) -> (u64, u64, u64) {
    let days = hours / 24;
    (days / 365, days % 365, hours % 24)
}

/// Joins the non-zero parts using the given templates, e.g. "1 year, 100 days, 4 hours".
/// Each template contains `%1`; `one`/`many` are singular/plural for (year, day, hour).
pub fn format_duration(hours: u64, one: [&str; 3], many: [&str; 3]) -> String {
    let (y, d, h) = duration_parts(hours);
    let mut parts = Vec::new();
    for (i, n) in [y, d, h].into_iter().enumerate() {
        if n > 0 {
            let tpl = if n == 1 { one[i] } else { many[i] };
            parts.push(tpl.replace("%1", &n.to_string()));
        }
    }
    if parts.is_empty() {
        many[2].replace("%1", "0")
    } else {
        parts.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_hours() {
        assert_eq!(duration_parts(0), (0, 0, 0));
        assert_eq!(duration_parts(23), (0, 0, 23));
        assert_eq!(duration_parts(24 * 365 + 24 * 100 + 4), (1, 100, 4));
        assert_eq!(duration_parts(4210), (0, 175, 10));
    }

    #[test]
    fn formats_with_plurals() {
        let one = ["%1 ano", "%1 dia", "%1 hora"];
        let many = ["%1 anos", "%1 dias", "%1 horas"];
        assert_eq!(format_duration(24 * 365 + 24 * 100 + 4, one, many), "1 ano, 100 dias, 4 horas");
        assert_eq!(format_duration(48, one, many), "2 dias");
        assert_eq!(format_duration(1, one, many), "1 hora");
        assert_eq!(format_duration(0, one, many), "0 horas");
    }
}
