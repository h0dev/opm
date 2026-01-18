use crate::{helpers, log, process::Process};
use colored::Colorize;
use macros_rs::{crashln, string, ternary};

use std::{
    env,
    fs::{self, File},
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    thread::sleep,
    time::Duration,
};

pub fn logs(item: &Process, lines_to_tail: usize, kind: &str) {
    logs_with_options(item, lines_to_tail, kind, false, None, false);
}

pub fn logs_with_options(
    item: &Process,
    lines_to_tail: usize,
    kind: &str,
    follow: bool,
    filter: Option<&str>,
    stats: bool,
) {
    let log_file = match kind {
        "out" => item.logs().out,
        "error" => item.logs().error,
        _ => item.logs().out,
    };

    if !Exists::check(&log_file).empty() {
        let file = File::open(&log_file).unwrap();
        let reader = BufReader::new(file);
        let lines: Vec<String> = reader
            .lines()
            .map(|line| line.unwrap_or_else(|err| format!("error reading line: {err}")))
            .collect();

        logs_internal_with_options(
            lines,
            lines_to_tail,
            &log_file,
            item.id,
            kind,
            &item.name,
            filter,
            stats,
        );

        if follow {
            // Follow mode: continuously watch for new lines
            use std::io::{Seek, SeekFrom};
            let mut file = File::open(&log_file).unwrap();

            // Start from the current end of file
            let mut last_pos = file.seek(SeekFrom::End(0)).unwrap();

            loop {
                // Check current file size
                let current_size = file.metadata().unwrap().len();

                if current_size > last_pos {
                    // New content available - read from last position
                    file.seek(SeekFrom::Start(last_pos)).unwrap();
                    let reader = BufReader::new(&file);

                    for line in reader.lines() {
                        if let Ok(line) = line {
                            if let Some(pattern) = filter {
                                if !line.to_lowercase().contains(&pattern.to_lowercase()) {
                                    continue;
                                }
                            }

                            let (level_indicator, line_color) = detect_log_level(&line, kind);
                            let color = ternary!(kind == "out", "green", "red");
                            println!(
                                "{} {} {}",
                                format!("{}|{}", item.id, item.name).color(color),
                                level_indicator,
                                line.color(line_color)
                            );
                        }
                    }

                    last_pos = current_size;
                } else if current_size < last_pos {
                    // File was truncated or rotated - reset position
                    last_pos = 0;
                    file.seek(SeekFrom::Start(0)).unwrap();
                }

                // Poll interval - using a simple polling mechanism
                // TODO: Consider using inotify on Linux for more efficient file watching
                sleep(Duration::from_millis(500));
            }
        }
    } else {
        println!(
            "{}",
            format!("[OPM] No logs found in {log_file}").bright_black()
        )
    }
}

pub fn logs_internal(
    lines: Vec<String>,
    lines_to_tail: usize,
    log_file: &str,
    id: usize,
    log_type: &str,
    item_name: &str,
) {
    logs_internal_with_options(
        lines,
        lines_to_tail,
        log_file,
        id,
        log_type,
        item_name,
        None,
        false,
    );
}

pub fn logs_internal_with_options(
    lines: Vec<String>,
    lines_to_tail: usize,
    log_file: &str,
    id: usize,
    log_type: &str,
    item_name: &str,
    filter: Option<&str>,
    stats: bool,
) {
    println!(
        "{}",
        format!("\n{log_file} last {lines_to_tail} lines:").bright_black()
    );

    let color = ternary!(log_type == "out", "green", "red");
    let start_index = if lines.len() > lines_to_tail {
        lines.len() - lines_to_tail
    } else {
        0
    };

    // Statistics counters
    let mut error_count = 0;
    let mut warn_count = 0;
    let mut info_count = 0;
    let mut debug_count = 0;
    let mut filtered_lines = Vec::new();

    for (_, line) in lines.iter().skip(start_index).enumerate() {
        // Apply filter if provided
        if let Some(pattern) = filter {
            if !line.to_lowercase().contains(&pattern.to_lowercase()) {
                continue;
            }
        }

        // Detect log level in the line content for better identification
        let (level_indicator, line_color) = detect_log_level(&line, log_type);

        // Count log levels for statistics
        if level_indicator.contains("ERR") {
            error_count += 1;
        } else if level_indicator.contains("WARN") {
            warn_count += 1;
        } else if level_indicator.contains("INFO") {
            info_count += 1;
        } else if level_indicator.contains("DBG") {
            debug_count += 1;
        }

        filtered_lines.push((level_indicator, line_color, line));
    }

    // Display statistics if requested
    if stats {
        println!("{}", "".bright_black());
        println!("{}", "Log Statistics:".bright_yellow().bold());
        println!(
            "  {} Errors:   {}",
            "✗".red(),
            error_count.to_string().red()
        );
        println!(
            "  {} Warnings: {}",
            "⚠".yellow(),
            warn_count.to_string().yellow()
        );
        println!(
            "  {} Info:     {}",
            "ℹ".blue(),
            info_count.to_string().blue()
        );
        println!(
            "  {} Debug:    {}",
            "⚙".cyan(),
            debug_count.to_string().cyan()
        );
        println!(
            "  {} Total:    {}",
            "∑".white(),
            filtered_lines.len().to_string().white()
        );
        println!("{}", "".bright_black());
    }

    // Display the filtered logs
    for (level_indicator, line_color, line) in filtered_lines {
        println!(
            "{} {} {}",
            format!("{}|{}", id, item_name).color(color),
            level_indicator,
            line.color(line_color)
        );
    }
}

