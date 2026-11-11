//! Minimal Discord API client: OAuth2 login and bot identity lookups.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use dc_core::crypto::sha256_hex;
use reqwest::{Response, StatusCode, Url};
use serde::Deserialize;

use crate::config::Config;

const DEFAULT_API: &str = "https://discord.com/api/v10";
const CDN: &str = "https://cdn.discordapp.com";
const IDENTITY_TTL: Duration = Duration::from_secs(600);

#[derive(Debug, thiserror::Error)]
pub enum DiscordError {
    #[error("credentials rejected by Discord")]
    Unauthorized,
    #[error("rate limited by Discord")]
    RateLimited,
    /// Discord refused a change and said why (name taken, changed too often...).
    #[error("{0}")]
    Rejected(String),
    #[error("unexpected response from Discord ({0})")]
    Status(StatusCode),
    #[error("Discord unreachable: {0}")]
    Http(#[from] reqwest::Error),
}

#[derive(Debug, Clone, Deserialize)]
pub struct DiscordUser {
    pub id: String,
    pub username: String,
    pub global_name: Option<String>,
    pub avatar: Option<String>,
    /// Profile banner (image hash), when set.
    #[serde(default)]
    pub banner: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Guild {
    pub id: String,
    pub name: String,
    pub icon: Option<String>,
    pub approximate_member_count: Option<u64>,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
}

type IdentityCache = HashMap<String, (Instant, Option<DiscordUser>)>;

#[derive(Clone)]
pub struct Discord {
    http: reqwest::Client,
    /// Bot identities keyed by the token hash: a new token invalidates the entry.
    identities: Arc<Mutex<IdentityCache>>,
    /// Base URL of the Discord API; `DISCORD_API_URL` points it at a mock in tests.
    api: String,
}

impl Discord {
    pub fn new() -> Result<Self, reqwest::Error> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("dc-panel/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(10))
            .build()?;
        Ok(Self {
            http,
            identities: Arc::default(),
            api: std::env::var("DISCORD_API_URL")
                .ok()
                .filter(|url| !url.is_empty())
                .unwrap_or_else(|| DEFAULT_API.to_owned()),
        })
    }

    /// Where to send the user to log in.
    pub fn authorize_url(cfg: &Config, state: &str) -> String {
        Url::parse_with_params(
            "https://discord.com/oauth2/authorize",
            [
                ("response_type", "code"),
                ("client_id", cfg.client_id.as_str()),
                ("scope", "identify"),
                ("redirect_uri", cfg.redirect_uri().as_str()),
                ("state", state),
                ("prompt", "none"),
            ],
        )
        .expect("valid base URL")
        .into()
    }

    /// Link to add a bot to a server.
    pub fn invite_url(bot_id: &str) -> String {
        Url::parse_with_params(
            "https://discord.com/oauth2/authorize",
            [
                ("client_id", bot_id),
                ("scope", "bot applications.commands"),
            ],
        )
        .expect("valid base URL")
        .into()
    }

    /// Exchanges the OAuth2 code of a panel login for a user access token.
    pub async fn exchange_code(&self, cfg: &Config, code: &str) -> Result<String, DiscordError> {
        let redirect_uri = cfg.redirect_uri();
        let form = [
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri.as_str()),
        ];
        self.token_request(&cfg.client_id, &cfg.client_secret, &form)
            .await
    }

    /// Same exchange for a code obtained inside a bot's Discord Activity,
    /// with that bot's own application credentials (no redirect involved).
    pub async fn exchange_activity_code(
        &self,
        client_id: &str,
        client_secret: &str,
        code: &str,
    ) -> Result<String, DiscordError> {
        let form = [("grant_type", "authorization_code"), ("code", code)];
        self.token_request(client_id, client_secret, &form).await
    }

    async fn token_request(
        &self,
        client_id: &str,
        client_secret: &str,
        form: &[(&str, &str)],
    ) -> Result<String, DiscordError> {
        let response = self
            .http
            .post(format!("{}/oauth2/token", self.api))
            .basic_auth(client_id, Some(client_secret))
            .form(form)
            .send()
            .await?;
        // Discord answers 400 when the code is invalid or already used.
        if response.status() == StatusCode::BAD_REQUEST {
            return Err(DiscordError::Unauthorized);
        }
        let token: TokenResponse = check(response)?.json().await?;
        Ok(token.access_token)
    }

