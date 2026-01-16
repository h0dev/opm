#![allow(non_snake_case)]

use chrono::{DateTime, Utc};
use global_placeholders::global;
use macros_rs::{fmtstr, string, ternary, then};
use prometheus::{Encoder, TextEncoder};
#[cfg(any(target_os = "linux", target_os = "macos"))]
use opm::process::unix::NativeProcess as Process;
use reqwest::header::HeaderValue;
use tera::Context;
use toml;
use utoipa::ToSchema;
use serde_json::json;

use rocket::{
    delete,
    get,
    http::{ContentType, Status},
    post,
    response::stream::{Event, EventStream},
    serde::{json::Json, Deserialize, Serialize},
    State,
};

use super::{
    helpers::{generic_error, not_found, GenericError, NotFound},
    render,
    structs::ErrorMessage,
    EnableWebUI, TeraState,
};

use opm::{
    config, file, helpers,
    process::{dump, http::client, ItemSingle, ProcessItem, Runner, get_process_cpu_usage_with_children_from_process, get_process_memory_with_children},
};

use crate::daemon::{
    api::{HTTP_COUNTER, HTTP_REQ_HISTOGRAM},
    pid::{self, Pid},
};

use std::{
    collections::{BTreeMap, HashMap},
    env,
    fs::{self, File},
    io::{self, BufRead, BufReader},
    path::PathBuf,
    thread::sleep,
    time::Duration,
};

use home;

pub(crate) struct Token;
type EnvList = Json<BTreeMap<String, String>>;

#[allow(dead_code)]
#[derive(ToSchema)]
#[schema(as = MemoryInfo)]
pub(crate) struct DocMemoryInfo {
    rss: u64,
    vms: u64,
    #[cfg(target_os = "linux")]
    shared: u64,
    #[cfg(target_os = "linux")]
    text: u64,
    #[cfg(target_os = "linux")]
    data: u64,
    #[cfg(target_os = "macos")]
    page_faults: u64,
    #[cfg(target_os = "macos")]
    pageins: u64,
}

#[derive(Serialize, Deserialize, ToSchema)]
pub(crate) struct ActionBody {
    #[schema(example = "restart")]
    method: String,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct ConfigBody {
    #[schema(example = "bash")]
    shell: String,
    #[schema(min_items = 1, example = json!(["-c"]))]
    args: Vec<String>,
    #[schema(example = "/home/user/.opm/logs")]
    log_path: String,
}

#[derive(Serialize, Deserialize, ToSchema)]
pub(crate) struct CreateBody {
    #[schema(example = "app")]
    name: Option<String>,
    #[schema(example = "node index.js")]
    script: String,
    #[schema(value_type = String, example = "/projects/app")]
    path: PathBuf,
    #[schema(example = "src")]
    watch: Option<String>,
}

#[derive(Serialize, Deserialize, ToSchema)]
pub(crate) struct ActionResponse {
    #[schema(example = true)]
    done: bool,
    #[schema(example = "name")]
    action: String,
}

#[derive(Serialize, Deserialize, ToSchema)]
pub(crate) struct LogResponse {
    logs: Vec<String>,
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct MetricsRoot {
    pub raw: Raw,
    pub version: Version,
    pub os: crate::globals::Os,
    pub daemon: Daemon,
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct Raw {
    pub memory_usage: Option<u64>,
    pub cpu_percent: Option<f64>,
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct Version {
    #[schema(example = "v1.0.0")]
    pub pkg: String,
    pub hash: Option<String>,
    #[schema(example = "2000-01-01")]
    pub build_date: String,
    #[schema(example = "release")]
    pub target: String,
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct Daemon {
    pub pid: Option<Pid>,
    #[schema(example = true)]
    pub running: bool,
    pub uptime: String,
    pub process_count: usize,
    #[schema(example = "default")]
    pub daemon_type: String,
    pub stats: Stats,
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct Stats {
    pub memory_usage: String,
    pub cpu_percent: String,
}

fn attempt(done: bool, method: &str) -> ActionResponse {
    ActionResponse {
        done,
        action: ternary!(done, Box::leak(Box::from(method)), "DOES_NOT_EXIST").to_string(),
    }
}

// WebUI Routes
#[get("/")]
pub async fn dashboard(state: &State<TeraState>, _webui: EnableWebUI) -> Result<(ContentType, String), NotFound> { 
    Ok((ContentType::HTML, render("dashboard", &state, &mut Context::new()).await?)) 
}

#[get("/servers")]
pub async fn servers(state: &State<TeraState>, _webui: EnableWebUI) -> Result<(ContentType, String), NotFound> { 
    Ok((ContentType::HTML, render("servers", &state, &mut Context::new()).await?)) 
}

#[get("/login")]
pub async fn login(state: &State<TeraState>, _webui: EnableWebUI) -> Result<(ContentType, String), NotFound> { 
    Ok((ContentType::HTML, render("login", &state, &mut Context::new()).await?)) 
}

#[get("/view/<id>")]
pub async fn view_process(id: usize, state: &State<TeraState>, _webui: EnableWebUI) -> Result<(ContentType, String), NotFound> {
    let mut ctx = Context::new();
    ctx.insert("process_id", &id);
    Ok((ContentType::HTML, render("view", &state, &mut ctx).await?))
}

#[get("/status/<name>")]
pub async fn server_status(name: String, state: &State<TeraState>, _webui: EnableWebUI) -> Result<(ContentType, String), NotFound> {
    let mut ctx = Context::new();
    ctx.insert("server_name", &name);
    Ok((ContentType::HTML, render("status", &state, &mut ctx).await?))
}

#[get("/notifications")]
pub async fn notifications(state: &State<TeraState>, _webui: EnableWebUI) -> Result<(ContentType, String), NotFound> { 
    Ok((ContentType::HTML, render("notifications", &state, &mut Context::new()).await?)) 
}

#[get("/daemon/prometheus")]
#[utoipa::path(get, tag = "Daemon", path = "/daemon/prometheus", security((), ("api_key" = [])),
    responses(
        (
            description = "Get prometheus metrics", body = String, status = 200,
            example = json!("# HELP daemon_cpu_percentage The cpu usage graph of the daemon.\n# TYPE daemon_cpu_percentage histogram\ndaemon_cpu_percentage_bucket{le=\"0.005\"} 0"),
        ),
        (
            status = UNAUTHORIZED, description = "Authentication failed or not provided", body = ErrorMessage, 
            example = json!({"code": 401, "message": "Unauthorized"})
        )
    )
)]
pub async fn prometheus_handler(_t: Token) -> String {
    let encoder = TextEncoder::new();
    let mut buffer = Vec::<u8>::new();
    let metric_families = prometheus::gather();

    encoder.encode(&metric_families, &mut buffer).unwrap();
    String::from_utf8(buffer.clone()).unwrap()
}

#[get("/daemon/servers")]
#[utoipa::path(get, tag = "Daemon", path = "/daemon/servers", security((), ("api_key" = [])),
    responses(
        (status = 200, description = "Get daemon servers successfully", body = [String]),
        (
            status = UNAUTHORIZED, description = "Authentication failed or not provided", body = ErrorMessage, 
            example = json!({"code": 401, "message": "Unauthorized"})
        )
    )
)]
pub async fn servers_handler(_t: Token) -> Result<Json<Vec<String>>, GenericError> {
    let timer = HTTP_REQ_HISTOGRAM.with_label_values(&["servers"]).start_timer();
    
    let result = if let Some(servers) = config::servers().servers {
        servers.into_keys().collect()
    } else {
        vec![]
    };
    
    HTTP_COUNTER.inc();
    timer.observe_duration();
    
    Ok(Json(result))
}

#[derive(Deserialize, ToSchema)]
pub struct AddServerBody {
    pub name: String,
    pub address: String,
    pub token: Option<String>,
}

#[post("/daemon/servers/add", format = "json", data = "<body>")]
#[utoipa::path(post, tag = "Daemon", path = "/daemon/servers/add", request_body = AddServerBody,
    security((), ("api_key" = [])),
    responses(
        (status = 200, description = "Server added successfully", body = ActionResponse),
        (
            status = UNAUTHORIZED, description = "Authentication failed or not provided", body = ErrorMessage, 
            example = json!({"code": 401, "message": "Unauthorized"})
        )
    )
)]
pub async fn add_server_handler(body: Json<AddServerBody>, _t: Token) -> Json<ActionResponse> {
    let timer = HTTP_REQ_HISTOGRAM.with_label_values(&["add_server"]).start_timer();
    HTTP_COUNTER.inc();
    
    let mut servers = config::servers();
    let server = config::structs::Server {
        address: body.address.trim_end_matches('/').to_string(),
        token: body.token.clone(),
    };
    
    if servers.servers.is_none() {
        servers.servers = Some(BTreeMap::new());
    }
    
    if let Some(ref mut server_map) = servers.servers {
        server_map.insert(body.name.clone(), server);
    }
    
    // Save to file
    match home::home_dir() {
        Some(path) => {
            let config_path = format!("{}/.opm/servers.toml", path.display());
            let contents = match toml::to_string(&servers) {
                Ok(c) => c,
                Err(_) => return Json(attempt(false, "add_server")),
            };
            
            if let Err(_) = fs::write(&config_path, contents) {
                return Json(attempt(false, "add_server"));
            }
        }
        None => return Json(attempt(false, "add_server")),
    }
    
    timer.observe_duration();
    Json(attempt(true, "add_server"))
}

