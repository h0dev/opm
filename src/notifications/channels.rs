use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// Note: uuid and urlencoding are used in Matrix and Matrix functions respectively
// These dependencies are declared in Cargo.toml

/// Notification channel configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum NotificationChannel {
    /// Discord webhook
    Discord {
        webhook_url: String,
    },
    /// Slack webhook
    Slack {
        webhook_url: String,
    },
    /// Telegram bot
    Telegram {
        bot_token: String,
        /// Numeric chat ID (e.g., -1001234567890 for channels/groups, 1234567890 for users)
        /// Get it from: https://api.telegram.org/bot<TOKEN>/getUpdates
        chat_id: String,
    },
    /// Gotify server
    Gotify {
        server_url: String,
        token: String,
        #[serde(default = "default_priority")]
        priority: u8,
    },
    /// Pushover
    Pushover {
        token: String,
        user_key: String,
        #[serde(default)]
        device: Option<String>,
    },
    /// Matrix homeserver
    Matrix {
        homeserver: String,
        room_id: String,
        access_token: String,
    },
    /// Ntfy.sh or self-hosted ntfy
    Ntfy {
        #[serde(default = "default_ntfy_server")]
        server_url: String,
        topic: String,
        #[serde(default)]
        token: Option<String>,
    },
    /// Mattermost webhook
    Mattermost {
        webhook_url: String,
        #[serde(default)]
        channel: Option<String>,
        #[serde(default)]
        username: Option<String>,
    },
    /// Generic webhook (POST JSON)
    Webhook {
        url: String,
        #[serde(default)]
        headers: Option<HashMap<String, String>>,
    },
}

fn default_priority() -> u8 {
    5
}

fn default_ntfy_server() -> String {
    "https://ntfy.sh".to_string()
}

impl NotificationChannel {
    pub async fn send(
        &self,
        client: &reqwest::Client,
        title: &str,
        message: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        match self {
            NotificationChannel::Discord { webhook_url } => {
                send_discord(client, webhook_url, title, message).await
            }
            NotificationChannel::Slack { webhook_url } => {
                send_slack(client, webhook_url, title, message).await
            }
            NotificationChannel::Telegram {
                bot_token,
                chat_id,
            } => send_telegram(client, bot_token, chat_id, title, message).await,
            NotificationChannel::Gotify {
                server_url,
                token,
                priority,
            } => send_gotify(client, server_url, token, *priority, title, message).await,
            NotificationChannel::Pushover {
                token,
                user_key,
                device,
            } => send_pushover(client, token, user_key, device.as_deref(), title, message).await,
            NotificationChannel::Matrix {
                homeserver,
                room_id,
                access_token,
            } => send_matrix(client, homeserver, room_id, access_token, title, message).await,
            NotificationChannel::Ntfy {
                server_url,
                topic,
                token,
            } => send_ntfy(client, server_url, topic, token.as_deref(), title, message).await,
            NotificationChannel::Mattermost {
                webhook_url,
                channel,
                username,
            } => {
                send_mattermost(
                    client,
                    webhook_url,
                    channel.as_deref(),
                    username.as_deref(),
                    title,
                    message,
                )
                .await
            }
            NotificationChannel::Webhook { url, headers } => {
                send_webhook(client, url, headers.as_ref(), title, message).await
            }
        }
    }
}

async fn send_discord(
    client: &reqwest::Client,
    webhook_url: &str,
    title: &str,
    message: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut payload = HashMap::new();
    payload.insert("content", format!("**{}**\n{}", title, message));

    let response = client.post(webhook_url).json(&payload).send().await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "Unable to read response".to_string());
        return Err(format!("Discord webhook failed ({}): {}", status, body).into());
    }

    Ok(())
}

async fn send_slack(
    client: &reqwest::Client,
    webhook_url: &str,
    title: &str,
    message: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut payload = HashMap::new();
    payload.insert("text", format!("*{}*\n{}", title, message));

    let response = client.post(webhook_url).json(&payload).send().await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "Unable to read response".to_string());
        return Err(format!("Slack webhook failed ({}): {}", status, body).into());
    }

    Ok(())
}

