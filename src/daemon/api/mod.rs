mod docs;
mod fairing;
mod helpers;
mod routes;
mod structs;
mod websocket;

use crate::webui::{self, assets::NamedFile};
use helpers::{NotFound, create_status};
use include_dir::{Dir, include_dir};
use lazy_static::lazy_static;
use once_cell::sync::OnceCell;
use opm::{config, process};
use prometheus::{Counter, Gauge, Histogram, HistogramVec};
use prometheus::{
    opts, register_counter, register_gauge, register_histogram, register_histogram_vec,
};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use global_placeholders::global;
use libc;
use std::fs::OpenOptions;
use std::os::unix::io::AsRawFd;
use structs::ErrorMessage;
use tera::Context;

use utoipa::{
    Modify, OpenApi,
    openapi::security::{ApiKey, ApiKeyValue, SecurityScheme},
};

use rocket::{
    State, catch,
    http::{ContentType, Status},
    outcome::Outcome,
    request::{self, FromRequest, Request},
    serde::json::Json,
};

// Global event and notification managers for daemon access
pub static GLOBAL_EVENT_MANAGER: OnceCell<Arc<opm::events::EventManager>> = OnceCell::new();
pub static GLOBAL_NOTIFICATION_MANAGER: OnceCell<Arc<opm::notifications::NotificationManager>> = OnceCell::new();

lazy_static! {
    pub static ref HTTP_COUNTER: Counter = register_counter!(opts!(
        "http_requests_total",
        "Number of HTTP requests made."
    ))
    .unwrap();
    pub static ref DAEMON_START_TIME: Gauge = register_gauge!(opts!(
        "process_start_time_seconds",
        "The uptime of the daemon."
    ))
    .unwrap();
    pub static ref DAEMON_MEM_USAGE: Histogram = register_histogram!(
        "daemon_memory_usage",
        "The memory usage graph of the daemon."
    )
    .unwrap();
    pub static ref DAEMON_CPU_PERCENTAGE: Histogram = register_histogram!(
        "daemon_cpu_percentage",
        "The cpu usage graph of the daemon."
    )
    .unwrap();
    pub static ref HTTP_REQ_HISTOGRAM: HistogramVec = register_histogram_vec!(
        "http_request_duration_seconds",
        "The HTTP request latencies in seconds.",
        &["route"]
    )
    .unwrap();
}

#[derive(OpenApi)]
#[openapi(
    modifiers(&SecurityAddon),
    paths(
        routes::action_handler,
        routes::bulk_action_handler,
        routes::env_handler,
        routes::info_handler,
        routes::dump_handler,
        routes::save_handler,
        routes::restore_handler,
        routes::servers_handler,
        routes::add_server_handler,
        routes::remove_server_handler,
        routes::config_handler,
        routes::get_notifications_handler,
        routes::save_notifications_handler,
        routes::test_notification_handler,
        routes::get_security_handler,
        routes::save_security_handler,
        routes::list_handler,
        routes::logs_handler,
        routes::remote_list,
        routes::remote_info,
        routes::remote_metrics,
        routes::remote_logs,
        routes::remote_rename,
        routes::remote_action,
        routes::logs_raw_handler,
        routes::metrics_handler,
        routes::prometheus_handler,
        routes::create_handler,
        routes::rename_handler,
        routes::agent_list_handler,
        routes::agent_unregister_handler,
        routes::agent_get_handler,
        routes::agent_processes_handler,
        routes::agent_action_handler,
    ),
    components(schemas(
        ErrorMessage,
        process::Log,
        process::Raw,
        process::Info,
        process::Stats,
        process::Watch,
        process::ItemSingle,
        process::ProcessItem,
        routes::Stats,
        routes::Daemon,
        routes::Version,
        routes::ActionBody,
        routes::AddServerBody,
        routes::ConfigBody,
        routes::CreateBody,
        routes::MetricsRoot,
        routes::LogResponse,
        routes::DocMemoryInfo,
        routes::ActionResponse,
        routes::NotificationConfig,
        routes::NotificationEvents,
        routes::SecurityConfig,
        routes::CliEventData,
        routes::TestNotificationBody,
        routes::BulkActionBody,
        routes::BulkActionResponse,
    ))
)]

struct ApiDoc;
struct Logger;
struct AddCORS;
struct EnableWebUI;
struct SecurityAddon;

struct TeraState {
    path: String,
    tera: tera::Tera,
}

