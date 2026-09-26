//! Local-calendar math shared by the History view and the pipeline (weekly recap), generic over
//! the time zone so it is tested with fixed offsets.

use chrono::{NaiveDate, Offset, TimeZone};
use sovawatch_core::time::{HOUR_MS, Ms};

/// The next full hour of the local clock after `now_ms` (half-hour zones included).
pub fn next_local_hour<Tz: TimeZone>(now_ms: Ms, tz: &Tz) -> Ms {
    let offset_ms =
        tz.timestamp_millis_opt(now_ms).single().map_or(0, |dt| i64::from(dt.offset().fix().local_minus_utc()) * 1_000);
    (now_ms + offset_ms).div_euclid(HOUR_MS).saturating_add(1) * HOUR_MS - offset_ms
}

/// Local midnights of every calendar day from the one containing `from_ms` to the one containing
/// `last_ms`, ascending, at most `max_days + 1` of them (a bound against a bogus clock).
pub fn local_day_starts<Tz: TimeZone>(from_ms: Ms, last_ms: Ms, max_days: usize, tz: &Tz) -> Vec<Ms> {
    let date_of = |t: Ms| tz.timestamp_millis_opt(t).single().map(|dt| dt.date_naive());
    let (Some(mut day), Some(last)) = (date_of(from_ms), date_of(last_ms)) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    while day <= last && out.len() <= max_days {
        out.extend(local_midnight(tz, day));
        let Some(next) = day.succ_opt() else { break };
        day = next;
    }
    out
}

/// First instant of a local calendar day. Where midnight does not exist (zones that switch to
/// daylight time at 00:00), the day starts at the first valid hour after it.
pub fn local_midnight<Tz: TimeZone>(tz: &Tz, day: NaiveDate) -> Option<Ms> {
    (0..=2)
        .find_map(|hour| tz.from_local_datetime(&day.and_hms_opt(hour, 0, 0)?).earliest())
        .map(|dt| dt.timestamp_millis())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{FixedOffset, Local, Timelike};
    use sovawatch_core::time::{DAY_MS, MINUTE_MS};

    /// 2026-09-24T10:17:00Z.
    const NOW: Ms = 1_790_208_000_000 + 10 * HOUR_MS + 17 * MINUTE_MS;

    #[test]
    fn half_hour_zones_align_to_the_local_hour() {
        let india = FixedOffset::east_opt(5 * 3600 + 1800).unwrap();
        assert_eq!(next_local_hour(NOW, &india), 1_790_208_000_000 + 10 * HOUR_MS + 30 * MINUTE_MS);
        // 2026-09-24 00:00 in +05:30 is 2026-09-23T18:30Z.
        let starts = local_day_starts(NOW - HOUR_MS, NOW, 14, &india);
        assert_eq!(starts, vec![1_790_208_000_000 - 5 * HOUR_MS - 30 * MINUTE_MS]);
    }

    #[test]
    fn local_day_starts_are_midnights() {
        let starts = local_day_starts(NOW - 14 * DAY_MS, NOW, 14, &Local);
        assert_eq!(starts.len(), 15);
        for w in starts.windows(2) {
            let len = w[1] - w[0];
            assert!((23 * HOUR_MS..=25 * HOUR_MS).contains(&len), "{len}");
        }
        for s in &starts {
            let dt = Local.timestamp_millis_opt(*s).single().unwrap();
            assert_eq!((dt.minute(), dt.second()), (0, 0));
            assert!(dt.hour() <= 2, "midnight, or the first hour after a DST gap");
        }
        assert!(starts[0] <= NOW - 14 * DAY_MS);
    }

    #[test]
    fn the_day_count_is_bounded() {
        let utc = FixedOffset::east_opt(0).unwrap();
        assert_eq!(local_day_starts(NOW - 400 * DAY_MS, NOW, 8, &utc).len(), 9);
        assert!(local_day_starts(NOW, NOW - DAY_MS, 8, &utc).is_empty());
    }
}
