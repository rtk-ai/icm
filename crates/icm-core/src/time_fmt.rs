//! Display-side timestamp formatting that respects the user's timezone.
//!
//! Storage stays in UTC (`DateTime<Utc>`) — that's the right canonical form
//! for ordering, cross-machine comparison, and durable serialization. But
//! when timestamps are shown to the user (CLI tables, MCP `icm_stats` /
//! `icm_recall` output, dashboards), they should appear in the local
//! timezone the user actually lives in. Issue #119 reported that all
//! timestamps were rendered in UTC even when `TZ=Asia/Bangkok` was set.
//!
//! `chrono::Local` honours the `TZ` environment variable on Unix (via libc)
//! and the system locale on Windows, so converting once here at the display
//! boundary is enough.

use chrono::{DateTime, Local, Utc};

/// Format a UTC timestamp in the user's local timezone.
///
/// Pass any `chrono::format::strftime` pattern, e.g. `"%Y-%m-%d %H:%M"`.
/// The output reflects `TZ` (Unix) or the system locale (Windows). For
/// machine-readable output (JSON APIs, RFC 3339 in stored fields), keep
/// using `to_rfc3339` on the UTC value directly — that path is unaffected.
pub fn format_local(dt: &DateTime<Utc>, fmt: &str) -> String {
    dt.with_timezone(&Local).format(fmt).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    /// Sanity: format_local must produce a non-empty string for any input.
    #[test]
    fn format_local_returns_non_empty() {
        let utc = Utc.with_ymd_and_hms(2026, 5, 10, 12, 0, 0).unwrap();
        let s = format_local(&utc, "%Y-%m-%d %H:%M");
        assert!(!s.is_empty());
        // Must look like a date-time of the requested shape.
        assert!(
            s.len() >= "2026-05-10 12:00".len(),
            "expected date-time-shaped string, got {s:?}"
        );
    }

    /// Issue #119: when TZ shifts the local clock, the formatted string
    /// must change. We can't force chrono::Local to a specific TZ from
    /// safe Rust without setting env vars (which is process-global), so
    /// instead we round-trip through the same conversion chrono::Local
    /// uses internally and assert the helper agrees with it.
    #[test]
    fn format_local_agrees_with_with_timezone_local() {
        let utc = Utc.with_ymd_and_hms(2026, 5, 10, 12, 34, 56).unwrap();
        let direct = utc.with_timezone(&Local).format("%H:%M:%S").to_string();
        assert_eq!(format_local(&utc, "%H:%M:%S"), direct);
    }
}