async fn send_telegram(
    client: &reqwest::Client,
    bot_token: &str,
    chat_id: &str,
    title: &str,
    message: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let api_url = format!("https://api.telegram.org/bot{}/sendMessage", bot_token);
    let text = format!("<b>{}</b>\n{}", title, message);
    let parse_mode = "HTML";

    let mut payload = HashMap::new();
    payload.insert("chat_id", chat_id);
    payload.insert("text", text.as_str());
    payload.insert("parse_mode", parse_mode);

    let response = client.post(&api_url).json(&payload).send().await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "Unable to read response".to_string());

        let hint = if body.contains("chat not found") {
            "\nHint: Ensure bot is added to chat/channel with admin rights. Use numeric chat_id (e.g., -1001234567890)."
        } else if body.contains("Unauthorized") {
            "\nHint: Check bot_token is correct."
        } else {
            ""
        };

        return Err(format!("Telegram API failed ({}): {}{}", status, body, hint).into());
    }

    Ok(())
}

async fn send_gotify(
    client: &reqwest::Client,
    server_url: &str,
    token: &str,
    priority: u8,
    title: &str,
    message: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let url = format!("{}/message", server_url.trim_end_matches('/'));
    let priority_str = priority.to_string();

    let mut payload = HashMap::new();
    payload.insert("title", title);
    payload.insert("message", message);
    payload.insert("priority", priority_str.as_str());

    // Use header for token to avoid it appearing in server logs
    let response = client
        .post(&url)
        .header("X-Gotify-Key", token)
        .json(&payload)
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "Unable to read response".to_string());
        return Err(format!("Gotify failed ({}): {}", status, body).into());
    }

    Ok(())
}

async fn send_pushover(
    client: &reqwest::Client,
    token: &str,
    user_key: &str,
    device: Option<&str>,
    title: &str,
    message: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let url = "https://api.pushover.net/1/messages.json";

    let mut payload = HashMap::new();
    payload.insert("token", token);
    payload.insert("user", user_key);
    payload.insert("title", title);
    payload.insert("message", message);
    if let Some(dev) = device {
        payload.insert("device", dev);
    }

    let response = client.post(url).form(&payload).send().await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "Unable to read response".to_string());
        return Err(format!("Pushover failed ({}): {}", status, body).into());
    }

    Ok(())
}

async fn send_matrix(
    client: &reqwest::Client,
    homeserver: &str,
    room_id: &str,
    access_token: &str,
    title: &str,
    message: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let txn_id = uuid::Uuid::new_v4();
    let url = format!(
        "{}/_matrix/client/r0/rooms/{}/send/m.room.message/{}",
        homeserver.trim_end_matches('/'),
        urlencoding::encode(room_id),
        txn_id
    );

    let body_text = format!("**{}**\n{}", title, message);
    let mut payload = HashMap::new();
    payload.insert("msgtype", "m.text");
    payload.insert("body", &body_text);

    let response = client
        .post(&url)
        .bearer_auth(access_token)
        .json(&payload)
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "Unable to read response".to_string());
        return Err(format!("Matrix failed ({}): {}", status, body).into());
    }

    Ok(())
}

async fn send_ntfy(
    client: &reqwest::Client,
    server_url: &str,
    topic: &str,
    token: Option<&str>,
    title: &str,
    message: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let url = format!("{}/{}", server_url.trim_end_matches('/'), topic);

    let mut request = client.post(&url).body(message.to_string());

    request = request.header("Title", title);
    if let Some(t) = token {
        request = request.bearer_auth(t);
    }

    let response = request.send().await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "Unable to read response".to_string());
        return Err(format!("Ntfy failed ({}): {}", status, body).into());
    }

    Ok(())
}

async fn send_mattermost(
    client: &reqwest::Client,
    webhook_url: &str,
    channel: Option<&str>,
    username: Option<&str>,
    title: &str,
    message: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut payload = HashMap::new();
    payload.insert("text", format!("**{}**\n{}", title, message));
    if let Some(ch) = channel {
        payload.insert("channel", ch.to_string());
    }
    if let Some(user) = username {
        payload.insert("username", user.to_string());
    }

    let response = client.post(webhook_url).json(&payload).send().await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "Unable to read response".to_string());
        return Err(format!("Mattermost webhook failed ({}): {}", status, body).into());
    }

    Ok(())
}

async fn send_webhook(
    client: &reqwest::Client,
    url: &str,
    headers: Option<&HashMap<String, String>>,
    title: &str,
    message: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut payload = HashMap::new();
    payload.insert("title", title);
    payload.insert("message", message);

    let mut request = client.post(url).json(&payload);

    if let Some(hdrs) = headers {
        for (key, value) in hdrs {
            request = request.header(key, value);
        }
    }

    let response = request.send().await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "Unable to read response".to_string());
        return Err(format!("Webhook failed ({}): {}", status, body).into());
    }

    Ok(())
}
