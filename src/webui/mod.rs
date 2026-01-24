use opm::config;
use tera::Tera;

pub fn create_templates() -> (Tera, String) {
    let mut tera = Tera::default();
    let path = config::read().get_path();

    #[cfg(all(not(debug_assertions), feature = "webui"))]
    {
        tera.add_raw_templates(vec![
            ("view", include_str!("dist/view.html")),
            ("login", include_str!("dist/login.html")),
            ("dashboard", include_str!("dist/index.html")),
            ("status", include_str!("dist/status.html")),
            ("servers", include_str!("dist/servers.html")),
            ("events", include_str!("dist/events.html")),
            ("system", include_str!("dist/system.html")),
            ("agent-detail", include_str!("dist/agent-detail.html")),
        ])
        .unwrap();
    }

    #[cfg(any(debug_assertions, not(feature = "webui")))]
    {
        // For debug builds or when webui is disabled, add placeholder templates
        tera.add_raw_templates(vec![
            (
                "view",
                "<html><body><h1>WebUI not available</h1><p>Build with --features webui to enable the web interface</p></body></html>",
            ),
            (
                "login",
                "<html><body><h1>WebUI not available</h1><p>Build with --features webui to enable the web interface</p></body></html>",
            ),
            (
                "dashboard",
                "<html><body><h1>WebUI not available</h1><p>Build with --features webui to enable the web interface</p></body></html>",
            ),
            (
                "status",
                "<html><body><h1>WebUI not available</h1><p>Build with --features webui to enable the web interface</p></body></html>",
            ),
            (
                "servers",
                "<html><body><h1>WebUI not available</h1><p>Build with --features webui to enable the web interface</p></body></html>",
            ),
            (
                "events",
                "<html><body><h1>WebUI not available</h1><p>Build with --features webui to enable the web interface</p></body></html>",
            ),
            (
                "system",
                "<html><body><h1>WebUI not available</h1><p>Build with --features webui to enable the web interface</p></body></html>",
            ),
            (
                "agent-detail",
                "<html><body><h1>WebUI not available</h1><p>Build with --features webui to enable the web interface</p></body></html>",
            ),
        ])
        .unwrap();
    }

    return (tera, path.trim_end_matches('/').to_string());
}

pub mod assets;