#[delete("/daemon/servers/<name>")]
#[utoipa::path(delete, tag = "Daemon", path = "/daemon/servers/{name}",
    security((), ("api_key" = [])),
    params(("name" = String, Path, description = "Server name to remove")),
    responses(
        (status = 200, description = "Server removed successfully", body = ActionResponse),
        (
            status = UNAUTHORIZED, description = "Authentication failed or not provided", body = ErrorMessage, 
            example = json!({"code": 401, "message": "Unauthorized"})
        )
    )
)]
pub async fn remove_server_handler(name: String, _t: Token) -> Json<ActionResponse> {
    let timer = HTTP_REQ_HISTOGRAM.with_label_values(&["remove_server"]).start_timer();
    HTTP_COUNTER.inc();
    
    let mut servers = config::servers();
    
    if let Some(ref mut server_map) = servers.servers {
        server_map.remove(&name);
    }
    
    // Save to file
    match home::home_dir() {
        Some(path) => {
            let config_path = format!("{}/.opm/servers.toml", path.display());
            let contents = match toml::to_string(&servers) {
                Ok(c) => c,
                Err(_) => return Json(attempt(false, "remove_server")),
            };
            
            if let Err(_) = fs::write(&config_path, contents) {
                return Json(attempt(false, "remove_server"));
            }
        }
        None => return Json(attempt(false, "remove_server")),
    }
    
    timer.observe_duration();
    Json(attempt(true, "remove_server"))
}


#[get("/remote/<name>/list")]
#[utoipa::path(get, tag = "Remote", path = "/remote/{name}/list", security((), ("api_key" = [])),
    params(("name" = String, Path, description = "Name of remote daemon", example = "example"),),
    responses(
        (status = 200, description = "Get list from remote daemon successfully", body = [ProcessItem]),
        (status = NOT_FOUND, description = "Remote daemon does not exist", body = ErrorMessage),
        (
            status = UNAUTHORIZED, description = "Authentication failed or not provided", body = ErrorMessage, 
            example = json!({"code": 401, "message": "Unauthorized"})
        )
    )
)]
pub async fn remote_list(name: String, _t: Token) -> Result<Json<Vec<ProcessItem>>, GenericError> {
    let timer = HTTP_REQ_HISTOGRAM.with_label_values(&["list"]).start_timer();

    if let Some(servers) = config::servers().servers {
        let (address, (client, headers)) = match servers.get(&name) {
            Some(server) => (&server.address, client(&server.token).await),
            None => return Err(generic_error(Status::NotFound, string!("Server was not found"))),
        };

        HTTP_COUNTER.inc();
        timer.observe_duration();

        match client.get(fmtstr!("{address}/list")).headers(headers).send().await {
            Ok(data) => {
                if data.status() != 200 {
                    let err = data.json::<ErrorMessage>().await.unwrap();
                    Err(generic_error(err.code, err.message))
                } else {
                    Ok(Json(data.json::<Vec<ProcessItem>>().await.unwrap()))
                }
            }
            Err(err) => Err(generic_error(Status::InternalServerError, err.to_string())),
        }
    } else {
        Err(generic_error(Status::BadRequest, string!("No servers have been added")))
    }
}

#[get("/remote/<name>/info/<id>")]
#[utoipa::path(get, tag = "Remote", path = "/remote/{name}/info/{id}", security((), ("api_key" = [])),
    params(
        ("name" = String, Path, description = "Name of remote daemon", example = "example"),
        ("id" = usize, Path, description = "Process id to get information for", example = 0)
    ),
    responses(
        (status = 200, description = "Get process info from remote daemon successfully", body = [ProcessItem]),
        (status = NOT_FOUND, description = "Remote daemon does not exist", body = ErrorMessage),
        (
            status = UNAUTHORIZED, description = "Authentication failed or not provided", body = ErrorMessage, 
            example = json!({"code": 401, "message": "Unauthorized"})
        )
    )
)]
pub async fn remote_info(name: String, id: usize, _t: Token) -> Result<Json<ItemSingle>, GenericError> {
    let timer = HTTP_REQ_HISTOGRAM.with_label_values(&["info"]).start_timer();

    if let Some(servers) = config::servers().servers {
        let (address, (client, headers)) = match servers.get(&name) {
            Some(server) => (&server.address, client(&server.token).await),
            None => return Err(generic_error(Status::NotFound, string!("Server was not found"))),
        };

        HTTP_COUNTER.inc();
        timer.observe_duration();

        match client.get(fmtstr!("{address}/process/{id}/info")).headers(headers).send().await {
            Ok(data) => {
                if data.status() != 200 {
                    let err = data.json::<ErrorMessage>().await.unwrap();
                    Err(generic_error(err.code, err.message))
                } else {
                    Ok(Json(data.json::<ItemSingle>().await.unwrap()))
                }
            }
            Err(err) => Err(generic_error(Status::InternalServerError, err.to_string())),
        }
    } else {
        Err(generic_error(Status::BadRequest, string!("No servers have been added")))
    }
}

#[get("/remote/<name>/logs/<id>/<kind>")]
#[utoipa::path(get, tag = "Remote", path = "/remote/{name}/logs/{id}/{kind}", security((), ("api_key" = [])),
    params(
        ("name" = String, Path, description = "Name of remote daemon", example = "example"),
        ("id" = usize, Path, description = "Process id to get information for", example = 0),
        ("kind" = String, Path, description = "Log output type", example = "out")
    ),
    responses(
        (status = 200, description = "Remote process logs of {type} fetched", body = LogResponse),
        (status = NOT_FOUND, description = "Remote daemon does not exist", body = ErrorMessage),
        (
            status = UNAUTHORIZED, description = "Authentication failed or not provided", body = ErrorMessage, 
            example = json!({"code": 401, "message": "Unauthorized"})
        )
    )
)]
pub async fn remote_logs(name: String, id: usize, kind: String, _t: Token) -> Result<Json<LogResponse>, GenericError> {
    let timer = HTTP_REQ_HISTOGRAM.with_label_values(&["info"]).start_timer();

    if let Some(servers) = config::servers().servers {
        let (address, (client, headers)) = match servers.get(&name) {
            Some(server) => (&server.address, client(&server.token).await),
            None => return Err(generic_error(Status::NotFound, string!("Server was not found"))),
        };

        HTTP_COUNTER.inc();
        timer.observe_duration();

        match client.get(fmtstr!("{address}/process/{id}/logs/{kind}")).headers(headers).send().await {
            Ok(data) => {
                if data.status() != 200 {
                    let err = data.json::<ErrorMessage>().await.unwrap();
                    Err(generic_error(err.code, err.message))
                } else {
                    Ok(Json(data.json::<LogResponse>().await.unwrap()))
                }
            }
            Err(err) => Err(generic_error(Status::InternalServerError, err.to_string())),
        }
    } else {
        Err(generic_error(Status::BadRequest, string!("No servers have been added")))
    }
}

