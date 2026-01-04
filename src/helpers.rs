use chrono::{DateTime, Utc};
use colored::Colorize;
use core::fmt;
use once_cell::sync::Lazy;
use regex::Regex;

pub static SUCCESS: Lazy<colored::ColoredString> = Lazy::new(|| "[OPM]".green());
pub static FAIL: Lazy<colored::ColoredString> = Lazy::new(|| "[OPM]".red());
pub static WARN: Lazy<colored::ColoredString> = Lazy::new(|| "[OPM]".yellow());
pub static WARN_STAR: Lazy<colored::ColoredString> = Lazy::new(|| "*".yellow());

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
    fn from(cs: colored::ColoredString) -> Self { ColoredString(cs) }
}

impl fmt::Display for ColoredString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { write!(f, "{}", self.0) }
}

pub fn format_duration(datetime: DateTime<Utc>) -> String {
    let current_time = Utc::now();
    let duration = current_time.signed_duration_since(datetime);

    match duration.num_seconds() {
        s if s >= 86400 => format!("{}d", s / 86400),
        s if s >= 3600 => format!("{}h", s / 3600),
        s if s >= 60 => format!("{}m", s / 60),
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
    let result = buffer.format((UNIT.powf(base - base.floor()) * 10.0).round() / 10.0).trim_end_matches(".0");

    [result, SUFFIX[base.floor() as usize]].join("")
}

/// Parse memory string like "100M", "1G", "500K" to bytes
pub fn parse_memory(mem_str: &str) -> Result<u64, String> {
    let mem_str = mem_str.trim().to_uppercase();
    let re = Regex::new(r"^(\d+(?:\.\d+)?)\s*([KMGT]?)B?$").unwrap();
    
    match re.captures(&mem_str) {
        Some(caps) => {
            let num_str = &caps[1];
            let num: f64 = num_str.parse()
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
        None => Err(format!("Invalid memory format: {}. Use format like '100M', '1G', '500K'", mem_str)),
    }
}
