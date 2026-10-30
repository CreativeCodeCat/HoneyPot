//! Small helpers used across the bot: timestamps and text formatting.

use chrono::{DateTime, NaiveDateTime, Utc};

pub fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, false)
}

/// Parses a timestamp written by `now_iso` or by Python's `datetime.isoformat()`.
/// Timestamps without an offset are treated as UTC.
pub fn parse_iso(timestamp: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(timestamp)
        .map(|dt| dt.with_timezone(&Utc))
        .or_else(|_| {
            NaiveDateTime::parse_from_str(timestamp, "%Y-%m-%dT%H:%M:%S%.f").map(|dt| dt.and_utc())
        })
        .ok()
}

pub fn format_time(timestamp: &str) -> String {
    parse_iso(timestamp)
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|| timestamp.to_string())
}

/// Equivalent of Python's `str.title()`.
pub fn title_case(text: &str) -> String {
    let mut prev_alpha = false;
    text.chars()
        .map(|c| {
            let out = if prev_alpha {
                c.to_lowercase().to_string()
            } else {
                c.to_uppercase().to_string()
            };
            prev_alpha = c.is_alphabetic();
            out
        })
        .collect()
}

/// Truncates to at most `max` characters (Discord limits are in characters).
pub fn truncate(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers_match_python() {
        assert_eq!(
            title_case("user_banned".replace('_', " ").as_str()),
            "User Banned"
        );
        assert_eq!(title_case("flagged code-blocks"), "Flagged Code-Blocks");
        assert_eq!(
            format_time("2026-09-30T13:26:35.965988+00:00"),
            "2026-09-30 13:26:35"
        );
        assert_eq!(
            format_time("2026-09-30T13:26:35.965988"),
            "2026-09-30 13:26:35"
        );
        assert!(parse_iso(&now_iso()).is_some());
        assert_eq!(crate::features::normalize_cog_name("Server Stats"), "serverstats");
    }
}
