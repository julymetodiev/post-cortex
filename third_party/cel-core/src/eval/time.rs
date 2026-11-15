//! Time parsing and formatting utilities for CEL timestamps and durations.
//!
//! This module provides functions to parse and format timestamps and durations
//! according to CEL specification requirements.

use super::value::{Duration, Timestamp};
use chrono::{DateTime, Datelike, FixedOffset, Offset, TimeZone, Timelike};
use chrono_tz::Tz;

/// Parse an RFC 3339 timestamp string.
///
/// Supports formats like:
/// - "2009-02-13T23:31:30Z"
/// - "2009-02-13T23:31:30.123456789Z"
/// - "2009-02-13T23:31:30+01:00"
pub fn parse_timestamp(s: &str) -> Result<Timestamp, String> {
    // Try parsing with chrono's RFC 3339 parser
    let dt =
        DateTime::parse_from_rfc3339(s).map_err(|e| format!("invalid timestamp format: {}", e))?;

    let ts = Timestamp {
        seconds: dt.timestamp(),
        nanos: dt.timestamp_subsec_nanos() as i32,
    };

    if !ts.is_valid() {
        return Err("timestamp out of range: must be between year 0001 and 9999".to_string());
    }

    Ok(ts)
}

/// Parse a CEL duration string.
///
/// Supports formats like:
/// - "100s" - 100 seconds
/// - "1.5h" - 1.5 hours
/// - "30m" - 30 minutes
/// - "1h30m" - 1 hour 30 minutes
/// - "1h30m45s" - 1 hour 30 minutes 45 seconds
/// - "100ms" - 100 milliseconds
/// - "100us" - 100 microseconds
/// - "100ns" - 100 nanoseconds
/// - "-30s" - negative 30 seconds
pub fn parse_duration(s: &str) -> Result<Duration, String> {
    if s.is_empty() {
        return Err("empty duration string".to_string());
    }

    let (negative, s) = if let Some(rest) = s.strip_prefix('-') {
        (true, rest)
    } else {
        (false, s)
    };

    if s.is_empty() {
        return Err("invalid duration: no value".to_string());
    }

    let mut total_nanos: i128 = 0;
    let mut remaining = s;

    while !remaining.is_empty() {
        // Parse the numeric part (including optional decimal point)
        let num_end = remaining
            .find(|c: char| !c.is_ascii_digit() && c != '.')
            .unwrap_or(remaining.len());

        if num_end == 0 {
            return Err(format!(
                "invalid duration format: expected number at '{}'",
                remaining
            ));
        }

        let num_str = &remaining[..num_end];
        remaining = &remaining[num_end..];

        // Parse the unit
        let unit_end = remaining
            .find(|c: char| c.is_ascii_digit() || c == '.')
            .unwrap_or(remaining.len());

        if unit_end == 0 {
            return Err(format!(
                "invalid duration: missing unit after '{}'",
                num_str
            ));
        }

        let unit = &remaining[..unit_end];
        remaining = &remaining[unit_end..];

        // Convert to nanoseconds based on unit
        let multiplier: i128 = match unit {
            "h" => 3_600_000_000_000,    // hours
            "m" => 60_000_000_000,       // minutes
            "s" => 1_000_000_000,        // seconds
            "ms" => 1_000_000,           // milliseconds
            "us" | "\u{00b5}s" => 1_000, // microseconds (supports μs)
            "ns" => 1,                   // nanoseconds
            _ => return Err(format!("invalid duration unit: '{}'", unit)),
        };

        // Parse the number (may be floating point)
        if num_str.contains('.') {
            let num: f64 = num_str
                .parse()
                .map_err(|_| format!("invalid number in duration: '{}'", num_str))?;
            total_nanos += (num * multiplier as f64) as i128;
        } else {
            let num: i128 = num_str
                .parse()
                .map_err(|_| format!("invalid number in duration: '{}'", num_str))?;
            total_nanos += num * multiplier;
        }
    }

    if negative {
        total_nanos = -total_nanos;
    }

    // Convert to seconds and nanos
    let seconds = (total_nanos / 1_000_000_000) as i64;
    let nanos = (total_nanos % 1_000_000_000) as i32;

    let duration = Duration::new(seconds, nanos);

    if !duration.is_valid() {
        return Err("duration out of range: must be within approximately 10000 years".to_string());
    }

    Ok(duration)
}

/// Format a timestamp as an RFC 3339 string with nanosecond precision.
///
/// Examples:
/// - "2009-02-13T23:31:30Z" (no fractional seconds)
/// - "2009-02-13T23:31:30.123456789Z" (with nanoseconds)
pub fn format_timestamp(ts: &Timestamp) -> String {
    if let Some(dt) = ts.to_datetime_utc() {
        if ts.nanos == 0 {
            dt.format("%Y-%m-%dT%H:%M:%SZ").to_string()
        } else {
            // Format with nanoseconds, trimming trailing zeros
            let nanos_str = format!("{:09}", ts.nanos);
            let trimmed = nanos_str.trim_end_matches('0');
            if trimmed.is_empty() {
                dt.format("%Y-%m-%dT%H:%M:%SZ").to_string()
            } else {
                format!("{}.{}Z", dt.format("%Y-%m-%dT%H:%M:%S"), trimmed)
            }
        }
    } else {
        // Fallback for invalid timestamps
        format!("{}s", ts.seconds)
    }
}

