use opm::config;
use tera::Tera;

pub fn create_templates() -> (Tera, String) {
    let mut tera = Tera::default();
    let path = config::read().get_path();

    #[cfg(not(debug_assertions))]
    {
        tera.add_raw_templates(vec![
            ("view", include_str!("dist/view.html")),
            ("login", include_str!("dist/login.html")),
            ("dashboard", include_str!("dist/index.html")),
            ("status", include_str!("dist/status.html")),
            ("servers", include_str!("dist/servers.html")),
            ("notifications", include_str!("dist/notifications.html")),
            ("agent-detail", include_str!("dist/agent-detail.html")),
        ])
        .unwrap();
    }

    #[cfg(debug_assertions)]
    {
        // For debug builds, add placeholder templates
        tera.add_raw_templates(vec![
            (
                "view",
                "<html><body><h1>Debug Mode - WebUI not built</h1></body></html>",
            ),
            (
                "login",
                "<html><body><h1>Debug Mode - WebUI not built</h1></body></html>",
            ),
            (
                "dashboard",
                "<html><body><h1>Debug Mode - WebUI not built</h1></body></html>",
            ),
            (
                "status",
                "<html><body><h1>Debug Mode - WebUI not built</h1></body></html>",
            ),
            (
                "servers",
                "<html><body><h1>Debug Mode - WebUI not built</h1></body></html>",
            ),
            (
                "notifications",
                "<html><body><h1>Debug Mode - WebUI not built</h1></body></html>",
            ),
            (
                "agent-detail",
                "<html><body><h1>Debug Mode - WebUI not built</h1></body></html>",
            ),
        ])
        .unwrap();
    }

    return (tera, path.trim_end_matches('/').to_string());
}

pub mod assets;
