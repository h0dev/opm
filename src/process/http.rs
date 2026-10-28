use crate::process::Remote;
use macros_rs::{fmtstr, string};
use anyhow::{bail, Context};
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT_ENCODING};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize)]
struct ActionBody {
    pub method: String,
}

#[derive(Deserialize)]
struct ActionResponse {
    pub done: bool,
}

/// Fail an action unless the remote reported a 2xx status and a completed action.
/// `daemon/api/routes.rs::action_handler` answers `{ "done": bool, "action": String }`.
fn action_response(
    status: StatusCode,
    body: &str,
    method: &str,
) -> Result<(), anyhow::Error> {
    if !status.is_success() {
        bail!("remote reported {status} for action `{method}`: {body}");
    }

    let response: ActionResponse = serde_json::from_str(body).with_context(|| {
        format!("remote returned an unreadable response for action `{method}`: {body}")
    })?;

    if !response.done {
        bail!("remote reported action `{method}` as failed: {body}");
    }

    Ok(())
}

/// Check the response to an action POST and fail when the remote did not do it.
pub fn check_action(response: sync::Response, method: &str) -> Result<(), anyhow::Error> {
    let status = response.status();
    let body = response.text()?;
    action_response(status, &body, method)
}

pub struct LogResponse {
    pub path: &'static str,
    pub lines: Vec<String>,
}

#[derive(Serialize)]
struct CreateBody<'c> {
    pub name: &'c String,
    pub script: &'c String,
    pub path: PathBuf,
    pub watch: &'c Option<String>,
}

pub mod sync {
    use reqwest::blocking::Client;
    use reqwest::header::{HeaderMap, HeaderValue, ACCEPT_ENCODING};

    pub use reqwest::blocking::Response;
    fn client_with_encoding(token: &Option<String>, encoding: &'static str) -> (Client, HeaderMap) {
        let client = Client::new();
        let mut headers = HeaderMap::new();

        headers.insert(ACCEPT_ENCODING, HeaderValue::from_static(encoding));

        if let Some(token) = token {
            headers.insert("token", HeaderValue::from_str(&token).unwrap());
        }

        return (client, headers);
    }

    pub fn client(token: &Option<String>) -> (Client, HeaderMap) {
        client_with_encoding(token, "gzip")
    }

    pub fn client_identity(token: &Option<String>) -> (Client, HeaderMap) {
        client_with_encoding(token, "identity")
    }
}

pub async fn client(token: &Option<String>) -> (Client, HeaderMap) {
    let client = Client::new();
    let mut headers = HeaderMap::new();

    headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("gzip"));

    if let Some(token) = token {
        headers.insert("token", HeaderValue::from_str(&token).unwrap());
    }

    return (client, headers);
}

pub fn info(
    Remote { address, token, .. }: &Remote,
    id: usize,
) -> Result<sync::Response, anyhow::Error> {
    let (client, headers) = sync::client(token);
    Ok(client
        .get(fmtstr!("{address}/process/{id}/info"))
        .headers(headers)
        .send()?)
}

pub fn logs(
    Remote { address, token, .. }: &Remote,
    id: usize,
    kind: &str,
) -> Result<LogResponse, anyhow::Error> {
    let (client, headers) = sync::client(token);
    let response = client
        .get(fmtstr!("{address}/process/{id}/logs/{kind}/raw"))
        .headers(headers)
        .send()?;
    let log = response.text()?;

    Ok(LogResponse {
        lines: log
            .lines()
            .skip(1)
            .map(|line| line.to_string())
            .collect::<Vec<String>>(),
        path: Box::leak(Box::from(
            log.lines()
                .next()
                .unwrap_or("")
                .split_whitespace()
                .last()
                .unwrap_or(""),
        )),
    })
}

pub fn create(
    Remote { address, token, .. }: &Remote,
    name: &String,
    script: &String,
    path: PathBuf,
    watch: &Option<String>,
) -> Result<sync::Response, anyhow::Error> {
    let (client, headers) = sync::client(token);
    let content = CreateBody {
        name,
        script,
        path,
        watch,
    };

    Ok(client
        .post(fmtstr!("{address}/process/create"))
        .json(&content)
        .headers(headers)
        .send()?)
}

pub fn restart(
    Remote { address, token, .. }: &Remote,
    id: usize,
) -> Result<(), anyhow::Error> {
    let (client, headers) = sync::client(token);
    let content = ActionBody {
        method: string!("restart"),
    };

    check_action(
        client
            .post(fmtstr!("{address}/process/{id}/action"))
            .json(&content)
            .headers(headers)
            .send()?,
        &content.method,
    )
}

pub fn reload(
    Remote { address, token, .. }: &Remote,
    id: usize,
) -> Result<(), anyhow::Error> {
    let (client, headers) = sync::client(token);
    let content = ActionBody {
        method: string!("reload"),
    };

    check_action(
        client
            .post(fmtstr!("{address}/process/{id}/action"))
            .json(&content)
            .headers(headers)
            .send()?,
        &content.method,
    )
}