    /// Profile of the user who just logged in.
    pub async fn oauth_user(&self, access_token: &str) -> Result<DiscordUser, DiscordError> {
        self.get("/users/@me", &format!("Bearer {access_token}"))
            .await
    }

    /// Discord account of a bot (id, name, avatar), cached for a few minutes.
    /// `None` when the token is missing or rejected.
    pub async fn bot_identity(&self, token: Option<&str>) -> Option<DiscordUser> {
        let token = token?;
        let key = sha256_hex(token);
        if let Some((at, cached)) = self.identities.lock().ok()?.get(&key)
            && at.elapsed() < IDENTITY_TTL
        {
            return cached.clone();
        }
        let fetched = match self.get("/users/@me", &format!("Bot {token}")).await {
            Ok(user) => Some(user),
            Err(DiscordError::Unauthorized) => None,
            // Transient failure: do not cache it.
            Err(error) => {
                tracing::warn!(%error, "bot identity lookup failed");
                return None;
            }
        };
        if let Ok(mut cache) = self.identities.lock() {
            cache.insert(key, (Instant::now(), fetched.clone()));
        }
        fetched
    }

    /// Changes the bot's account: `username`, `avatar` and `banner` (`data:`
    /// URIs, or null to remove). Discord allows two username changes per hour.
    pub async fn update_bot_profile(
        &self,
        token: &str,
        changes: serde_json::Map<String, serde_json::Value>,
    ) -> Result<DiscordUser, DiscordError> {
        let updated = self.patch(token, "/users/@me", changes).await?;
        let user: DiscordUser = serde_json::from_value(updated)
            .map_err(|_| DiscordError::Status(StatusCode::BAD_GATEWAY))?;
        if let Ok(mut cache) = self.identities.lock() {
            cache.insert(sha256_hex(token), (Instant::now(), Some(user.clone())));
        }
        Ok(user)
    }

    /// The bot's bio: the description of its application, shown in the
    /// "About me" of its profile. `None` when it cannot be read.
    pub async fn bot_bio(&self, token: &str) -> Option<String> {
        let application: serde_json::Value = self
            .get("/applications/@me", &format!("Bot {token}"))
            .await
            .map_err(|error| tracing::warn!(%error, "reading the bot's application failed"))
            .ok()?;
        Some(application["description"].as_str().unwrap_or("").to_owned())
    }

    pub async fn update_bot_bio(&self, token: &str, bio: &str) -> Result<(), DiscordError> {
        let mut body = serde_json::Map::new();
        body.insert("description".into(), bio.into());
        self.patch(token, "/applications/@me", body).await?;
        Ok(())
    }

    async fn patch(
        &self,
        token: &str,
        path: &str,
        body: serde_json::Map<String, serde_json::Value>,
    ) -> Result<serde_json::Value, DiscordError> {
        let response = self
            .http
            .patch(format!("{}{path}", self.api))
            .header(reqwest::header::AUTHORIZATION, format!("Bot {token}"))
            .json(&body)
            .send()
            .await?;
        if response.status() == StatusCode::BAD_REQUEST {
            let details: serde_json::Value = response.json().await.unwrap_or_default();
            return Err(DiscordError::Rejected(rejection_reason(&details)));
        }
        Ok(check(response)?.json().await?)
    }

    /// A user's avatar as PNG, for the image previews.
    pub async fn avatar_png(&self, user_id: &str, hash: Option<&str>) -> Option<Vec<u8>> {
        let url = match hash {
            Some(hash) => format!("{CDN}/avatars/{user_id}/{hash}.png?size=256"),
            None => {
                let index = (user_id.parse::<u64>().unwrap_or(0) >> 22) % 6;
                format!("{CDN}/embed/avatars/{index}.png")
            }
        };
        let response = self.http.get(url).send().await.ok()?;
        if !response.status().is_success() {
            return None;
        }
        Some(response.bytes().await.ok()?.to_vec())
    }

    /// Servers the bot is in (200 max).
    pub async fn bot_guilds(&self, token: &str) -> Result<Vec<Guild>, DiscordError> {
        self.get(
            "/users/@me/guilds?with_counts=true",
            &format!("Bot {token}"),
        )
        .await
    }