/// Format a duration as a string.
///
/// CEL format: "Xs" or "X.XXXXXXXXXs" for durations with fractional seconds.
pub fn format_duration(d: &Duration) -> String {
    if d.nanos == 0 {
        format!("{}s", d.seconds)
    } else {
        // Format with fractional seconds
        let total_nanos = d.seconds as i128 * 1_000_000_000 + d.nanos as i128;
        let sign = if total_nanos < 0 { "-" } else { "" };
        let abs_nanos = total_nanos.abs();
        let secs = abs_nanos / 1_000_000_000;
        let frac = abs_nanos % 1_000_000_000;

        if frac == 0 {
            format!("{}{}s", sign, secs)
        } else {
            // Format fractional part, trimming trailing zeros
            let frac_str = format!("{:09}", frac);
            let trimmed = frac_str.trim_end_matches('0');
            format!("{}{}.{}s", sign, secs, trimmed)
        }
    }
}

/// Parse a timezone string.
///
/// Supports:
/// - IANA timezone names: "America/New_York", "Europe/London", "Australia/Sydney"
/// - Fixed UTC offsets: "+01:00", "-05:30", "02:00" (positive assumed)
///
/// Returns a FixedOffset that can be used with chrono.
pub fn parse_timezone(tz: &str) -> Result<TimezoneInfo, String> {
    // First, try parsing as an IANA timezone name
    if let Ok(tz_parsed) = tz.parse::<Tz>() {
        return Ok(TimezoneInfo::Iana(tz_parsed));
    }

    // Try parsing as a fixed offset
    parse_fixed_offset(tz).map(TimezoneInfo::Fixed)
}

/// Parse a fixed UTC offset string like "+01:00", "-05:30", or "02:00".
fn parse_fixed_offset(s: &str) -> Result<FixedOffset, String> {
    let s = s.trim();

    if s.is_empty() {
        return Err("empty timezone string".to_string());
    }

    // Determine sign and remaining string
    let (negative, rest) = if let Some(r) = s.strip_prefix('-') {
        (true, r)
    } else if let Some(r) = s.strip_prefix('+') {
        (false, r)
    } else {
        // No sign prefix - assume positive
        (false, s)
    };

    // Parse hours and minutes
    let parts: Vec<&str> = rest.split(':').collect();
    if parts.len() != 2 {
        return Err(format!("invalid timezone offset format: '{}'", s));
    }

    let hours: i32 = parts[0]
        .parse()
        .map_err(|_| format!("invalid hours in timezone: '{}'", parts[0]))?;
    let minutes: i32 = parts[1]
        .parse()
        .map_err(|_| format!("invalid minutes in timezone: '{}'", parts[1]))?;

    let total_seconds = (hours * 3600 + minutes * 60) * if negative { -1 } else { 1 };

    FixedOffset::east_opt(total_seconds)
        .ok_or_else(|| format!("timezone offset out of range: '{}'", s))
}

/// Represents either an IANA timezone or a fixed offset.
pub enum TimezoneInfo {
    Iana(Tz),
    Fixed(FixedOffset),
}

impl TimezoneInfo {
    /// Convert a UTC timestamp to a DateTime in this timezone.
    pub fn datetime_from_timestamp(&self, ts: &Timestamp) -> Option<DateTime<FixedOffset>> {
        let utc_dt = ts.to_datetime_utc()?;

        match self {
            TimezoneInfo::Iana(tz) => {
                let local = utc_dt.with_timezone(tz);
                // Convert to fixed offset
                let offset = local.offset().fix();
                Some(local.with_timezone(&offset))
            }
            TimezoneInfo::Fixed(offset) => Some(utc_dt.with_timezone(offset)),
        }
    }
}

/// Timestamp accessor component.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimestampComponent {
    /// Full 4-digit year.
    FullYear,
    /// Month (0-11, 0 = January).
    Month,
    /// Day of month (1-31, 1-indexed).
    Date,
    /// Day of month (0-30, 0-indexed).
    DayOfMonth,
    /// Day of week (0-6, 0 = Sunday).
    DayOfWeek,
    /// Day of year (0-365).
    DayOfYear,
    /// Hours (0-23).
    Hours,
    /// Minutes (0-59).
    Minutes,
    /// Seconds (0-59).
    Seconds,
    /// Milliseconds (0-999).
    Milliseconds,
}

