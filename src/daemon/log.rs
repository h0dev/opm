use chrono::Local;
use global_placeholders::global;
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};

pub struct Logger {
    file: File,
}

/// Formats arguments into a string for logging
pub fn format_args(args: &HashMap<String, String>) -> String {
    args.iter()
        .map(|(key, value)| format!("{}={}", key, value))
        .collect::<Vec<String>>()
        .join(", ")
}

impl Logger {
    pub fn new() -> io::Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(global!("opm.daemon.log"))?;
        Ok(Logger { file })
    }

    pub fn write(&mut self, message: &str, args: HashMap<String, String>) {
        let args_str = format_args(&args);
        let msg = format!("{message} ({args_str})");

        // Use ::log:: prefix to reference the external log crate, avoiding shadowing
        // with the local log module in daemon/mod.rs
        ::log::info!("{msg}");
        // Silently ignore write errors to prevent panics
        let _ = writeln!(
            &mut self.file,
            "[{}] {msg}",
            Local::now().format("%Y-%m-%d %H:%M:%S%.3f")
        );
    }
}

#[macro_export]
macro_rules! log {
    ($msg:expr, $($key:expr => $value:expr),* $(,)?) => {{
        let mut args = std::collections::HashMap::new();
        $(args.insert($key.to_string(), format!("{}", $value));)*
        if let Ok(mut logger) = crate::daemon::log::Logger::new() {
            logger.write($msg, args)
        } else {
            // If logger creation fails, fall back to using the external log crate
            let args_str = crate::daemon::log::format_args(&args);
            ::log::info!("{} ({})", $msg, args_str);
        }
    }}
}