/// Detect log level from line content and return appropriate indicator and color
fn detect_log_level(line: &str, log_type: &str) -> (String, &'static str) {
    let line_lower = line.to_lowercase();

    // Check for common error patterns
    if line_lower.contains("error") || line_lower.contains("fatal") || line_lower.contains("fail") {
        return ("[ ERR]".to_string(), "red");
    }

    // Check for common warning patterns
    if line_lower.contains("warn") || line_lower.contains("warning") {
        return ("[WARN]".to_string(), "yellow");
    }

    // Check for debug patterns
    if line_lower.contains("debug") || line_lower.contains("trace") {
        return ("[ DBG]".to_string(), "cyan");
    }

    // Check for info patterns
    if line_lower.contains("info") {
        return ("[INFO]".to_string(), "blue");
    }

    // Default based on log type
    if log_type == "error" {
        return ("[ ERR]".to_string(), "red");
    }

    // Default to stdout
    ("[OUT ]".to_string(), "white")
}

pub fn cwd() -> PathBuf {
    match env::current_dir() {
        Ok(path) => path,
        Err(_) => crashln!(
            "{} Unable to find current working directory",
            *helpers::FAIL
        ),
    }
}

pub fn make_relative(current: &Path, home: &Path) -> PathBuf {
    match current.strip_prefix(home) {
        Err(_) => Path::new(home).join(current),
        Ok(relative_path) => Path::new("~").join(relative_path),
    }
}

pub struct Exists<'p> {
    path: &'p str,
}

impl<'p> Exists<'p> {
    pub fn check(path: &'p str) -> Self {
        Self { path }
    }
    pub fn folder(&self) -> bool {
        Path::new(self.path).is_dir()
    }
    pub fn file(&self) -> bool {
        Path::new(self.path).exists()
    }
    pub fn empty(&self) -> bool {
        fs::metadata(Path::new(self.path))
            .map(|m| m.len() == 0)
            .unwrap_or(true)
    }
}

pub fn raw(path: String) -> Vec<u8> {
    match fs::read(&path) {
        Ok(contents) => contents,
        Err(err) => crashln!(
            "{} Cannot find dumpfile.\n{}",
            *helpers::FAIL,
            string!(err).white()
        ),
    }
}

pub fn read<T: serde::de::DeserializeOwned>(path: String) -> T {
    let contents = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(err) => crashln!(
            "{} Cannot find dumpfile.\n{}",
            *helpers::FAIL,
            string!(err).white()
        ),
    };

    match toml::from_str(&contents).map_err(|err| string!(err)) {
        Ok(parsed) => parsed,
        Err(err) => crashln!("{} Cannot parse dumpfile.\n{}", *helpers::FAIL, err.white()),
    }
}

pub fn from_object<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> T {
    match ron::de::from_bytes(&bytes) {
        Ok(parsed) => parsed,
        Err(err) => crashln!(
            "{} Cannot parse file.\n{}",
            *helpers::FAIL,
            string!(err).white()
        ),
    }
}

fn read_file_with_retry(path: &str) -> Result<Vec<u8>, String> {
    let mut retry_count = 0;
    let max_retries = 5;

    loop {
        match fs::read(path) {
            Ok(contents) => return Ok(contents),
            Err(err) => {
                retry_count += 1;
                if retry_count >= max_retries {
                    log!("[file::read] Cannot find file: {err}");
                    return Err(format!("Cannot find file: {}", err));
                } else {
                    log!(
                        "[file::read] Error reading file. Retrying... (Attempt {retry_count}/{max_retries})"
                    );
                    println!(
                        "{} Error reading file. Retrying... (Attempt {retry_count}/{max_retries})",
                        *helpers::FAIL
                    );
                }
            }
        }
        sleep(Duration::from_secs(1));
    }
}

pub fn try_read_object<T: serde::de::DeserializeOwned>(path: String) -> Result<T, String> {
    let bytes = read_file_with_retry(&path)?;

    // Parse the file content - return error if parsing fails
    match ron::de::from_bytes(&bytes) {
        Ok(parsed) => Ok(parsed),
        Err(err) => {
            log!("[file::parse] Cannot parse file: {err}");
            Err(format!("Cannot parse file: {}", err))
        }
    }
}

pub fn read_object<T: serde::de::DeserializeOwned>(path: String) -> T {
    let bytes = match read_file_with_retry(&path) {
        Ok(bytes) => bytes,
        Err(err) => crashln!("{} Cannot find file.\n{}", *helpers::FAIL, err.white()),
    };

    // Parse the file content - no retry loop needed as corrupted content won't fix itself
    match ron::de::from_bytes(&bytes) {
        Ok(parsed) => parsed,
        Err(err) => {
            log!("[file::parse] Cannot parse file: {err}");
            crashln!(
                "{} Cannot parse file.\n{}",
                *helpers::FAIL,
                string!(err).white()
            );
        }
    }
}