impl TimestampComponent {
    /// Get the component value from a DateTime.
    pub fn extract<Tz: TimeZone>(&self, dt: &DateTime<Tz>) -> i64 {
        match self {
            TimestampComponent::FullYear => dt.year() as i64,
            TimestampComponent::Month => (dt.month0()) as i64, // 0-11
            TimestampComponent::Date => dt.day() as i64,       // 1-31
            TimestampComponent::DayOfMonth => (dt.day() - 1) as i64, // 0-30
            TimestampComponent::DayOfWeek => {
                // chrono: Mon=0, Sun=6; CEL: Sun=0, Sat=6
                let weekday = dt.weekday().num_days_from_sunday();
                weekday as i64
            }
            TimestampComponent::DayOfYear => (dt.ordinal0()) as i64, // 0-365
            TimestampComponent::Hours => dt.hour() as i64,
            TimestampComponent::Minutes => dt.minute() as i64,
            TimestampComponent::Seconds => dt.second() as i64,
            TimestampComponent::Milliseconds => (dt.nanosecond() / 1_000_000) as i64,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_timestamp_basic() {
        let ts = parse_timestamp("2009-02-13T23:31:30Z").unwrap();
        assert_eq!(ts.seconds, 1234567890);
        assert_eq!(ts.nanos, 0);
    }

    #[test]
    fn test_parse_timestamp_with_nanos() {
        let ts = parse_timestamp("2009-02-13T23:31:30.123456789Z").unwrap();
        assert_eq!(ts.seconds, 1234567890);
        assert_eq!(ts.nanos, 123456789);
    }

    #[test]
    fn test_parse_timestamp_with_offset() {
        let ts = parse_timestamp("2009-02-13T18:31:30-05:00").unwrap();
        assert_eq!(ts.seconds, 1234567890);
    }

    #[test]
    fn test_parse_duration_seconds() {
        let d = parse_duration("100s").unwrap();
        assert_eq!(d.seconds, 100);
        assert_eq!(d.nanos, 0);
    }

    #[test]
    fn test_parse_duration_hours() {
        let d = parse_duration("2h").unwrap();
        assert_eq!(d.seconds, 7200);
    }

    #[test]
    fn test_parse_duration_compound() {
        let d = parse_duration("1h30m").unwrap();
        assert_eq!(d.seconds, 5400);
    }

    #[test]
    fn test_parse_duration_negative() {
        let d = parse_duration("-30s").unwrap();
        assert_eq!(d.seconds, -30);
    }

    #[test]
    fn test_parse_duration_milliseconds() {
        let d = parse_duration("500ms").unwrap();
        assert_eq!(d.seconds, 0);
        assert_eq!(d.nanos, 500_000_000);
    }

    #[test]
    fn test_parse_duration_fractional() {
        let d = parse_duration("1.5h").unwrap();
        assert_eq!(d.seconds, 5400);
    }

    #[test]
    fn test_format_timestamp() {
        let ts = Timestamp::new(1234567890, 0);
        assert_eq!(format_timestamp(&ts), "2009-02-13T23:31:30Z");
    }

    #[test]
    fn test_format_timestamp_with_nanos() {
        let ts = Timestamp::new(1234567890, 123000000);
        assert_eq!(format_timestamp(&ts), "2009-02-13T23:31:30.123Z");
    }

    #[test]
    fn test_format_duration() {
        let d = Duration::new(100, 0);
        assert_eq!(format_duration(&d), "100s");
    }

    #[test]
    fn test_format_duration_with_nanos() {
        let d = Duration::new(1, 500000000);
        assert_eq!(format_duration(&d), "1.5s");
    }

    #[test]
    fn test_parse_timezone_iana() {
        let tz = parse_timezone("America/New_York").unwrap();
        assert!(matches!(tz, TimezoneInfo::Iana(_)));
    }

    #[test]
    fn test_parse_timezone_offset() {
        let tz = parse_timezone("+05:30").unwrap();
        assert!(matches!(tz, TimezoneInfo::Fixed(_)));
    }

    #[test]
    fn test_parse_timezone_offset_no_sign() {
        let tz = parse_timezone("05:30").unwrap();
        assert!(matches!(tz, TimezoneInfo::Fixed(_)));
    }

    #[test]
    fn test_timestamp_component_extract() {
        let ts = Timestamp::new(1234567890, 0);
        let dt = ts.to_datetime_utc().unwrap();

        assert_eq!(TimestampComponent::FullYear.extract(&dt), 2009);
        assert_eq!(TimestampComponent::Month.extract(&dt), 1); // February = 1 (0-indexed)
        assert_eq!(TimestampComponent::Date.extract(&dt), 13);
        assert_eq!(TimestampComponent::DayOfMonth.extract(&dt), 12); // 0-indexed
        assert_eq!(TimestampComponent::Hours.extract(&dt), 23);
        assert_eq!(TimestampComponent::Minutes.extract(&dt), 31);
        assert_eq!(TimestampComponent::Seconds.extract(&dt), 30);
    }

    #[test]
    fn test_day_of_week() {
        // 2009-02-13 was a Friday
        let ts = Timestamp::new(1234567890, 0);
        let dt = ts.to_datetime_utc().unwrap();
        assert_eq!(TimestampComponent::DayOfWeek.extract(&dt), 5); // Friday = 5 (Sun=0)
    }
}