#[post("/remote/<name>/rename/<id>", format = "text", data = "<body>")]
#[utoipa::path(post, tag = "Remote", path = "/remote/{name}/rename/{id}", 
    security((), ("api_key" = [])),
    request_body(content = String, example = json!("example_name")), 
    params(
        ("id" = usize, Path, description = "Process id to rename", example = 0),
        ("name" = String, Path, description = "Name of remote daemon", example = "example"),
    ),
    responses(
        (
            description = "Remote rename process successful", body = ActionResponse,
            example = json!({"action": "rename", "done": true }), status = 200,
        ),
        (status = NOT_FOUND, description = "Process was not found", body = ErrorMessage),
        (
            status = UNAUTHORIZED, description = "Authentication failed or not provided", body = ErrorMessage, 
            example = json!({"code": 401, "message": "Unauthorized"})
        )
    )
)]
pub async fn remote_rename(name: String, id: usize, body: String, _t: Token) -> Result<Json<ActionResponse>, GenericError> {
    let timer = HTTP_REQ_HISTOGRAM.with_label_values(&["rename"]).start_timer();

    if let Some(servers) = config::servers().servers {
        let (address, (client, mut headers)) = match servers.get(&name) {
            Some(server) => (&server.address, client(&server.token).await),
            None => return Err(generic_error(Status::NotFound, string!("Server was not found"))),
        };

        HTTP_COUNTER.inc();
        timer.observe_duration();
        headers.insert("content-type", HeaderValue::from_static("text/plain"));

        match client.post(fmtstr!("{address}/process/{id}/rename")).body(body).headers(headers).send().await {
            Ok(data) => {
                if data.status() != 200 {
                    let err = data.json::<ErrorMessage>().await.unwrap();
                    Err(generic_error(err.code, err.message))
                } else {
                    Ok(Json(data.json::<ActionResponse>().await.unwrap()))
                }
            }
            Err(err) => Err(generic_error(Status::InternalServerError, err.to_string())),
        }
    } else {
        Err(generic_error(Status::BadRequest, string!("No servers have been added")))
    }
}

#[post("/remote/<name>/action/<id>", format = "json", data = "<body>")]
#[utoipa::path(post, tag = "Remote", path = "/remote/{name}/action/{id}", request_body = ActionBody,
    security((), ("api_key" = [])),
    params(
        ("id" = usize, Path, description = "Process id to run action on", example = 0),
        ("name" = String, Path, description = "Name of remote daemon", example = "example")
    ),
    responses(
        (status = 200, description = "Run action on remote process successful", body = ActionResponse),
        (status = NOT_FOUND, description = "Process/action was not found", body = ErrorMessage),
        (
            status = UNAUTHORIZED, description = "Authentication failed or not provided", body = ErrorMessage, 
            example = json!({"code": 401, "message": "Unauthorized"})
        )
    )
)]
pub async fn remote_action(name: String, id: usize, body: Json<ActionBody>, _t: Token) -> Result<Json<ActionResponse>, GenericError> {
    let timer = HTTP_REQ_HISTOGRAM.with_label_values(&["action"]).start_timer();

    if let Some(servers) = config::servers().servers {
        let (address, (client, headers)) = match servers.get(&name) {
            Some(server) => (&server.address, client(&server.token).await),
            None => return Err(generic_error(Status::NotFound, string!("Server was not found"))),
        };

        HTTP_COUNTER.inc();
        timer.observe_duration();

        match client.post(fmtstr!("{address}/process/{id}/action")).json(&body.0).headers(headers).send().await {
            Ok(data) => {
                if data.status() != 200 {
                    let err = data.json::<ErrorMessage>().await.unwrap();
                    Err(generic_error(err.code, err.message))
                } else {
                    Ok(Json(data.json::<ActionResponse>().await.unwrap()))
                }
            }
            Err(err) => Err(generic_error(Status::InternalServerError, err.to_string())),
        }
    } else {
        Err(generic_error(Status::BadRequest, string!("No servers have been added")))
    }
}

#[get("/daemon/dump")]
#[utoipa::path(get, tag = "Daemon", path = "/daemon/dump", security((), ("api_key" = [])),
    responses(
        (status = 200, description = "Dump processes successfully", body = [u8]),
        (
            status = UNAUTHORIZED, description = "Authentication failed or not provided", body = ErrorMessage, 
            example = json!({"code": 401, "message": "Unauthorized"})
        )
    )
)]
pub async fn dump_handler(_t: Token) -> Vec<u8> {
    let timer = HTTP_REQ_HISTOGRAM.with_label_values(&["dump"]).start_timer();

    HTTP_COUNTER.inc();
    timer.observe_duration();

    dump::raw()
}

#[post("/daemon/save")]
#[utoipa::path(post, tag = "Daemon", path = "/daemon/save", security((), ("api_key" = [])),
    responses(
        (status = 200, description = "Save all processes successfully", body = ActionResponse),
        (
            status = UNAUTHORIZED, description = "Authentication failed or not provided", body = ErrorMessage, 
            example = json!({"code": 401, "message": "Unauthorized"})
        )
    )
)]
pub async fn save_handler(_t: Token) -> Json<ActionResponse> {
    let timer = HTTP_REQ_HISTOGRAM.with_label_values(&["save"]).start_timer();
    HTTP_COUNTER.inc();
    
    Runner::new().save();
    
    timer.observe_duration();
    Json(attempt(true, "save"))
}

#[post("/daemon/restore")]
#[utoipa::path(post, tag = "Daemon", path = "/daemon/restore", security((), ("api_key" = [])),
    responses(
        (status = 200, description = "Restore all processes successfully", body = ActionResponse),
        (
            status = UNAUTHORIZED, description = "Authentication failed or not provided", body = ErrorMessage, 
            example = json!({"code": 401, "message": "Unauthorized"})
        )
    )
)]
pub async fn restore_handler(_t: Token) -> Json<ActionResponse> {
    let timer = HTTP_REQ_HISTOGRAM.with_label_values(&["restore"]).start_timer();
    HTTP_COUNTER.inc();
    
    let runner = Runner::new();
    
    // Collect IDs of processes that were running when saved
    let running_ids: Vec<usize> = runner.items()
        .into_iter()
        .filter(|(_, item)| item.running)
        .map(|(_, item)| item.id)
        .collect();
    
    // Restore those processes (without incrementing counters)
    let mut runner = Runner::new();
    let total_processes = running_ids.len();
    for (index, id) in running_ids.iter().enumerate() {
        runner.restart(*id, false, false);
        runner.save();
        
        // Only add delay between processes when restoring multiple processes
        // This prevents resource conflicts and false crash detection
        // Skip delay after the last process
        if total_processes > 1 && index < total_processes - 1 {
            tokio::time::sleep(tokio::time::Duration::from_millis(150)).await;
        }
    }
    
    // Reset restart and crash counters after restore for ALL processes
    // This gives each process a fresh start after system restore/reboot
    let all_process_ids: Vec<usize> = runner.items().keys().copied().collect();
    for id in all_process_ids {
        runner.reset_counters(id);
    }
    runner.save();
    
    timer.observe_duration();
    Json(attempt(true, "restore"))
}

