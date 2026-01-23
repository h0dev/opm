use chrono::{DateTime, Utc};
use colored::Colorize;
use core::fmt;
use once_cell::sync::Lazy;
use regex::Regex;

pub static SUCCESS: Lazy<colored::ColoredString> = Lazy::new(|| "[OPM]".green());
pub static FAIL: Lazy<colored::ColoredString> = Lazy::new(|| "[OPM]".red());
pub static WARN: Lazy<colored::ColoredString> = Lazy::new(|| "[OPM]".yellow());
pub static INFO: Lazy<colored::ColoredString> = Lazy::new(|| "[OPM]".cyan());
pub static WARN_STAR: Lazy<colored::ColoredString> = Lazy::new(|| "*".yellow());

// Time constants for duration formatting
const SECONDS_IN_YEAR: i64 = 365 * 24 * 60 * 60; // 31536000 seconds
const SECONDS_IN_DAY: i64 = 24 * 60 * 60; // 86400 seconds
const SECONDS_IN_HOUR: i64 = 60 * 60; // 3600 seconds
const SECONDS_IN_MINUTE: i64 = 60;

#[derive(Clone, Debug)]
pub struct ColoredString(pub colored::ColoredString);

impl serde::Serialize for ColoredString {
    fn serialize<S: serde::ser::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let re = Regex::new(r"\x1B\[([0-9;]+)m").unwrap();
        let colored_string = &self.0;
        let stripped_string = re.replace_all(colored_string, "").to_string();
        serializer.serialize_str(&stripped_string)
    }
}

impl From<colored::ColoredString> for ColoredString {
    fn from(cs: colored::ColoredString) -> Self {
        ColoredString(cs)
    }
}

impl fmt::Display for ColoredString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

pub fn format_duration(datetime: DateTime<Utc>) -> String {
    let current_time = Utc::now();
    let duration = current_time.signed_duration_since(datetime);

    match duration.num_seconds() {
        s if s >= SECONDS_IN_YEAR => format!("{}y", s / SECONDS_IN_YEAR),
        s if s >= SECONDS_IN_DAY => format!("{}d", s / SECONDS_IN_DAY),
        s if s >= SECONDS_IN_HOUR => format!("{}h", s / SECONDS_IN_HOUR),
        s if s >= SECONDS_IN_MINUTE => format!("{}m", s / SECONDS_IN_MINUTE),
        s => format!("{}s", s),
    }
}

pub fn format_memory(bytes: u64) -> String {
    const UNIT: f64 = 1024.0;
    const SUFFIX: [&str; 4] = ["b", "kb", "mb", "gb"];

    let size = bytes as f64;
    let base = size.log10() / UNIT.log10();

    if size <= 0.0 {
        return "0b".to_string();
    }

    let mut buffer = ryu::Buffer::new();
    let result = buffer
        .format((UNIT.powf(base - base.floor()) * 10.0).round() / 10.0)
        .trim_end_matches(".0");

    [result, SUFFIX[base.floor() as usize]].join("")
}