pub fn rename(
    Remote { address, token, .. }: &Remote,
    id: usize,
    name: String,
) -> Result<sync::Response, anyhow::Error> {
    let (client, headers) = sync::client(token);
    Ok(client
        .post(fmtstr!("{address}/process/{id}/rename"))
        .body(name)
        .headers(headers)
        .send()?)
}

// merge into one function
pub fn stop(
    Remote { address, token, .. }: &Remote,
    id: usize,
) -> Result<(), anyhow::Error> {
    let (client, headers) = sync::client(token);
    let content = ActionBody {
        method: string!("stop"),
    };

    check_action(
        client
            .post(fmtstr!("{address}/process/{id}/action"))
            .json(&content)
            .headers(headers)
            .send()?,
        &content.method,
    )
}

pub fn remove(
    Remote { address, token, .. }: &Remote,
    id: usize,
) -> Result<(), anyhow::Error> {
    let (client, headers) = sync::client(token);
    let content = ActionBody {
        method: string!("remove"),
    };

    check_action(
        client
            .post(fmtstr!("{address}/process/{id}/action"))
            .json(&content)
            .headers(headers)
            .send()?,
        &content.method,
    )
}

pub fn flush(
    Remote { address, token, .. }: &Remote,
    id: usize,
) -> Result<(), anyhow::Error> {
    let (client, headers) = sync::client(token);
    let content = ActionBody {
        method: string!("flush"),
    };

    check_action(
        client
            .post(fmtstr!("{address}/process/{id}/action"))
            .json(&content)
            .headers(headers)
            .send()?,
        &content.method,
    )
}

pub fn clear_env(
    Remote { address, token, .. }: &Remote,
    id: usize,
) -> Result<(), anyhow::Error> {
    let (client, headers) = sync::client(token);
    let content = ActionBody {
        method: string!("clear_env"),
    };

    check_action(
        client
            .post(fmtstr!("{address}/process/{id}/action"))
            .json(&content)
            .headers(headers)
            .send()?,
        &content.method,
    )
}

/// Get list of connected agents from server
pub fn agent_list(address: &str, token: &Option<String>) -> Result<sync::Response, anyhow::Error> {
    let (client, headers) = sync::client(token);
    Ok(client
        .get(fmtstr!("{address}/daemon/agents/list"))
        .headers(headers)
        .send()?)
}

pub fn agent_list_identity(
    address: &str,
    token: &Option<String>,
) -> Result<sync::Response, anyhow::Error> {
    let (client, headers) = sync::client_identity(token);
    Ok(client
        .get(fmtstr!("{address}/daemon/agents/list"))
        .headers(headers)
        .send()?)
}

/// Get processes for a specific agent
pub fn agent_processes(
    address: &str,
    token: &Option<String>,
    agent_id: &str,
) -> Result<sync::Response, anyhow::Error> {
    let (client, headers) = sync::client(token);
    Ok(client
        .get(fmtstr!("{address}/daemon/agents/{agent_id}/processes"))
        .headers(headers)
        .send()?)
}

#[cfg(test)]
mod tests {
    use super::action_response;
    use reqwest::StatusCode;

    #[test]
    fn http_action_rejects_a_non_2xx_status() {
        let err = action_response(StatusCode::NOT_FOUND, r#"{"code":404,"message":"Process was not found"}"#, "remove")
            .expect_err("a 404 must not be reported as a successful action");

        assert!(
            err.to_string().contains("404"),
            "error should carry the status, got: {err}"
        );
    }

    #[test]
    fn http_action_rejects_a_500_status() {
        let err = action_response(StatusCode::INTERNAL_SERVER_ERROR, "boom", "stop")
            .expect_err("a 500 must not be reported as a successful action");

        assert!(err.to_string().contains("500"), "got: {err}");
    }

    #[test]
    fn http_action_rejects_a_body_that_reports_failure() {
        let err = action_response(
            StatusCode::OK,
            r#"{"done":false,"action":"DOES_NOT_EXIST"}"#,
            "remove",
        )
        .expect_err("a done:false body must not be reported as a successful action");

        assert!(err.to_string().contains("remove"), "got: {err}");
    }

    #[test]
    fn http_action_rejects_an_unparseable_success_body() {
        let err = action_response(StatusCode::OK, "not json", "start")
            .expect_err("an unreadable body must not be reported as a successful action");

        assert!(err.to_string().contains("start"), "got: {err}");
    }

    #[test]
    fn http_action_accepts_a_success_body() {
        action_response(StatusCode::OK, r#"{"done":true,"action":"remove"}"#, "remove")
            .expect("a done:true body is a successful action");
    }
}