#[get("/daemon/config")]
#[utoipa::path(get, tag = "Daemon", path = "/daemon/config", security((), ("api_key" = [])),
    responses(
        (status = 200, description = "Get daemon config successfully", body = ConfigBody),
        (
            status = UNAUTHORIZED, description = "Authentication failed or not provided", body = ErrorMessage, 
            example = json!({"code": 401, "message": "Unauthorized"})
        )
    )
)]
pub async fn config_handler(_t: Token) -> Json<ConfigBody> {
    let timer = HTTP_REQ_HISTOGRAM.with_label_values(&["dump"]).start_timer();
    let config = config::read().runner;

    HTTP_COUNTER.inc();
    timer.observe_duration();

    Json(ConfigBody {
        shell: config.shell,
        args: config.args,
        log_path: config.log_path,
    })
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct NotificationConfig {
    enabled: bool,
    #[serde(default)]
    events: NotificationEvents,
    #[serde(default)]
    channels: Vec<String>,
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct NotificationEvents {
    #[serde(default)]
    agent_connect: bool,
    #[serde(default)]
    agent_disconnect: bool,
    #[serde(default)]
    process_start: bool,
    #[serde(default)]
    process_stop: bool,
    #[serde(default)]
    process_crash: bool,
    #[serde(default)]
    process_restart: bool,
}

impl Default for NotificationEvents {
    fn default() -> Self {
        Self {
            agent_connect: false,
            agent_disconnect: false,
            process_start: false,
            process_stop: false,
            process_crash: false,
            process_restart: false,
        }
    }
}

#[get("/daemon/config/notifications")]
#[utoipa::path(get, tag = "Daemon", path = "/daemon/config/notifications", security((), ("api_key" = [])),
    responses(
        (status = 200, description = "Get notification config successfully", body = NotificationConfig),
        (
            status = UNAUTHORIZED, description = "Authentication failed or not provided", body = ErrorMessage, 
            example = json!({"code": 401, "message": "Unauthorized"})
        )
    )
)]
pub async fn get_notifications_handler(_t: Token) -> Json<NotificationConfig> {
    let timer = HTTP_REQ_HISTOGRAM.with_label_values(&["get_notifications"]).start_timer();
    let config = config::read().daemon.notifications;

    HTTP_COUNTER.inc();
    timer.observe_duration();

    let notification_config = match config {
        Some(notif) => NotificationConfig {
            enabled: notif.enabled,
            events: NotificationEvents {
                agent_connect: notif.events.as_ref().map(|e| e.agent_connect).unwrap_or(false),
                agent_disconnect: notif.events.as_ref().map(|e| e.agent_disconnect).unwrap_or(false),
                process_start: notif.events.as_ref().map(|e| e.process_start).unwrap_or(false),
                process_stop: notif.events.as_ref().map(|e| e.process_stop).unwrap_or(false),
                process_crash: notif.events.as_ref().map(|e| e.process_crash).unwrap_or(false),
                process_restart: notif.events.as_ref().map(|e| e.process_restart).unwrap_or(false),
            },
            channels: notif.channels.unwrap_or_default(),
        },
        None => NotificationConfig {
            enabled: false,
            events: NotificationEvents::default(),
            channels: vec![],
        },
    };

    Json(notification_config)
}

#[post("/daemon/config/notifications", format = "json", data = "<body>")]
#[utoipa::path(post, tag = "Daemon", path = "/daemon/config/notifications", request_body = NotificationConfig,
    security((), ("api_key" = [])),
    responses(
        (status = 200, description = "Notification config saved successfully"),
        (
            status = UNAUTHORIZED, description = "Authentication failed or not provided", body = ErrorMessage, 
            example = json!({"code": 401, "message": "Unauthorized"})
        )
    )
)]
pub async fn save_notifications_handler(body: Json<NotificationConfig>, _t: Token) -> Result<Json<serde_json::Value>, GenericError> {
    let timer = HTTP_REQ_HISTOGRAM.with_label_values(&["save_notifications"]).start_timer();
    
    HTTP_COUNTER.inc();
    
    // Read current config
    let mut full_config = config::read();
    
    // Update notification config
    full_config.daemon.notifications = Some(config::structs::Notifications {
        enabled: body.enabled,
        events: Some(config::structs::NotificationEvents {
            agent_connect: body.events.agent_connect,
            agent_disconnect: body.events.agent_disconnect,
            process_start: body.events.process_start,
            process_stop: body.events.process_stop,
            process_crash: body.events.process_crash,
            process_restart: body.events.process_restart,
        }),
        channels: Some(body.channels.clone()),
    });
    
    // Save config to file
    let config_path = match home::home_dir() {
        Some(path) => format!("{}/.opm/config.toml", path.display()),
        None => return Err(generic_error(Status::InternalServerError, "Cannot determine home directory".to_string())),
    };
    
    let contents = match toml::to_string(&full_config) {
        Ok(contents) => contents,
        Err(err) => return Err(generic_error(Status::InternalServerError, format!("Cannot serialize config: {}", err))),
    };
    
    if let Err(err) = std::fs::write(&config_path, contents) {
        return Err(generic_error(Status::InternalServerError, format!("Cannot write config: {}", err)));
    }
    
    timer.observe_duration();
    Ok(Json(json!({"success": true, "message": "Notification settings saved"})))
}

#[derive(Deserialize, ToSchema)]
#[serde(crate = "rocket::serde")]
pub struct TestNotificationBody {
    title: String,
    message: String,
}

#[post("/daemon/test-notification", format = "json", data = "<body>")]
#[utoipa::path(post, tag = "Daemon", path = "/daemon/test-notification", request_body = TestNotificationBody,
    security((), ("api_key" = [])),
    responses(
        (status = 200, description = "Test notification sent successfully"),
        (
            status = UNAUTHORIZED, description = "Authentication failed or not provided", body = ErrorMessage, 
            example = json!({"code": 401, "message": "Unauthorized"})
        )
    )
)]
pub async fn test_notification_handler(body: Json<TestNotificationBody>, _t: Token) -> Result<Json<serde_json::Value>, GenericError> {
    let timer = HTTP_REQ_HISTOGRAM.with_label_values(&["test_notification"]).start_timer();
    
    HTTP_COUNTER.inc();
    
    // Get notification config
    let config = config::read().daemon.notifications;
    
    if let Some(cfg) = config {
        if !cfg.enabled {
            timer.observe_duration();
            return Err(generic_error(Status::BadRequest, "Notifications are not enabled".to_string()));
        }
        
        let mut desktop_sent = false;
        let mut channels_sent = false;
        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        
        // Try to send desktop notification (may fail in headless environments)
        match send_test_desktop_notification(&body.title, &body.message).await {
            Ok(_) => {
                desktop_sent = true;
            }
            Err(e) => {
                let error_msg = e.to_string();
                // Desktop notifications are expected to fail in headless environments
                // Treat as warning rather than error
                log::debug!("Desktop notification not available: {}", error_msg);
                warnings.push(format!("Desktop: {}", error_msg));
            }
        }
        
        // Send to external channels if configured
        if let Some(channels) = &cfg.channels {
            if !channels.is_empty() {
                match send_test_channel_notifications(&body.title, &body.message, channels).await {
                    Ok(_) => {
                        channels_sent = true;
                    }
                    Err(e) => {
                        log::warn!("Failed to send channel notifications: {}", e);
                        errors.push(format!("Channels: {}", e));
                    }
                }
            }
        }
        
        // Return success if at least one notification method succeeded
        if desktop_sent || channels_sent {
            let mut message = "Test notification sent successfully".to_string();
            let mut details = Vec::new();
            
            if desktop_sent {
                details.push("desktop");
            }
            if channels_sent {
                details.push("external channels");
            }
            
            if !details.is_empty() {
                message.push_str(" via ");
                message.push_str(&details.join(" and "));
            }
            
            // Include warnings if any (e.g., desktop failed but not critical)
            let response = if !warnings.is_empty() {
                json!({
                    "success": true, 
                    "message": message,
                    "warnings": warnings
                })
            } else {
                json!({
                    "success": true, 
                    "message": message
                })
            };
            
            timer.observe_duration();
            Ok(Json(response))
        } else {
            // All notification methods failed
            timer.observe_duration();
            
            // Build clear error message distinguishing expected vs unexpected failures
            let mut error_parts = Vec::new();
            
            if !warnings.is_empty() {
                error_parts.push(format!("Expected failures (headless environment): {}", warnings.join("; ")));
            }
            
            if !errors.is_empty() {
                error_parts.push(format!("Unexpected failures: {}", errors.join("; ")));
            }
            
            let error_msg = if error_parts.is_empty() {
                "No notification channels available".to_string()
            } else {
                format!("All notification methods failed. {}", error_parts.join(" | "))
            };
            
            Err(generic_error(Status::InternalServerError, error_msg))
        }
    } else {
        timer.observe_duration();
        Err(generic_error(Status::BadRequest, "Notifications are not configured".to_string()))
    }
}

