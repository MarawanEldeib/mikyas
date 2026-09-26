//! Epoch-millisecond helpers. Everything in the engine is expressed in UTC epoch ms
//! so that logic stays pure and testable (callers pass `now_ms` explicitly).

use std::time::{SystemTime, UNIX_EPOCH};

/// UTC epoch milliseconds.
pub type Ms = i64;

pub const SECOND_MS: Ms = 1_000;
pub const MINUTE_MS: Ms = 60 * SECOND_MS;
pub const HOUR_MS: Ms = 60 * MINUTE_MS;
pub const DAY_MS: Ms = 24 * HOUR_MS;
pub const FIVE_HOURS_MS: Ms = 5 * HOUR_MS;
pub const SEVEN_DAYS_MS: Ms = 7 * DAY_MS;
/// 2001-01-01T00:00:00Z. An epoch number before this is a corrupt value, not a real time.
pub const MIN_PLAUSIBLE_MS: Ms = 978_307_200_000;
/// 2200-01-01T00:00:00Z. An epoch number after this is a corrupt value, not a real time.
pub const MAX_PLAUSIBLE_MS: Ms = 7_258_118_400_000;
/// Samples, captures and turns dated further in the future than this (clock skew, corrupt files)
/// are ignored.
pub const FUTURE_SLACK_MS: Ms = 5 * MINUTE_MS;

/// True for a time within [`MIN_PLAUSIBLE_MS`]..=[`MAX_PLAUSIBLE_MS`]; anything else is corrupt.
pub fn plausible_ms(ms: Ms) -> bool {
    (MIN_PLAUSIBLE_MS..=MAX_PLAUSIBLE_MS).contains(&ms)
}

/// A file-system time in epoch ms: `None` before 1970, saturating at [`Ms::MAX`].
pub fn system_time_ms(t: SystemTime) -> Option<Ms> {
    let d = t.duration_since(UNIX_EPOCH).ok()?;
    Some(Ms::try_from(d.as_millis()).unwrap_or(Ms::MAX))
}

/// Current wall-clock time. Only the app shell and the capture shim should call this;
/// engine functions take `now_ms` as a parameter.
pub fn now_ms() -> Ms {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as Ms).unwrap_or(0)
}

/// Parses an RFC 3339 / ISO 8601 timestamp such as `2026-09-24T12:34:56.789Z` into epoch ms.
pub fn parse_rfc3339_ms(s: &str) -> Option<Ms> {
    chrono::DateTime::parse_from_rfc3339(s).ok().map(|dt| dt.timestamp_millis())
}

/// Interprets a JSON value that may be epoch seconds, epoch milliseconds, or an RFC 3339 string.
/// Numbers below 10^11 are treated as seconds (10^11 s is year 5138). A time that lands outside
/// [`MIN_PLAUSIBLE_MS`]..=[`MAX_PLAUSIBLE_MS`] ([`plausible_ms`]) is corrupt (a tiny number would
/// otherwise become a 1970 timestamp) and yields `None`, for numbers and strings alike.
pub fn json_time_to_ms(v: &serde_json::Value) -> Option<Ms> {
    let ms = match v {
        serde_json::Value::Number(n) => {
            let f = n.as_f64()?;
            let ms = if f < 1e11 { f * 1000.0 } else { f };
            // `as` saturates (NaN becomes 0), so every out-of-range value fails the check below.
            ms as Ms
        }
        serde_json::Value::String(s) => parse_rfc3339_ms(s)?,
        _ => return None,
    };
    Some(ms).filter(|&ms| plausible_ms(ms))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rfc3339() {
        assert_eq!(parse_rfc3339_ms("1970-01-01T00:00:01.500Z"), Some(1_500));
        // 2026-09-24T00:00:00Z is 1_790_208_000 s; +02:00 is two hours earlier in UTC.
        assert_eq!(parse_rfc3339_ms("2026-09-24T00:00:00+02:00"), Some(1_790_208_000_000 - 2 * HOUR_MS));
        assert_eq!(parse_rfc3339_ms("not a date"), None);
    }

    #[test]
    fn json_time_units() {
        assert_eq!(json_time_to_ms(&serde_json::json!(1_790_000_000)), Some(1_790_000_000_000));
        assert_eq!(json_time_to_ms(&serde_json::json!(1_790_000_000.5)), Some(1_790_000_000_500));
        assert_eq!(json_time_to_ms(&serde_json::json!(1_790_000_000_123_i64)), Some(1_790_000_000_123));
        assert_eq!(json_time_to_ms(&serde_json::json!("2026-09-24T00:00:02Z")), Some(1_790_208_002_000));
        // Strings are range-checked like numbers.
        assert_eq!(json_time_to_ms(&serde_json::json!("1970-01-01T00:00:02Z")), None);
        assert_eq!(json_time_to_ms(&serde_json::json!("2200-01-01T00:00:00.001Z")), None);
        assert_eq!(json_time_to_ms(&serde_json::json!("2200-01-01T00:00:00Z")), Some(MAX_PLAUSIBLE_MS));
        assert_eq!(json_time_to_ms(&serde_json::json!(null)), None);
        assert_eq!(json_time_to_ms(&serde_json::json!(-5)), None);
    }

    #[test]
    fn plausible_range_is_2001_to_2200() {
        assert_eq!(parse_rfc3339_ms("2001-01-01T00:00:00Z"), Some(MIN_PLAUSIBLE_MS));
        assert_eq!(parse_rfc3339_ms("2200-01-01T00:00:00Z"), Some(MAX_PLAUSIBLE_MS));
    }

    #[test]
    fn json_time_rejects_implausible_epoch_numbers() {
        use serde_json::json;
        for corrupt in [
            json!(0.5),
            json!(1),
            json!(86_400),
            json!(978_307_199),
            json!(978_307_199_999_i64),
            json!(7_258_118_401_i64),
            // Below 10^11, so read as seconds: year 5138.
            json!(99_999_999_999_i64),
            // Read as milliseconds: 1973.
            json!(100_000_000_000_i64),
            json!(7_258_118_400_001_i64),
            json!(u64::MAX),
            json!(1e300),
        ] {
            assert_eq!(json_time_to_ms(&corrupt), None, "{corrupt}");
        }
        assert_eq!(json_time_to_ms(&json!(978_307_200)), Some(MIN_PLAUSIBLE_MS));
        assert_eq!(json_time_to_ms(&json!(978_307_200_000_i64)), Some(MIN_PLAUSIBLE_MS));
        assert_eq!(json_time_to_ms(&json!(7_258_118_400_i64)), Some(MAX_PLAUSIBLE_MS));
        assert_eq!(json_time_to_ms(&json!(7_258_118_400_000_i64)), Some(MAX_PLAUSIBLE_MS));
        assert!(plausible_ms(MIN_PLAUSIBLE_MS) && !plausible_ms(MIN_PLAUSIBLE_MS - 1));
    }

    #[test]
    fn system_times_before_1970_are_none() {
        assert_eq!(system_time_ms(UNIX_EPOCH + std::time::Duration::from_millis(5)), Some(5));
        assert_eq!(system_time_ms(UNIX_EPOCH - std::time::Duration::from_millis(5)), None);
    }
}
