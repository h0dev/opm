use chrono::Local;
use global_placeholders::global;
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};

pub struct Logger {
    file: File,
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
        let args_str = args
            .iter()
            .map(|(key, value)| format!("{}={}", key, value))
            .collect::<Vec<String>>()
            .join(", ");
        let msg = format!("{message} ({args_str})");

        // Use external log crate for logging
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
            // If logger creation fails, fall back to just using log crate
            let args_str = args
                .iter()
                .map(|(key, value)| format!("{}={}", key, value))
                .collect::<Vec<String>>()
                .join(", ");
            ::log::info!("{} ({})", $msg, args_str);
        }
    }}
}