async fn send_test_desktop_notification(
    title: &str,
    message: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    use notify_rust::{Notification, Urgency};
    
    Notification::new()
        .summary(title)
        .body(message)
        .urgency(Urgency::Normal)
        .appname("OPM")
        .timeout(5000)
        .show()?;
    
    Ok(())
}

async fn send_test_channel_notifications(
    title: &str,
    message: &str,
    channels: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    use reqwest::Client;
    
    let client = Client::new();
    let mut errors = Vec::new();
    let mut success_count = 0;
    
    for channel_url in channels {
        // Parse the shoutrrr URL to determine the service type
        if let Some((service, rest)) = channel_url.split_once("://") {
            let result = match service {
                "discord" => send_discord_webhook(&client, rest, title, message).await,
                "slack" => send_slack_webhook(&client, rest, title, message).await,
                "telegram" => send_telegram_message(&client, rest, title, message).await,
                _ => {
                    log::warn!("Unsupported notification service: {}", service);
                    errors.push(format!("Unsupported service: {}", service));
                    continue;
                }
            };
            
            match result {
                Ok(_) => success_count += 1,
                Err(e) => {
                    log::warn!("Failed to send to {}: {}", service, e);
                    errors.push(format!("{}: {}", service, e));
                }
            }
        } else {
            log::warn!("Invalid channel URL format: {}", channel_url);
            errors.push(format!("Invalid URL format: {}", channel_url));
        }
    }
    
    if success_count > 0 {
        Ok(())
    } else if !errors.is_empty() {
        Err(errors.join("; ").into())
    } else {
        Err("No valid notification channels configured".into())
    }
}

async fn send_discord_webhook(
    client: &reqwest::Client,
    webhook_data: &str,
    title: &str,
    message: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    // Discord webhook URL format: token@id or full webhook URL
    let webhook_url = if webhook_data.starts_with("http") {
        webhook_data.to_string()
    } else {
        // Parse token@id format (shoutrrr: discord://token@id)
        // Discord API expects: https://discord.com/api/webhooks/{id}/{token}
        if let Some((token, id)) = webhook_data.split_once('@') {
            format!("https://discord.com/api/webhooks/{}/{}", id, token)
        } else {
            return Err("Invalid Discord webhook format: expected 'token@id' or full webhook URL".into());
        }
    };
    
    let mut payload = HashMap::new();
    payload.insert("content", format!("**{}**\n{}", title, message));
    
    let response = client
        .post(&webhook_url)
        .json(&payload)
        .send()
        .await?;
    
    if !response.status().is_success() {
        let status = response.status();
        // Only read response body for error responses, and limit size to prevent issues
        let body = if status.is_client_error() || status.is_server_error() {
            response.text().await.unwrap_or_else(|_| "Unable to read response body".to_string())
        } else {
            "Non-success status but no error details available".to_string()
        };
        return Err(format!("Discord webhook failed with status: {} - Response: {}", status, body).into());
    }
    
    Ok(())
}

async fn send_slack_webhook(
    client: &reqwest::Client,
    webhook_data: &str,
    title: &str,
    message: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    // Slack webhook URL format: full webhook URL is required
    let webhook_url = if webhook_data.starts_with("http") {
        webhook_data.to_string()
    } else {
        return Err("Slack webhooks require full URL format (e.g., https://hooks.slack.com/services/T00000000/B00000000/XXXXXXXXXXXXXXXXXXXX)".into());
    };
    
    let mut payload = HashMap::new();
    payload.insert("text", format!("*{}*\n{}", title, message));
    
    let response = client
        .post(&webhook_url)
        .json(&payload)
        .send()
        .await?;
    
    if !response.status().is_success() {
        let status = response.status();
        // Only read response body for error responses, and limit size to prevent issues
        let body = if status.is_client_error() || status.is_server_error() {
            response.text().await.unwrap_or_else(|_| "Unable to read response body".to_string())
        } else {
            "Non-success status but no error details available".to_string()
        };
        return Err(format!("Slack webhook failed with status: {} - Response: {}", status, body).into());
    }
    
    Ok(())
}

async fn send_telegram_message(
    client: &reqwest::Client,
    webhook_data: &str,
    title: &str,
    message: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    // Telegram format: token@telegram?chats=@chat_id
    // Extract token and chat ID
    let (token, rest) = webhook_data
        .split_once('@')
        .ok_or("Invalid Telegram format: expected 'token@telegram?chats=@chat_id'")?;
    
    let chat_id = if let Some(query) = rest.strip_prefix("telegram?chats=") {
        query
    } else {
        return Err("Invalid Telegram format: expected 'token@telegram?chats=@chat_id'".into());
    };
    
    let api_url = format!("https://api.telegram.org/bot{}/sendMessage", token);
    let text = format!("<b>{}</b>\n{}", title, message);
    
    let mut payload = HashMap::new();
    payload.insert("chat_id", chat_id);
    payload.insert("text", &text);
    payload.insert("parse_mode", "HTML");
    
    let response = client
        .post(&api_url)
        .json(&payload)
        .send()
        .await?;
    
    if !response.status().is_success() {
        let status = response.status();
        // Only read response body for error responses, and limit size to prevent issues
        let body = if status.is_client_error() || status.is_server_error() {
            response.text().await.unwrap_or_else(|_| "Unable to read response body".to_string())
        } else {
            "Non-success status but no error details available".to_string()
        };
        return Err(format!("Telegram API failed with status: {} - Response: {}", status, body).into());
    }
    
    Ok(())
}

#[get("/list")]
#[utoipa::path(get, path = "/list", tag = "Process", security((), ("api_key" = [])),
    responses(
        (status = 200, description = "List processes successfully", body = [ProcessItem]),
        (
            status = UNAUTHORIZED, description = "Authentication failed or not provided", body = ErrorMessage, 
            example = json!({"code": 401, "message": "Unauthorized"})
        )
    )
)]
pub async fn list_handler(_t: Token) -> Json<Vec<ProcessItem>> {
    let timer = HTTP_REQ_HISTOGRAM.with_label_values(&["list"]).start_timer();
    let data = Runner::new().fetch();

    HTTP_COUNTER.inc();
    timer.observe_duration();

    Json(data)
}

