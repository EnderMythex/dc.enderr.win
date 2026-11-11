//! Discord REST client. Payloads are plain JSON so every message can use
//! Components V2 without waiting on a library to model it.

use std::time::Duration;

use reqwest::{Method, multipart};
use serde_json::{Value, json};

use crate::ui::Msg;

const API: &str = "https://discord.com/api/v10";
const MAX_RETRIES: u32 = 3;
const MAX_RETRY_WAIT: f64 = 10.0;

#[derive(Debug)]
pub enum ApiError {
    /// Discord answered with an error status.
    Status {
        status: u16,
        code: u64,
        message: String,
    },
    Http(reqwest::Error),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Status {
                status,
                code,
                message,
            } => write!(f, "Discord {status} (code {code}): {message}"),
            Self::Http(error) => write!(f, "Discord unreachable: {error}"),
        }
    }
}

impl std::error::Error for ApiError {}

impl ApiError {
    /// Short explanation suitable for showing to a moderator.
    pub fn user_message(&self) -> String {
        match self {
            Self::Status { code: 50013, .. } | Self::Status { status: 403, .. } => {
                "I am missing permissions for that. Check my role's permissions and make sure \
                 my role is above the roles and members I have to manage."
                    .into()
            }
            Self::Status { status: 404, .. } => "That no longer exists on Discord.".into(),
            Self::Status { message, .. } => format!("Discord refused the action: {message}"),
            Self::Http(_) => "Discord could not be reached. Try again in a moment.".into(),
        }
    }

    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::Status { status: 404, .. })
    }
}

pub type ApiResult<T = Value> = Result<T, ApiError>;

#[derive(Clone)]
pub struct Api {
    http: reqwest::Client,
    auth: String,
}

impl Api {
    pub fn new(token: &str) -> Result<Self, reqwest::Error> {
        let http = reqwest::Client::builder()
            .user_agent(concat!(
                "DiscordBot (dc-bot, ",
                env!("CARGO_PKG_VERSION"),
                ")"
            ))
            .timeout(Duration::from_secs(20))
            .build()?;
        Ok(Self {
            http,
            auth: format!("Bot {token}"),
        })
    }

    pub async fn get(&self, path: &str) -> ApiResult {
        self.send(Method::GET, path, None, None, &[]).await
    }

    pub async fn post(&self, path: &str, body: &Value) -> ApiResult {
        self.send(Method::POST, path, Some(body), None, &[]).await
    }

    pub async fn patch(&self, path: &str, body: &Value, reason: Option<&str>) -> ApiResult {
        self.send(Method::PATCH, path, Some(body), reason, &[])
            .await
    }

    pub async fn put(&self, path: &str, body: &Value, reason: Option<&str>) -> ApiResult {
        self.send(Method::PUT, path, Some(body), reason, &[]).await
    }

    pub async fn delete(&self, path: &str, reason: Option<&str>) -> ApiResult {
        self.send(Method::DELETE, path, None, reason, &[]).await
    }

    /// Sends a request, as multipart when files are attached. Retries when
    /// Discord asks to slow down.
    pub async fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
        reason: Option<&str>,
        files: &[(String, Vec<u8>)],
    ) -> ApiResult {
        let mut attempt = 0;
        loop {
            let mut request = self
                .http
                .request(method.clone(), format!("{API}{path}"))
                .header("Authorization", &self.auth);
            if let Some(reason) = reason {
                request = request.header("X-Audit-Log-Reason", audit_reason(reason));
            }
            request = if files.is_empty() {
                match body {
                    Some(body) => request.json(body),
                    None => request,
                }
            } else {
                let mut form = multipart::Form::new().text(
                    "payload_json",
                    body.cloned().unwrap_or(Value::Null).to_string(),
                );
                for (index, (name, bytes)) in files.iter().enumerate() {
                    let part = multipart::Part::bytes(bytes.clone()).file_name(name.clone());
                    form = form.part(format!("files[{index}]"), part);
                }
                request.multipart(form)
            };

            let response = request.send().await.map_err(ApiError::Http)?;
            let status = response.status();
            let text = response.text().await.map_err(ApiError::Http)?;
            let value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);

            if status.as_u16() == 429 && attempt < MAX_RETRIES {
                attempt += 1;
                let wait = value["retry_after"].as_f64().unwrap_or(1.0);
                if wait <= MAX_RETRY_WAIT {
                    tokio::time::sleep(Duration::from_secs_f64(wait + 0.05)).await;
                    continue;
                }
            }
            if status.is_success() {
                return Ok(value);
            }
            return Err(ApiError::Status {
                status: status.as_u16(),
                code: value["code"].as_u64().unwrap_or(0),
                message: value["message"]
                    .as_str()
                    .unwrap_or("unknown error")
                    .to_owned(),
            });
        }
    }

    // ------------------------------------------------------------ messages

    pub async fn send_message(&self, channel: &str, msg: &Msg) -> ApiResult {
        let msg = msg.clone().finish().await;
        let path = format!("/channels/{channel}/messages");
        self.send(Method::POST, &path, Some(&msg.payload()), None, &msg.files)
            .await
    }

    pub async fn edit_message(&self, channel: &str, message: &str, msg: &Msg) -> ApiResult {
        let msg = msg.clone().finish().await;
        let path = format!("/channels/{channel}/messages/{message}");
        self.send(Method::PATCH, &path, Some(&msg.payload()), None, &msg.files)
            .await
    }

    pub async fn delete_message(&self, channel: &str, message: &str) -> ApiResult {
        self.delete(&format!("/channels/{channel}/messages/{message}"), None)
            .await
    }

    // ------------------------------------------------------------ members

    pub async fn add_role(&self, guild: &str, user: &str, role: &str, reason: &str) -> ApiResult {
        let path = format!("/guilds/{guild}/members/{user}/roles/{role}");
        self.put(&path, &json!({}), Some(reason)).await
    }

    pub async fn remove_role(
        &self,
        guild: &str,
        user: &str,
        role: &str,
        reason: &str,
    ) -> ApiResult {
        let path = format!("/guilds/{guild}/members/{user}/roles/{role}");
        self.delete(&path, Some(reason)).await
    }
}

/// Audit log reasons travel in an HTTP header: percent-encode them and keep
/// them within Discord's 512 character limit.
fn audit_reason(reason: &str) -> String {
    let mut out = String::new();
    for byte in reason.bytes() {
        if out.len() > 500 {
            break;
        }
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_reasons_are_header_safe() {
        assert_eq!(audit_reason("spam & ads"), "spam%20%26%20ads");
        assert_eq!(audit_reason("é"), "%C3%A9");
        assert!(audit_reason(&"x".repeat(2000)).len() <= 512);
    }
}
