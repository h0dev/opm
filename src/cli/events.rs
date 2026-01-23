use opm::{config, events::EventType};

/// Emit an event to the daemon if it's running
/// This is a best-effort operation using synchronous blocking HTTP
/// If the daemon is not running or not accessible, it silently fails
pub fn emit_event(
    event_type: EventType,
    process_id: usize,
    process_name: &str,
    message: &str,
) {
    // Convert to owned strings before spawning thread
    let process_name = process_name.to_string();
    let message = message.to_string();
    
    // Try to send event to local daemon API with a very short timeout
    // This is done synchronously to avoid complex async handling in CLI
    std::thread::spawn(move || {
        let config = config::read();
        if !config.daemon.web.api {
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

        if let Ok(client) = client {
            let _ = client.post(&url).json(&event).send();
        }
    });
}