#[get("/process/<id>/logs/<kind>")]
#[utoipa::path(get, tag = "Process", path = "/process/{id}/logs/{kind}", 
    security((), ("api_key" = [])),
    params(
        ("id" = usize, Path, description = "Process id to get logs for", example = 0),
        ("kind" = String, Path, description = "Log output type", example = "out")
    ),
    responses(
        (status = 200, description = "Process logs of {type} fetched", body = LogResponse),
        (status = NOT_FOUND, description = "Process was not found", body = ErrorMessage),
        (
            status = UNAUTHORIZED, description = "Authentication failed or not provided", body = ErrorMessage, 
            example = json!({"code": 401, "message": "Unauthorized"})
        )
    )
)]
pub async fn logs_handler(id: usize, kind: String, _t: Token) -> Result<Json<LogResponse>, NotFound> {
    let timer = HTTP_REQ_HISTOGRAM.with_label_values(&["log"]).start_timer();

    HTTP_COUNTER.inc();
    match Runner::new().info(id) {
        Some(item) => {
            let log_file = match kind.as_str() {
                "out" | "stdout" => item.logs().out,
                "error" | "stderr" => item.logs().error,
                _ => item.logs().out,
            };

            match File::open(log_file) {
                Ok(data) => {
                    let reader = BufReader::new(data);
                    let logs: Vec<String> = reader.lines().collect::<io::Result<_>>().unwrap();

                    timer.observe_duration();
                    Ok(Json(LogResponse { logs }))
                }
                Err(_) => Ok(Json(LogResponse { logs: vec![] })),
            }
        }
        None => {
            timer.observe_duration();
            Err(not_found("Process was not found"))
        }
    }
}

#[get("/process/<id>/logs/<kind>/raw")]
#[utoipa::path(get, tag = "Process", path = "/process/{id}/logs/{kind}/raw", 
    security((), ("api_key" = [])),
    params(
        ("id" = usize, Path, description = "Process id to get logs for", example = 0),
        ("kind" = String, Path, description = "Log output type", example = "out")
    ),
    responses(
        (
            description = "Process logs of {type} fetched raw", body = String, status = 200,
            example = json!("# PATH path/of/file.log\nserver started on port 3000")
        ),
        (status = NOT_FOUND, description = "Process was not found", body = ErrorMessage),
        (
            status = UNAUTHORIZED, description = "Authentication failed or not provided", body = ErrorMessage, 
            example = json!({"code": 401, "message": "Unauthorized"})
        )
    )
)]
pub async fn logs_raw_handler(id: usize, kind: String, _t: Token) -> Result<String, NotFound> {
    let timer = HTTP_REQ_HISTOGRAM.with_label_values(&["log"]).start_timer();

    HTTP_COUNTER.inc();
    match Runner::new().info(id) {
        Some(item) => {
            let log_file = match kind.as_str() {
                "out" | "stdout" => item.logs().out,
                "error" | "stderr" => item.logs().error,
                _ => item.logs().out,
            };

            let data = match fs::read_to_string(&log_file) {
                Ok(data) => format!("# PATH {log_file}\n{data}"),
                Err(err) => err.to_string(),
            };

            timer.observe_duration();
            Ok(data)
        }
        None => {
            timer.observe_duration();
            Err(not_found("Process was not found"))
        }
    }
}

#[get("/process/<id>/info")]
#[utoipa::path(get, tag = "Process", path = "/process/{id}/info", security((), ("api_key" = [])),
    params(("id" = usize, Path, description = "Process id to get information for", example = 0)),
    responses(
        (status = 200, description = "Current process info retrieved", body = ItemSingle),
        (status = NOT_FOUND, description = "Process was not found", body = ErrorMessage),
        (
            status = UNAUTHORIZED, description = "Authentication failed or not provided", body = ErrorMessage, 
            example = json!({"code": 401, "message": "Unauthorized"})
        )
    )
)]
pub async fn info_handler(id: usize, _t: Token) -> Result<Json<ItemSingle>, NotFound> {
    let timer = HTTP_REQ_HISTOGRAM.with_label_values(&["info"]).start_timer();
    let runner = Runner::new();

    if runner.exists(id) {
        let item = runner.get(id);
        HTTP_COUNTER.inc();
        timer.observe_duration();
        Ok(Json(item.fetch()))
    } else {
        Err(not_found("Process was not found"))
    }
}

#[post("/process/create", format = "json", data = "<body>")]
#[utoipa::path(post, tag = "Process", path = "/process/create", request_body(content = CreateBody), 
    security((), ("api_key" = [])),
    responses(
        (
            description = "Create process successful", body = ActionResponse,
            example = json!({"action": "create", "done": true }), status = 200,
        ),
        (status = INTERNAL_SERVER_ERROR, description = "Failed to create process", body = ErrorMessage),
        (
            status = UNAUTHORIZED, description = "Authentication failed or not provided", body = ErrorMessage, 
            example = json!({"code": 401, "message": "Unauthorized"})
        )
    )
)]
pub async fn create_handler(body: Json<CreateBody>, _t: Token) -> Result<Json<ActionResponse>, ()> {
    let timer = HTTP_REQ_HISTOGRAM.with_label_values(&["create"]).start_timer();
    let mut runner = Runner::new();

    HTTP_COUNTER.inc();

    let name = match &body.name {
        Some(name) => string!(name),
        None => string!(body.script.split_whitespace().next().unwrap_or_default()),
    };

    runner.start(&name, &body.script, body.path.clone(), &body.watch, 0).save();
    timer.observe_duration();

    Ok(Json(attempt(true, "create")))
}

#[post("/process/<id>/rename", format = "text", data = "<body>")]
#[utoipa::path(post, tag = "Process", path = "/process/{id}/rename", 
    security((), ("api_key" = [])),
    request_body(content = String, example = json!("example_name")), 
    params(("id" = usize, Path, description = "Process id to rename", example = 0)),
    responses(
        (
            description = "Rename process successful", body = ActionResponse,
            example = json!({"action": "rename", "done": true }), status = 200,
        ),
        (status = NOT_FOUND, description = "Process was not found", body = ErrorMessage),
        (
            status = UNAUTHORIZED, description = "Authentication failed or not provided", body = ErrorMessage, 
            example = json!({"code": 401, "message": "Unauthorized"})
        )
    )
)]
pub async fn rename_handler(id: usize, body: String, _t: Token) -> Result<Json<ActionResponse>, NotFound> {
    let timer = HTTP_REQ_HISTOGRAM.with_label_values(&["rename"]).start_timer();
    let runner = Runner::new();

    match runner.clone().info(id) {
        Some(process) => {
            HTTP_COUNTER.inc();
            let mut item = runner.clone().get(id);
            item.rename(body.trim().replace("\n", ""));
            then!(process.running, item.restart(true));  // API rename+restart should increment
            runner.save();  // Persist the renamed process to dump file
            timer.observe_duration();
            Ok(Json(attempt(true, "rename")))
        }
        None => {
            timer.observe_duration();
            Err(not_found("Process was not found"))
        }
    }
}

#[get("/process/<id>/env")]
#[utoipa::path(get, tag = "Process", path = "/process/{id}/env",
    params(("id" = usize, Path, description = "Process id to fetch env from", example = 0)),
    responses(
        (
            description = "Current process env", body = HashMap<String, String>,
            example = json!({"ENV_TEST_VALUE": "example_value"}), status = 200
        ),
        (status = NOT_FOUND, description = "Process was not found", body = ErrorMessage),
        (
            status = UNAUTHORIZED, description = "Authentication failed or not provided", body = ErrorMessage, 
            example = json!({"code": 401, "message": "Unauthorized"})
        )
    )
)]
pub async fn env_handler(id: usize, _t: Token) -> Result<EnvList, NotFound> {
    let timer = HTTP_REQ_HISTOGRAM.with_label_values(&["env"]).start_timer();

    HTTP_COUNTER.inc();
    match Runner::new().info(id) {
        Some(item) => {
            timer.observe_duration();
            Ok(Json(item.clone().env))
        }
        None => {
            timer.observe_duration();
            Err(not_found("Process was not found"))
        }
    }
}

