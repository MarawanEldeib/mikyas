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

/// Current wall-clock time. Only the app shell and the capture shim should call this;
/// engine functions take `now_ms` as a parameter.
pub fn now_ms() -> Ms {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as Ms)
        .unwrap_or(0)
}

/// Parses an RFC 3339 / ISO 8601 timestamp such as `2026-09-24T12:34:56.789Z` into epoch ms.
pub fn parse_rfc3339_ms(s: &str) -> Option<Ms> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.timestamp_millis())
}

/// Interprets a JSON value that may be epoch seconds, epoch milliseconds, or an RFC 3339 string.
/// Numbers below 10^11 are treated as seconds (10^11 s is year 5138).
pub fn json_time_to_ms(v: &serde_json::Value) -> Option<Ms> {
    match v {
        serde_json::Value::Number(n) => {
            let f = n.as_f64()?;
            if !f.is_finite() || f <= 0.0 {
                return None;
            }
            if f < 1e11 {
                Some((f * 1000.0) as Ms)
            } else {
                Some(f as Ms)
            }
        }
        serde_json::Value::String(s) => parse_rfc3339_ms(s),
        _ => None,
    }
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
        assert_eq!(json_time_to_ms(&serde_json::json!(1_790_000_000_123_i64)), Some(1_790_000_000_123));
        assert_eq!(json_time_to_ms(&serde_json::json!("1970-01-01T00:00:02Z")), Some(2_000));
        assert_eq!(json_time_to_ms(&serde_json::json!(null)), None);
        assert_eq!(json_time_to_ms(&serde_json::json!(-5)), None);
    }
}