impl Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.as_mut().unwrap();
        components.add_security_scheme(
            "api_key",
            SecurityScheme::ApiKey(ApiKey::Header(ApiKeyValue::new("token"))),
        )
    }
}

#[catch(500)]
fn internal_error<'m>() -> Json<ErrorMessage> {
    create_status(Status::InternalServerError)
}

#[catch(400)]
fn bad_request<'m>() -> Json<ErrorMessage> {
    create_status(Status::BadRequest)
}

#[catch(405)]
fn not_allowed<'m>() -> Json<ErrorMessage> {
    create_status(Status::MethodNotAllowed)
}

#[catch(404)]
fn not_found<'m>() -> Json<ErrorMessage> {
    create_status(Status::NotFound)
}

#[catch(401)]
fn unauthorized<'m>() -> Json<ErrorMessage> {
    create_status(Status::Unauthorized)
}

#[rocket::async_trait]
impl<'r> FromRequest<'r> for EnableWebUI {
    type Error = ();

    async fn from_request(_req: &'r Request<'_>) -> request::Outcome<Self, Self::Error> {
        let webui = IS_WEBUI.load(Ordering::Acquire);

        if webui {
            Outcome::Success(EnableWebUI)
        } else {
            Outcome::Error((rocket::http::Status::NotFound, ()))
        }
    }
}

#[rocket::async_trait]
impl<'r> FromRequest<'r> for routes::Token {
    type Error = ();

    async fn from_request(
        request: &'r rocket::Request<'_>,
    ) -> rocket::request::Outcome<Self, Self::Error> {
        let config = config::read().daemon.web;

        match config.secure {
            Some(val) => {
                if !val.enabled {
                    return Outcome::Success(routes::Token);
                }

                if let Some(header_value) = request.headers().get_one("token") {
                    if header_value == val.token {
                        return Outcome::Success(routes::Token);
                    }
                }

                Outcome::Error((rocket::http::Status::Unauthorized, ()))
            }
            None => return Outcome::Success(routes::Token),
        }
    }
}

static IS_WEBUI: AtomicBool = AtomicBool::new(false);

/// Redirects stderr to the daemon log file
/// This ensures that Rocket's error messages are captured in containers
fn redirect_stderr_to_log() {
    // Get the daemon log file path
    let log_path = global!("opm.daemon.log");

    // Open the log file in append mode
    match OpenOptions::new().create(true).append(true).open(log_path) {
        Ok(log_file) => {
            let log_fd = log_file.as_raw_fd();
            // Redirect stderr to the log file
            unsafe {
                let result = libc::dup2(log_fd, libc::STDERR_FILENO);
                if result == -1 {
                    let error = std::io::Error::last_os_error();
                    log::error!("Failed to dup2 stderr to log file: {}", error);
                    return;
                }
            }
            log::info!("Redirected stderr to daemon log file");
            // Note: log_file will be dropped here, but the duplicated file descriptor remains open
            // and is owned by the process. The dup2 call creates a new file descriptor that persists
            // independently of the original log_file.
        }
        Err(err) => {
            log::error!("Failed to open log file for stderr redirection: {}", err);
        }
    }
}