#[post("/process/<id>/action", format = "json", data = "<body>")]
#[utoipa::path(post, tag = "Process", path = "/process/{id}/action", request_body = ActionBody,
    security((), ("api_key" = [])),
    params(("id" = usize, Path, description = "Process id to run action on", example = 0)),
    responses(
        (status = 200, description = "Run action on process successful", body = ActionResponse),
        (status = NOT_FOUND, description = "Process/action was not found", body = ErrorMessage),
        (
            status = UNAUTHORIZED, description = "Authentication failed or not provided", body = ErrorMessage, 
            example = json!({"code": 401, "message": "Unauthorized"})
        )
    )
)]
pub async fn action_handler(id: usize, body: Json<ActionBody>, _t: Token) -> Result<Json<ActionResponse>, NotFound> {
    let timer = HTTP_REQ_HISTOGRAM.with_label_values(&["action"]).start_timer();
    let mut runner = Runner::new();
    let method = body.method.as_str();

    if runner.exists(id) {
        HTTP_COUNTER.inc();
        match method {
            "start" => {
                let mut item = runner.get(id);
                item.restart(false);  // start should not increment
                item.get_runner().save();
                timer.observe_duration();
                Ok(Json(attempt(true, method)))
            }
            "restart" => {
                let mut item = runner.get(id);
                item.restart(true);  // restart should increment
                item.get_runner().save();
                timer.observe_duration();
                Ok(Json(attempt(true, method)))
            }
            "reload" => {
                let mut item = runner.get(id);
                item.reload(true);  // reload should increment
                item.get_runner().save();
                timer.observe_duration();
                Ok(Json(attempt(true, method)))
            }
            "stop" | "kill" => {
                let mut item = runner.get(id);
                item.stop();
                item.get_runner().save();
                timer.observe_duration();
                Ok(Json(attempt(true, method)))
            }
            "reset_env" | "clear_env" => {
                let mut item = runner.get(id);
                item.clear_env();
                item.get_runner().save();
                timer.observe_duration();
                Ok(Json(attempt(true, method)))
            }
            "remove" | "delete" => {
                runner.remove(id);
                timer.observe_duration();
                Ok(Json(attempt(true, method)))
            }
            "flush" | "clean" => {
                runner.flush(id);
                timer.observe_duration();
                Ok(Json(attempt(true, method)))
            }
            _ => {
                timer.observe_duration();
                Err(not_found("Invalid action attempt"))
            }
        }
    } else {
        Err(not_found("Process was not found"))
    }
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct BulkActionBody {
    #[schema(example = json!([0, 1, 2]))]
    ids: Vec<usize>,
    #[schema(example = "restart")]
    method: String,
}

#[derive(Serialize, ToSchema)]
pub struct BulkActionResponse {
    success: Vec<usize>,
    failed: Vec<usize>,
    action: String,
}

#[post("/process/bulk-action", format = "json", data = "<body>")]
#[utoipa::path(post, tag = "Process", path = "/process/bulk-action", request_body = BulkActionBody,
    security((), ("api_key" = [])),
    responses(
        (status = 200, description = "Run bulk action on processes", body = BulkActionResponse),
        (
            status = UNAUTHORIZED, description = "Authentication failed or not provided", body = ErrorMessage, 
            example = json!({"code": 401, "message": "Unauthorized"})
        )
    )
)]
pub async fn bulk_action_handler(body: Json<BulkActionBody>, _t: Token) -> Json<BulkActionResponse> {
    let timer = HTTP_REQ_HISTOGRAM.with_label_values(&["bulk_action"]).start_timer();
    let method = body.method.as_str();
    let mut success = Vec::new();
    let mut failed = Vec::new();

    HTTP_COUNTER.inc();
    
    for id in &body.ids {
        // Create a new runner for each iteration to avoid borrow checker issues
        let mut runner = Runner::new();
        
        if runner.exists(*id) {
            match method {
                "start" => {
                    let mut item = runner.get(*id);
                    item.restart(false);
                    item.get_runner().save();
                    success.push(*id);
                }
                "restart" => {
                    let mut item = runner.get(*id);
                    item.restart(true);
                    item.get_runner().save();
                    success.push(*id);
                }
                "reload" => {
                    let mut item = runner.get(*id);
                    item.reload(true);
                    item.get_runner().save();
                    success.push(*id);
                }
                "stop" | "kill" => {
                    let mut item = runner.get(*id);
                    item.stop();
                    item.get_runner().save();
                    success.push(*id);
                }
                "delete" | "remove" => {
                    runner.remove(*id);
                    success.push(*id);
                }
                _ => {
                    failed.push(*id);
                }
            }
        } else {
            failed.push(*id);
        }
    }

    timer.observe_duration();
    Json(BulkActionResponse {
        success,
        failed,
        action: method.to_string(),
    })
}

pub async fn get_metrics() -> MetricsRoot {
    let timer = HTTP_REQ_HISTOGRAM.with_label_values(&["metrics"]).start_timer();
    let os_info = crate::globals::get_os_info();

    let mut pid: Option<Pid> = None;
    let mut cpu_percent: Option<f64> = None;
    let mut uptime: Option<DateTime<Utc>> = None;
    let mut memory_usage: Option<u64> = None;
    let mut runner: Runner = file::read_object(global!("opm.dump"));

    HTTP_COUNTER.inc();
    if pid::exists() {
        if let Ok(process_id) = pid::read() {
            if let Ok(process) = Process::new(process_id.get()) {
                pid = Some(process_id);
                uptime = Some(pid::uptime().unwrap());
                if let Some(mem_info) = get_process_memory_with_children(process_id.get::<i64>()) {
                    memory_usage = Some(mem_info.rss);
                }
                cpu_percent = Some(get_process_cpu_usage_with_children_from_process(&process, process_id.get::<i64>()));
            }
        }
    }

    let memory_usage_fmt = match memory_usage {
        Some(usage) => helpers::format_memory(usage),
        None => string!("0b"),
    };

    let cpu_percent_fmt = match cpu_percent {
        Some(percent) => format!("{:.2}%", percent),
        None => string!("0.00%"),
    };

    let uptime_fmt = match uptime {
        Some(uptime) => helpers::format_duration(uptime),
        None => string!("none"),
    };

    timer.observe_duration();
    MetricsRoot {
        os: os_info.clone(),
        raw: Raw { memory_usage, cpu_percent },
        version: Version {
            target: env!("PROFILE").into(),
            build_date: env!("BUILD_DATE").into(),
            pkg: format!("v{}", env!("CARGO_PKG_VERSION")),
            hash: ternary!(env!("GIT_HASH_FULL") == "", None, Some(env!("GIT_HASH_FULL").into())),
        },
        daemon: Daemon {
            pid,
            uptime: uptime_fmt,
            running: pid::exists(),
            process_count: runner.count(),
            daemon_type: global!("opm.daemon.kind"),
            stats: Stats {
                memory_usage: memory_usage_fmt,
                cpu_percent: cpu_percent_fmt,
            },
        },
    }
}

#[get("/daemon/metrics")]
#[utoipa::path(get, tag = "Daemon", path = "/daemon/metrics", security((), ("api_key" = [])),
    responses(
        (status = 200, description = "Get daemon metrics", body = MetricsRoot),
        (
            status = UNAUTHORIZED, description = "Authentication failed or not provided", body = ErrorMessage, 
            example = json!({"code": 401, "message": "Unauthorized"})
        )
    )
)]
pub async fn metrics_handler(_t: Token) -> Json<MetricsRoot> { Json(get_metrics().await) }