/// Parse memory string like "100M", "1G", "500K" to bytes
pub fn parse_memory(mem_str: &str) -> Result<u64, String> {
    let mem_str = mem_str.trim().to_uppercase();
    let re = Regex::new(r"^(\d+(?:\.\d+)?)\s*([KMGT]?)B?$").unwrap();

    match re.captures(&mem_str) {
        Some(caps) => {
            let num_str = &caps[1];
            let num: f64 = num_str
                .parse()
                .map_err(|_| format!("Invalid number format: {}", num_str))?;
            let unit = caps.get(2).map_or("", |m| m.as_str());

            let multiplier: u64 = match unit {
                "" | "B" => 1,
                "K" => 1024,
                "M" => 1024 * 1024,
                "G" => 1024 * 1024 * 1024,
                "T" => 1024_u64.pow(4),
                _ => return Err(format!("Unknown unit: {}", unit)),
            };

            let result = num * multiplier as f64;
            // Check for overflow before casting to u64
            if result > u64::MAX as f64 || result < 0.0 {
                return Err(format!("Memory value too large: {}{}", num, unit));
            }

            Ok(result as u64)
        }
        None => Err(format!(
            "Invalid memory format: {}. Use format like '100M', '1G', '500K'",
            mem_str
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn test_format_duration_seconds() {
        let now = Utc::now();
        let datetime = now - Duration::seconds(30);
        assert_eq!(format_duration(datetime), "30s");
    }

    #[test]
    fn test_format_duration_minutes() {
        let now = Utc::now();
        let datetime = now - Duration::minutes(5);
        assert_eq!(format_duration(datetime), "5m");
    }

    #[test]
    fn test_format_duration_hours() {
        let now = Utc::now();
        let datetime = now - Duration::hours(3);
        assert_eq!(format_duration(datetime), "3h");
    }

    #[test]
    fn test_format_duration_days() {
        let now = Utc::now();
        let datetime = now - Duration::days(10);
        assert_eq!(format_duration(datetime), "10d");
    }

    #[test]
    fn test_format_duration_years() {
        let now = Utc::now();
        // 365 days should show as 1 year
        let datetime = now - Duration::days(365);
        assert_eq!(format_duration(datetime), "1y");
    }

    #[test]
    fn test_format_duration_multiple_years() {
        let now = Utc::now();
        // 730 days (2 years) should show as 2y
        let datetime = now - Duration::days(730);
        assert_eq!(format_duration(datetime), "2y");
    }

    #[test]
    fn test_format_duration_just_under_year() {
        let now = Utc::now();
        // 364 days should still show as days
        let datetime = now - Duration::days(364);
        assert_eq!(format_duration(datetime), "364d");
    }

    #[test]
    fn test_format_memory_bytes() {
        assert_eq!(format_memory(0), "0b");
        assert_eq!(format_memory(500), "500b");
    }

    #[test]
    fn test_format_memory_kilobytes() {
        assert_eq!(format_memory(1024), "1kb");
        assert_eq!(format_memory(2048), "2kb");
    }

    #[test]
    fn test_format_memory_megabytes() {
        assert_eq!(format_memory(1024 * 1024), "1mb");
        assert_eq!(format_memory(1024 * 1024 * 5), "5mb");
    }

    #[test]
    fn test_format_memory_gigabytes() {
        assert_eq!(format_memory(1024 * 1024 * 1024), "1gb");
        assert_eq!(format_memory(1024 * 1024 * 1024 * 2), "2gb");
    }

    #[test]
    fn test_parse_memory_bytes() {
        assert_eq!(parse_memory("100").unwrap(), 100);
        assert_eq!(parse_memory("100B").unwrap(), 100);
    }

    #[test]
    fn test_parse_memory_kilobytes() {
        assert_eq!(parse_memory("1K").unwrap(), 1024);
        assert_eq!(parse_memory("2KB").unwrap(), 2048);
    }

    #[test]
    fn test_parse_memory_megabytes() {
        assert_eq!(parse_memory("1M").unwrap(), 1024 * 1024);
        assert_eq!(parse_memory("5MB").unwrap(), 5 * 1024 * 1024);
    }

    #[test]
    fn test_parse_memory_gigabytes() {
        assert_eq!(parse_memory("1G").unwrap(), 1024 * 1024 * 1024);
        assert_eq!(parse_memory("2GB").unwrap(), 2 * 1024 * 1024 * 1024);
    }

    #[test]
    fn test_parse_memory_case_insensitive() {
        assert_eq!(parse_memory("100m").unwrap(), 100 * 1024 * 1024);
        assert_eq!(parse_memory("100M").unwrap(), 100 * 1024 * 1024);
        assert_eq!(parse_memory("100mb").unwrap(), 100 * 1024 * 1024);
        assert_eq!(parse_memory("100MB").unwrap(), 100 * 1024 * 1024);
    }

    #[test]
    fn test_parse_memory_with_spaces() {
        assert_eq!(parse_memory(" 100M ").unwrap(), 100 * 1024 * 1024);
        assert_eq!(parse_memory("100 M").unwrap(), 100 * 1024 * 1024);
    }

    #[test]
    fn test_parse_memory_invalid_format() {
        assert!(parse_memory("invalid").is_err());
        assert!(parse_memory("100X").is_err());
        assert!(parse_memory("").is_err());
    }
}
