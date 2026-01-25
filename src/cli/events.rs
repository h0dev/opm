use opm::{config, events::EventType};

/// Emit an event to the daemon if it's running
/// This is a best-effort operation using synchronous blocking HTTP
/// If the daemon is not running or not accessible, it silently fails
///
/// Note: Spawns a detached thread for each call. This is acceptable since CLI
/// operations are infrequent (not called in hot loops).
pub fn emit_event(event_type: EventType, process_id: usize, process_name: &str, message: &str) {
    // Convert to owned strings before spawning thread
    let process_name = process_name.to_string();
    let message = message.to_string();

    // Try to send event to local daemon API with a very short timeout
    // This is done in a separate thread to avoid blocking CLI operations
    std::thread::spawn(move || {
        let config = config::read();
        if !config.daemon.web.api {
            log::debug!("Daemon API not enabled, skipping event emission");
            return;
        }

        let base_url = format!("{}:{}", config.daemon.web.address, config.daemon.web.port);
        let path = config.get_path();
        let url = format!("http://{}{}/api/internal/cli-event", base_url, path);

        // Create event payload
        let event = serde_json::json!({
            "event_type": event_type,
            "agent_id": "local",
            "agent_name": "Local",
            "process_id": process_id.to_string(),
            "process_name": process_name,
            "message": message,
        });

        // Best effort - use blocking client with very short timeout
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_millis(100))
            .build();

        match client {
            Ok(client) => {
                if let Err(e) = client.post(&url).json(&event).send() {
                    log::debug!("Failed to send CLI event to daemon: {}", e);
                }
            }
            Err(e) => {
                log::debug!("Failed to create HTTP client for CLI event: {}", e);
            }
        }
    });
}