#[get("/remote/<name>/metrics")]
#[utoipa::path(get, tag = "Remote", path = "/remote/{name}/metrics", security((), ("api_key" = [])),
    params(("name" = String, Path, description = "Name of remote daemon", example = "example")),
    responses(
        (status = 200, description = "Get remote metrics", body = MetricsRoot),
        (
            status = UNAUTHORIZED, description = "Authentication failed or not provided", body = ErrorMessage, 
            example = json!({"code": 401, "message": "Unauthorized"})
        )
    )
)]
pub async fn remote_metrics(name: String, _t: Token) -> Result<Json<MetricsRoot>, GenericError> {
    let timer = HTTP_REQ_HISTOGRAM.with_label_values(&["info"]).start_timer();

    if let Some(servers) = config::servers().servers {
        let (address, (client, headers)) = match servers.get(&name) {
            Some(server) => (&server.address, client(&server.token).await),
            None => return Err(generic_error(Status::NotFound, string!("Server was not found"))),
        };

        HTTP_COUNTER.inc();
        timer.observe_duration();

        match client.get(fmtstr!("{address}/daemon/metrics")).headers(headers).send().await {
            Ok(data) => {
                if data.status() != 200 {
                    let err = data.json::<ErrorMessage>().await.unwrap();
                    Err(generic_error(err.code, err.message))
                } else {
                    Ok(Json(data.json::<MetricsRoot>().await.unwrap()))
                }
            }
            Err(err) => Err(generic_error(Status::InternalServerError, err.to_string())),
        }
    } else {
        Err(generic_error(Status::BadRequest, string!("No servers have been added")))
    }
}

#[get("/live/daemon/<server>/metrics")]
pub async fn stream_metrics(server: String, _t: Token) -> EventStream![] {
    EventStream! {
        match config::servers().servers {
            Some(servers) => {
                let (address, (client, headers)) = match servers.get(&server) {
                    Some(server) => (&server.address, client(&server.token).await),
                    None => match &*server {
                        "local" | "internal" => loop {
                            let response = get_metrics().await;
                            yield Event::data(serde_json::to_string(&response).unwrap());
                            sleep(Duration::from_millis(500));
                        },
                        _ => return yield Event::data(format!("{{\"error\": \"server does not exist\"}}")),
                    }
                };

                loop {
                    match client.get(fmtstr!("{address}/daemon/metrics")).headers(headers.clone()).send().await {
                        Ok(data) => {
                            if data.status() != 200 {
                                break yield Event::data(data.text().await.unwrap());
                            } else {
                                yield Event::data(data.text().await.unwrap());
                                sleep(Duration::from_millis(1500));
                            }
                        }
                        Err(err) => break yield Event::data(format!("{{\"error\": \"{err}\"}}")),
                    }
                }
            }
            None => loop {
                let response = get_metrics().await;
                yield Event::data(serde_json::to_string(&response).unwrap());
                sleep(Duration::from_millis(500))
            },
        };
    }
}

#[get("/live/process/<server>/<id>")]
pub async fn stream_info(server: String, id: usize, _t: Token) -> EventStream![] {
    EventStream! {
        let runner = Runner::new();

        match config::servers().servers {
            Some(servers) => {
                let (address, (client, headers)) = match servers.get(&server) {
                    Some(server) => (&server.address, client(&server.token).await),
                    None => match &*server {
                        "local" | "internal" => loop {
                            let item = runner.refresh().get(id);
                            yield Event::data(serde_json::to_string(&item.fetch()).unwrap());
                            sleep(Duration::from_millis(1000));
                        },
                        _ => return yield Event::data(format!("{{\"error\": \"server does not exist\"}}")),
                    }
                };

                loop {
                    match client.get(fmtstr!("{address}/process/{id}/info")).headers(headers.clone()).send().await {
                        Ok(data) => {
                            if data.status() != 200 {
                                break yield Event::data(data.text().await.unwrap());
                            } else {
                                yield Event::data(data.text().await.unwrap());
                                sleep(Duration::from_millis(1500));
                            }
                        }
                        Err(err) => break yield Event::data(format!("{{\"error\": \"{err}\"}}")),
                    }
                }
            }
            None => loop {
                let item = runner.refresh().get(id);
                yield Event::data(serde_json::to_string(&item.fetch()).unwrap());
                sleep(Duration::from_millis(1000));
            }
        };
    }
}

// Agent Management Endpoints

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(crate = "rocket::serde")]
pub struct AgentRegisterBody {
    pub id: String,
    pub name: String,
    pub hostname: Option<String>,
}

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(crate = "rocket::serde")]
pub struct AgentHeartbeatBody {
    pub id: String,
}

/// Register a new agent
#[utoipa::path(
    post,
    path = "/daemon/agents/register",
    request_body = AgentRegisterBody,
    responses(
        (status = 200, description = "Agent registered successfully"),
        (status = 400, description = "Bad request")
    ),
    security(("api_key" = []))
)]
#[post("/daemon/agents/register", data = "<body>")]
pub async fn agent_register_handler(
    body: Json<AgentRegisterBody>,
    registry: &State<opm::agent::registry::AgentRegistry>,
    _t: Token,
) -> Result<Json<serde_json::Value>, NotFound> {
    let timer = HTTP_REQ_HISTOGRAM.with_label_values(&["agent_register"]).start_timer();
    HTTP_COUNTER.inc();

    let agent_info = opm::agent::types::AgentInfo {
        id: body.id.clone(),
        name: body.name.clone(),
        hostname: body.hostname.clone(),
        status: opm::agent::types::AgentStatus::Online,
        connection_type: opm::agent::types::ConnectionType::In,
        last_seen: std::time::SystemTime::now(),
        connected_at: std::time::SystemTime::now(),
    };

    registry.register(agent_info);
    timer.observe_duration();

    Ok(Json(json!({
        "success": true,
        "message": "Agent registered successfully"
    })))
}

/// Agent heartbeat
#[utoipa::path(
    post,
    path = "/daemon/agents/heartbeat",
    request_body = AgentHeartbeatBody,
    responses(
        (status = 200, description = "Heartbeat received"),
        (status = 404, description = "Agent not found")
    ),
    security(("api_key" = []))
)]
#[post("/daemon/agents/heartbeat", data = "<body>")]
pub async fn agent_heartbeat_handler(
    body: Json<AgentHeartbeatBody>,
    registry: &State<opm::agent::registry::AgentRegistry>,
    _t: Token,
) -> Result<Json<serde_json::Value>, NotFound> {
    let timer = HTTP_REQ_HISTOGRAM.with_label_values(&["agent_heartbeat"]).start_timer();
    HTTP_COUNTER.inc();

    registry.update_heartbeat(&body.id);
    timer.observe_duration();

    Ok(Json(json!({
        "success": true,
        "message": "Heartbeat received"
    })))
}

/// List all connected agents
#[utoipa::path(
    get,
    path = "/daemon/agents/list",
    responses(
        (status = 200, description = "List of connected agents"),
    ),
    security(("api_key" = []))
)]
#[get("/daemon/agents/list")]
pub async fn agent_list_handler(
    registry: &State<opm::agent::registry::AgentRegistry>,
    _t: Token,
) -> Result<Json<Vec<opm::agent::types::AgentInfo>>, NotFound> {
    let timer = HTTP_REQ_HISTOGRAM.with_label_values(&["agent_list"]).start_timer();
    HTTP_COUNTER.inc();

    let agents = registry.list();
    timer.observe_duration();

    Ok(Json(agents))
}

/// Unregister an agent
#[utoipa::path(
    delete,
    path = "/daemon/agents/{id}",
    params(
        ("id" = String, Path, description = "Agent ID")
    ),
    responses(
        (status = 200, description = "Agent unregistered successfully"),
        (status = 404, description = "Agent not found")
    ),
    security(("api_key" = []))
)]
#[delete("/daemon/agents/<id>")]
pub async fn agent_unregister_handler(
    id: String,
    registry: &State<opm::agent::registry::AgentRegistry>,
    _t: Token,
) -> Result<Json<serde_json::Value>, NotFound> {
    let timer = HTTP_REQ_HISTOGRAM.with_label_values(&["agent_unregister"]).start_timer();
    HTTP_COUNTER.inc();

    registry.unregister(&id);
    timer.observe_duration();

    Ok(Json(json!({
        "success": true,
        "message": "Agent unregistered successfully"
    })))
}