    async fn get<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        authorization: &str,
    ) -> Result<T, DiscordError> {
        let response = self
            .http
            .get(format!("{}{path}", self.api))
            .header(reqwest::header::AUTHORIZATION, authorization)
            .send()
            .await?;
        Ok(check(response)?.json().await?)
    }
}

/// The first human-readable reason in a Discord validation error, e.g.
/// `{"errors":{"username":{"_errors":[{"message":"..."}]}}}`.
fn rejection_reason(details: &serde_json::Value) -> String {
    fn first(value: &serde_json::Value) -> Option<&str> {
        match value {
            serde_json::Value::Object(map) => match map.get("_errors") {
                Some(errors) => errors[0]["message"].as_str(),
                None => map.values().find_map(first),
            },
            _ => None,
        }
    }
    first(&details["errors"])
        .or(details["message"].as_str())
        .unwrap_or("Discord refused the change.")
        .to_owned()
}

fn check(response: Response) -> Result<Response, DiscordError> {
    match response.status() {
        status if status.is_success() => Ok(response),
        StatusCode::UNAUTHORIZED => Err(DiscordError::Unauthorized),
        StatusCode::TOO_MANY_REQUESTS => Err(DiscordError::RateLimited),
        status => Err(DiscordError::Status(status)),
    }
}

// ---------------------------------------------------------------- server details

#[derive(Debug, Deserialize)]
pub struct GuildDetail {
    pub id: String,
    pub name: String,
    pub icon: Option<String>,
    #[serde(default)]
    pub approximate_member_count: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct Channel {
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// 0 text, 4 category, 5 announcement, others voice / forum / threads.
    #[serde(rename = "type")]
    pub kind: u8,
    #[serde(default)]
    pub position: i64,
}

impl Channel {
    pub fn is_text(&self) -> bool {
        matches!(self.kind, 0 | 5)
    }

    pub fn is_category(&self) -> bool {
        self.kind == 4
    }
}

#[derive(Debug, Deserialize)]
pub struct Role {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub position: i64,
    /// Roles owned by an integration (bots, boosts) cannot be assigned.
    #[serde(default)]
    pub managed: bool,
}

/// What the dashboard needs to configure a bot on one server.
pub struct GuildContext {
    pub guild: GuildDetail,
    pub channels: Vec<Channel>,
    /// Assignable roles, highest first, without @everyone.
    pub roles: Vec<Role>,
}

impl Discord {
    /// Name, icon and member count of a server the bot is in.
    pub async fn guild(&self, token: &str, guild_id: &str) -> Result<GuildDetail, DiscordError> {
        let path = format!("/guilds/{guild_id}?with_counts=true");
        self.get(&path, &format!("Bot {token}")).await
    }

    /// Loads a server through the bot's token. Fails with `Unauthorized` or a
    /// 403 / 404 status when the bot is not on that server.
    pub async fn guild_context(
        &self,
        token: &str,
        guild_id: &str,
    ) -> Result<GuildContext, DiscordError> {
        let auth = format!("Bot {token}");
        let base = format!("/guilds/{guild_id}");
        let (channels_path, roles_path) = (format!("{base}/channels"), format!("{base}/roles"));
        let (guild, channels, roles) = tokio::try_join!(
            self.guild(token, guild_id),
            self.get::<Vec<Channel>>(&channels_path, &auth),
            self.get::<Vec<Role>>(&roles_path, &auth),
        )?;
        let mut channels = channels;
        channels.sort_by_key(|channel| channel.position);
        let mut roles: Vec<Role> = roles
            .into_iter()
            .filter(|role| role.id != guild_id && !role.managed)
            .collect();
        roles.sort_by_key(|role| std::cmp::Reverse(role.position));
        Ok(GuildContext {
            guild,
            channels,
            roles,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejection_reasons() {
        let nested = serde_json::json!({
            "code": 50035, "message": "Invalid Form Body",
            "errors": { "username": { "_errors": [{ "code": "USERNAME_RATE_LIMIT", "message": "Too fast." }] } }
        });
        assert_eq!(rejection_reason(&nested), "Too fast.");
        let flat = serde_json::json!({ "message": "Nope" });
        assert_eq!(rejection_reason(&flat), "Nope");
        assert_eq!(
            rejection_reason(&serde_json::Value::Null),
            "Discord refused the change."
        );
    }
}
