mod docs;
mod fairing;
mod helpers;
mod routes;
mod structs;

use helpers::{create_status, NotFound};
use include_dir::{include_dir, Dir};
use lazy_static::lazy_static;
use opm::{config, process};
use prometheus::{opts, register_counter, register_gauge, register_histogram, register_histogram_vec};
use prometheus::{Counter, Gauge, Histogram, HistogramVec};
use serde_json::{json, Value};
use structs::ErrorMessage;

use utoipa::{
    openapi::security::{ApiKey, ApiKeyValue, SecurityScheme},
    Modify, OpenApi,
};

use rocket::{
    catch,
    http::{ContentType, Status},
    outcome::Outcome,
    request::{self, FromRequest, Request},
    response::{self, Responder},
    serde::json::Json,
    State,
};

use std::{io, path::PathBuf};

#[derive(Debug)]
struct NamedFile(PathBuf, String);

impl NamedFile {
    async fn send(name: String, contents: Option<&str>) -> io::Result<NamedFile> {
        match contents {
            Some(content) => Ok(NamedFile(PathBuf::from(name), content.to_string())),
            None => Err(io::Error::new(io::ErrorKind::InvalidData, "File contents are not valid UTF-8")),
        }
    }
}

impl<'r> Responder<'r, 'static> for NamedFile {
    fn respond_to(self, req: &'r Request<'_>) -> response::Result<'static> {
        let mut response = self.1.respond_to(req)?;
        if let Some(ext) = self.0.extension() {
            if let Some(ct) = ContentType::from_extension(&ext.to_string_lossy()) {
                response.set_header(ct);
            }
        }
        Ok(response)
    }
}

lazy_static! {
    pub static ref HTTP_COUNTER: Counter = register_counter!(opts!("http_requests_total", "Number of HTTP requests made.")).unwrap();
    pub static ref DAEMON_START_TIME: Gauge = register_gauge!(opts!("process_start_time_seconds", "The uptime of the daemon.")).unwrap();
    pub static ref DAEMON_MEM_USAGE: Histogram = register_histogram!("daemon_memory_usage", "The memory usage graph of the daemon.").unwrap();
    pub static ref DAEMON_CPU_PERCENTAGE: Histogram = register_histogram!("daemon_cpu_percentage", "The cpu usage graph of the daemon.").unwrap();
    pub static ref HTTP_REQ_HISTOGRAM: HistogramVec = register_histogram_vec!("http_request_duration_seconds", "The HTTP request latencies in seconds.", &["route"]).unwrap();
}

#[derive(OpenApi)]
#[openapi(
    modifiers(&SecurityAddon),
    paths(
        routes::action_handler,
        routes::env_handler,
        routes::info_handler,
        routes::dump_handler,
        routes::servers_handler,
        routes::config_handler,
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
        routes::rename_handler
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
        routes::ConfigBody,
        routes::CreateBody,
        routes::MetricsRoot,
        routes::LogResponse,
        routes::DocMemoryInfo,
        routes::ActionResponse,
    ))
)]

struct ApiDoc;
struct Logger;
struct AddCORS;
struct SecurityAddon;

impl Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.as_mut().unwrap();
        components.add_security_scheme("api_key", SecurityScheme::ApiKey(ApiKey::Header(ApiKeyValue::new("token"))))
    }
}

#[catch(500)]
fn internal_error<'m>() -> Json<ErrorMessage> { create_status(Status::InternalServerError) }

#[catch(400)]
fn bad_request<'m>() -> Json<ErrorMessage> { create_status(Status::BadRequest) }

#[catch(405)]
fn not_allowed<'m>() -> Json<ErrorMessage> { create_status(Status::MethodNotAllowed) }

#[catch(404)]
fn not_found<'m>() -> Json<ErrorMessage> { create_status(Status::NotFound) }

#[catch(401)]
fn unauthorized<'m>() -> Json<ErrorMessage> { create_status(Status::Unauthorized) }

#[rocket::async_trait]
impl<'r> FromRequest<'r> for routes::Token {
    type Error = ();

    async fn from_request(request: &'r rocket::Request<'_>) -> rocket::request::Outcome<Self, Self::Error> {
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

pub async fn start() {
    let s_path = config::read().get_path().trim_end_matches('/').to_string();

    let routes = rocket::routes![
        embed,
        health,
        docs_json,
        static_assets,
        routes::action_handler,
        routes::env_handler,
        routes::info_handler,
        routes::dump_handler,
        routes::remote_list,
        routes::remote_info,
        routes::remote_logs,
        routes::remote_rename,
        routes::remote_action,
        routes::servers_handler,
        routes::config_handler,
        routes::list_handler,
        routes::logs_handler,
        routes::logs_raw_handler,
        routes::metrics_handler,
        routes::remote_metrics,
        routes::stream_info,
        routes::stream_metrics,
        routes::prometheus_handler,
        routes::create_handler,
        routes::rename_handler,
    ];

    let rocket = rocket::custom(config::read().get_address())
        .attach(Logger)
        .attach(AddCORS)
        .mount(format!("{s_path}/"), routes)
        .register("/", rocket::catchers![internal_error, bad_request, not_allowed, not_found, unauthorized])
        .launch()
        .await;

    if let Err(err) = rocket {
        log::error!("failed to launch!\n{err}")
    }
}

#[rocket::get("/static/<name>")]
async fn static_assets(name: String) -> Option<NamedFile> {
    static DIR: Dir = include_dir!("src/daemon/static");
    let file = DIR.get_file(&name)?;

    NamedFile::send(name, file.contents_utf8()).await.ok()
}

#[rocket::get("/openapi.json")]
async fn docs_json() -> Value { json!(ApiDoc::openapi()) }

#[rocket::get("/docs/embed")]
async fn embed() -> (ContentType, String) { (ContentType::HTML, docs::Docs::new().render()) }

#[rocket::get("/health")]
async fn health() -> Value { json!({"healthy": true}) }