pub async fn start(webui: bool) {
    IS_WEBUI.store(webui, Ordering::Release);

    // Redirect stderr to the daemon log file so that Rocket errors are captured
    // This is critical in containerized environments where stderr might not be accessible
    redirect_stderr_to_log();

    log::info!("API start: Creating templates");
    let tera = webui::create_templates();
    let s_path = config::read().get_path().trim_end_matches('/').to_string();

    log::info!("API start: Initializing event manager");
    // Initialize event manager (max 1000 events in memory)
    let event_manager = std::sync::Arc::new(opm::events::EventManager::new(1000));
    
    // Store in global static for daemon access
    // Safe to ignore error: API start is called once per daemon lifecycle
    let _ = GLOBAL_EVENT_MANAGER.set(event_manager.clone());
    
    log::info!("API start: Initializing notification manager");
    // Initialize notification manager with current config
    let notification_config = config::read().daemon.notifications;
    let notification_manager = std::sync::Arc::new(opm::notifications::NotificationManager::new(notification_config));
    
    // Store in global static for daemon access
    // Safe to ignore error: API start is called once per daemon lifecycle
    let _ = GLOBAL_NOTIFICATION_MANAGER.set(notification_manager.clone());

    log::info!("API start: Initializing agent registry");
    // Initialize agent registry
    let agent_registry = opm::agent::registry::AgentRegistry::new();

    log::info!("API start: Building routes");
    let routes = rocket::routes![
        embed,
        health,
        docs_json,
        static_assets,
        dynamic_assets,
        routes::login,
        routes::servers,
        routes::dashboard,
        routes::view_process,
        routes::server_status,
        routes::events_page,
        routes::system_page,
        routes::agent_detail,
        routes::settings_page,
        routes::action_handler,
        routes::env_handler,
        routes::info_handler,
        routes::dump_handler,
        routes::save_handler,
        routes::restore_handler,
        routes::remote_list,
        routes::remote_info,
        routes::remote_logs,
        routes::remote_rename,
        routes::remote_action,
        routes::servers_handler,
        routes::add_server_handler,
        routes::remove_server_handler,
        routes::config_handler,
        routes::get_notifications_handler,
        routes::save_notifications_handler,
        routes::test_notification_handler,
        routes::get_security_handler,
        routes::save_security_handler,
        routes::get_events_handler,
        routes::clear_events_handler,
        routes::get_system_info_handler,
        routes::bulk_action_handler,
        routes::list_handler,
        routes::logs_handler,
        routes::logs_raw_handler,
        routes::metrics_handler,
        routes::remote_metrics,
        routes::stream_info,
        routes::stream_metrics,
        routes::stream_agents,
        routes::stream_agent_detail,
        routes::stream_events,
        routes::cli_event_handler,
        routes::prometheus_handler,
        routes::create_handler,
        routes::rename_handler,
        routes::agent_list_handler,
        routes::agent_unregister_handler,
        routes::agent_get_handler,
        routes::agent_processes_handler,
        routes::agent_action_handler,
        websocket::websocket_handler,
    ];

    log::info!(
        "API start: Configuring Rocket server at {}",
        config::read().fmt_address()
    );

    let rocket = rocket::custom(config::read().get_address())
        .attach(Logger)
        .attach(AddCORS)
        .manage(TeraState {
            path: tera.1,
            tera: tera.0,
        })
        .manage(event_manager)
        .manage(notification_manager)
        .manage(agent_registry)
        .mount(format!("{s_path}/"), routes)
        .register(
            "/",
            rocket::catchers![
                internal_error,
                bad_request,
                not_allowed,
                not_found,
                unauthorized
            ],
        );

    log::info!("API start: Launching Rocket server");
    let result = rocket.launch().await;

    if let Err(err) = result {
        log::error!("Failed to launch Rocket server: {}", err);
        eprintln!("ERROR: Failed to launch API server: {}", err);
        eprintln!("Please check:");
        eprintln!("  1. The port is not already in use");
        eprintln!("  2. You have permission to bind to the configured address");
        eprintln!("  3. Your firewall settings allow the connection");
    } else {
        log::info!("Rocket server stopped normally");
    }
}

async fn render(
    name: &str,
    state: &State<TeraState>,
    ctx: &mut Context,
) -> Result<String, NotFound> {
    ctx.insert("base_path", &state.path);
    ctx.insert("build_version", env!("CARGO_PKG_VERSION"));

    state
        .tera
        .render(name, &ctx)
        .or(Err(helpers::not_found("Page was not found")))
}

#[rocket::get("/assets/<name>")]
async fn dynamic_assets(name: String) -> Option<NamedFile> {
    #[cfg(all(not(debug_assertions), feature = "webui"))]
    {
        static DIR: Dir = include_dir!("src/webui/dist/assets");
        let file = DIR.get_file(&name)?;
        NamedFile::send(name, file.contents_utf8()).await.ok()
    }

    #[cfg(any(debug_assertions, not(feature = "webui")))]
    {
        let _ = name; // Unused in debug builds or when webui is disabled
        None
    }
}

#[rocket::get("/static/<name>")]
async fn static_assets(name: String) -> Option<NamedFile> {
    static DIR: Dir = include_dir!("src/daemon/static");
    let file = DIR.get_file(&name)?;

    NamedFile::send(name, file.contents_utf8()).await.ok()
}

#[rocket::get("/openapi.json")]
async fn docs_json() -> Value {
    json!(ApiDoc::openapi())
}

#[rocket::get("/docs/embed")]
async fn embed() -> (ContentType, String) {
    (ContentType::HTML, docs::Docs::new().render())
}

#[rocket::get("/health")]
async fn health(_t: routes::Token) -> Value {
    json!({"healthy": true})
}
