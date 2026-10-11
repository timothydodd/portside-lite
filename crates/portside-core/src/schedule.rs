//! Schedules for jobs this PC runs itself (copying backups, manifest
//! backups). They're the cron forms the UI's schedule picker produces, in the
//! PC's local time: every N hours, daily, weekly or monthly at a time. A run
//! is due when the most recent scheduled time has passed since the last
//! success, so a run missed while the PC was off happens at the next start.

use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Every {
    /// At `minute` past every hour divisible by `hours` (1 = every hour).
    Hours { hours: u32, minute: u32 },
    Daily { hour: u32, minute: u32 },
    /// `weekday`: 0 = Sunday.
    Weekly { weekday: u32, hour: u32, minute: u32 },
    /// `day`: 1–28, so every month has it.
    Monthly { day: u32, hour: u32, minute: u32 },
}

fn num(s: &str, max: u32) -> Option<u32> {
    s.parse::<u32>().ok().filter(|n| *n <= max && s.bytes().all(|b| b.is_ascii_digit()))
}

/// The picker's forms; anything else (a custom expression) is `None`.
pub fn parse(cron: &str) -> Option<Every> {
    let expanded = match cron.trim() {
        "@hourly" => "0 * * * *",
        "@daily" | "@midnight" => "0 0 * * *",
        "@weekly" => "0 0 * * 0",
        "@monthly" => "0 0 1 * *",
        other => other,
    };
    let f: Vec<&str> = expanded.split_whitespace().collect();
    let [mi, ho, dom, mon, dow] = f.as_slice() else { return None };
    let minute = num(mi, 59)?;
    if *mon != "*" {
        return None;
    }
    let step = if *ho == "*" { Some(1) } else { ho.strip_prefix("*/").and_then(|h| num(h, 23)) };
    if let Some(hours) = step.filter(|h| *h >= 1) {
        return (*dom == "*" && *dow == "*").then_some(Every::Hours { hours, minute });
    }
    let hour = num(ho, 23)?;
    match (*dom, *dow) {
        ("*", "*") => Some(Every::Daily { hour, minute }),
        ("*", d) => Some(Every::Weekly { weekday: num(d, 7)? % 7, hour, minute }),
        (d, "*") => num(d, 28).filter(|d| *d >= 1).map(|day| Every::Monthly { day, hour, minute }),
        _ => None,
    }
}

fn at(date: NaiveDate, hour: u32, minute: u32) -> NaiveDateTime {
    date.and_time(NaiveTime::from_hms_opt(hour, minute, 0).expect("hour and minute are in range"))
}

/// The most recent scheduled time at or before `now` (local time).
pub fn last_due(every: Every, now: NaiveDateTime) -> NaiveDateTime {
    let today = now.date();
    match every {
        Every::Hours { hours, minute } => {
            // Hours in a day that are multiples of `hours`, latest first, today then yesterday.
            for day in [today, today - Duration::days(1)] {
                let mut h = 23 - 23 % hours;
                loop {
                    let t = at(day, h, minute);
                    if t <= now {
                        return t;
                    }
                    if h < hours {
                        break;
                    }
                    h -= hours;
                }
            }
            unreachable!("yesterday always has a scheduled hour")
        }
        Every::Daily { hour, minute } => {
            let t = at(today, hour, minute);
            if t <= now { t } else { t - Duration::days(1) }
        }
        Every::Weekly { weekday, hour, minute } => {
            let back = (7 + today.weekday().num_days_from_sunday() - weekday) % 7;
            let t = at(today - Duration::days(back as i64), hour, minute);
            if t <= now { t } else { t - Duration::days(7) }
        }
        Every::Monthly { day, hour, minute } => {
            let this = at(today.with_day(day).expect("day 1-28 exists in every month"), hour, minute);
            if this <= now {
                return this;
            }
            let (y, m) = if today.month() == 1 { (today.year() - 1, 12) } else { (today.year(), today.month() - 1) };
            at(NaiveDate::from_ymd_opt(y, m, day).expect("day 1-28 exists in every month"), hour, minute)
        }
    }
}

/// Roughly how often it runs.
pub fn interval_hours(every: Every) -> u32 {
    match every {
        Every::Hours { hours, .. } => hours,
        Every::Daily { .. } => 24,
        Every::Weekly { .. } => 7 * 24,
        Every::Monthly { .. } => 31 * 24,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap()
    }

    #[test]
    fn parses_the_pickers_forms() {
        assert_eq!(parse("15 */6 * * *"), Some(Every::Hours { hours: 6, minute: 15 }));
        assert_eq!(parse("0 * * * *"), Some(Every::Hours { hours: 1, minute: 0 }));
        assert_eq!(parse("30 2 * * *"), Some(Every::Daily { hour: 2, minute: 30 }));
        assert_eq!(parse("0 3 * * 7"), Some(Every::Weekly { weekday: 0, hour: 3, minute: 0 }));
        assert_eq!(parse("0 3 15 * *"), Some(Every::Monthly { day: 15, hour: 3, minute: 0 }));
        assert_eq!(parse("@daily"), Some(Every::Daily { hour: 0, minute: 0 }));
        for custom in ["0 3 31 * *", "0 3 * 1 *", "0 3 1 * 1", "0 1-5 * * *", "nonsense", ""] {
            assert_eq!(parse(custom), None, "{custom}");
        }
    }

    #[test]
    fn last_due_times() {
        // 2026-10-11 is a Sunday.
        let now = t("2026-10-11 10:20");
        assert_eq!(last_due(Every::Daily { hour: 2, minute: 0 }, now), t("2026-10-11 02:00"));
        assert_eq!(last_due(Every::Daily { hour: 23, minute: 0 }, now), t("2026-10-10 23:00"));
        assert_eq!(last_due(Every::Hours { hours: 6, minute: 30 }, now), t("2026-10-11 06:30"));
        assert_eq!(last_due(Every::Hours { hours: 6, minute: 30 }, t("2026-10-11 00:10")), t("2026-10-10 18:30"));
        assert_eq!(last_due(Every::Hours { hours: 1, minute: 0 }, now), t("2026-10-11 10:00"));
        assert_eq!(last_due(Every::Weekly { weekday: 0, hour: 3, minute: 0 }, now), t("2026-10-11 03:00"));
        assert_eq!(last_due(Every::Weekly { weekday: 0, hour: 12, minute: 0 }, now), t("2026-10-04 12:00"));
        assert_eq!(last_due(Every::Weekly { weekday: 5, hour: 3, minute: 0 }, now), t("2026-10-09 03:00"));
        assert_eq!(last_due(Every::Monthly { day: 1, hour: 3, minute: 0 }, now), t("2026-10-01 03:00"));
        assert_eq!(last_due(Every::Monthly { day: 20, hour: 3, minute: 0 }, now), t("2026-09-20 03:00"));
        assert_eq!(last_due(Every::Monthly { day: 20, hour: 3, minute: 0 }, t("2026-01-05 00:00")), t("2025-12-20 03:00"));
        assert_eq!(last_due(Every::Daily { hour: 10, minute: 20 }, now), now, "a time exactly now is due");
    }
}
