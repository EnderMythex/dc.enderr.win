//! Shared state of the bot: REST client, database, caches and configuration.

use std::{
    collections::{HashMap, VecDeque},
    io,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
    time::{Instant, SystemTime},
};

use dc_core::guild::GuildConfig;
use serde_json::Value;
use sqlx::SqlitePool;
use twilight_gateway::MessageSender;

use crate::api::Api;

const PERM_ADMINISTRATOR: u64 = 1 << 3;
const MESSAGE_CACHE_SIZE: usize = 5000;

pub struct App {
    pub api: Api,
    /// Plain HTTP client, for downloading avatars.
    pub web: reqwest::Client,
    pub db: SqlitePool,
    /// The bot folder (`bots/<slug>`): configuration and data live here.
    pub dir: PathBuf,
    /// Panel owners of this bot (from `bot.toml`): admins on every server.
    pub owners: Vec<String>,
    /// The bot's name in `bot.toml`.
    pub name: String,
    /// The bot's folder name, its id on the panel.
    pub slug: String,
    /// Link to this bot's page on the dashboard, when configured.
    pub dashboard_url: Option<String>,
    /// Bot user id and application id, known once connected.
    pub identity: OnceLock<Identity>,
    pub gateway: Mutex<Option<MessageSender>>,
    pub guilds: Mutex<HashMap<String, GuildInfo>>,
    configs: Mutex<HashMap<String, CachedConfig>>,
    pub messages: Mutex<MessageCache>,
    pub xp_cooldowns: Mutex<HashMap<(String, String), Instant>>,
    pub captchas: Mutex<HashMap<(String, String), Captcha>>,
    /// Bans and kicks waiting for the moderator to confirm, by random id.
    pub confirmations: Mutex<HashMap<String, crate::moderation::Pending>>,
    /// Servers where a role is being given to or taken from everyone.
    pub mass_roles: Mutex<std::collections::HashSet<String>>,
    /// Giveaways whose entry count changed: their message is refreshed soon.
    pub giveaways_changed: Mutex<std::collections::HashSet<(String, i64)>>,
    /// `/announce` forms waiting to be filled in, by random id.
    pub drafts: Mutex<HashMap<String, crate::announce::Draft>>,
}

pub struct Identity {
    pub user_id: String,
    pub app_id: String,
}

#[derive(Debug, Clone, Default)]
pub struct GuildInfo {
    pub name: String,
    pub owner_id: String,
    pub member_count: u64,
    /// Role id -> permission bits.
    pub roles: HashMap<String, u64>,
    /// Role id -> name, for the images.
    pub role_names: HashMap<String, String>,
    pub channels: HashMap<String, ChannelInfo>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChannelInfo {
    pub name: String,
    pub topic: String,
}

struct CachedConfig {
    modified: Option<SystemTime>,
    config: Arc<GuildConfig>,
}

pub struct Captcha {
    pub code: String,
    pub expires: Instant,
    pub attempts: u32,
}

#[derive(Debug, Clone)]
pub struct CachedMessage {
    pub channel_id: String,
    pub author_id: String,
    pub content: String,
    pub attachments: usize,
    /// Messages of bots are cached only so their deletion is not logged.
    pub from_bot: bool,
}

/// Recent messages, so deleted and edited ones can be logged with their
/// original content (Discord only sends the id of a deleted message).
#[derive(Default)]
pub struct MessageCache {
    by_id: HashMap<String, CachedMessage>,
    order: VecDeque<String>,
}

impl MessageCache {
    pub fn insert(&mut self, id: String, message: CachedMessage) {
        if self.by_id.insert(id.clone(), message).is_none() {
            self.order.push_back(id);
            if self.order.len() > MESSAGE_CACHE_SIZE
                && let Some(oldest) = self.order.pop_front()
            {
                self.by_id.remove(&oldest);
            }
        }
    }

    pub fn get(&self, id: &str) -> Option<CachedMessage> {
        self.by_id.get(id).cloned()
    }

    pub fn remove(&mut self, id: &str) -> Option<CachedMessage> {
        self.by_id.remove(id)
    }
}

impl App {
    pub fn new(
        api: Api,
        db: SqlitePool,
        dir: PathBuf,
        owners: Vec<String>,
        (name, slug): (String, String),
        dashboard_url: Option<String>,
    ) -> Self {
        Self {
            api,
            web: reqwest::Client::new(),
            db,
            dir,
            owners,
            name,
            slug,
            dashboard_url,
            identity: OnceLock::new(),
            gateway: Mutex::default(),
            guilds: Mutex::default(),
            configs: Mutex::default(),
            messages: Mutex::default(),
            xp_cooldowns: Mutex::default(),
            captchas: Mutex::default(),
            confirmations: Mutex::default(),
            mass_roles: Mutex::default(),
            giveaways_changed: Mutex::default(),
            drafts: Mutex::default(),
        }
    }

    /// Address of the panel (`https://dc.enderr.win`), from `DASHBOARD_URL`.
    pub fn panel_origin(&self) -> Option<String> {
        let url = self.dashboard_url.as_deref()?;
        let scheme_end = url.find("://")? + 3;
        let host_end = url[scheme_end..]
            .find('/')
            .map_or(url.len(), |index| scheme_end + index);
        Some(url[..host_end].to_owned())
    }

    pub fn user_id(&self) -> &str {
        self.identity.get().map_or("", |i| i.user_id.as_str())
    }

    pub fn app_id(&self) -> &str {
        self.identity.get().map_or("", |i| i.app_id.as_str())
    }

    /// Configuration of a server, re-read from disk whenever the dashboard
    /// (or a `/settings` command) changed the file.
    pub fn config(&self, guild_id: &str) -> Arc<GuildConfig> {
        let modified = std::fs::metadata(GuildConfig::path(&self.dir, guild_id))
            .and_then(|meta| meta.modified())
            .ok();
        let mut configs = self.configs.lock().unwrap();
        if let Some(cached) = configs.get(guild_id)
            && cached.modified == modified
        {
            return cached.config.clone();
        }
        let config = match GuildConfig::load(&self.dir, guild_id) {
            Ok(config) => Arc::new(config),
            Err(error) => {
                tracing::warn!(guild = guild_id, %error, "unreadable server configuration");
                // Keep the last good configuration rather than resetting to defaults.
                match configs.get(guild_id) {
                    Some(cached) => cached.config.clone(),
                    None => Arc::default(),
                }
            }
        };
        configs.insert(
            guild_id.to_owned(),
            CachedConfig {
                modified,
                config: config.clone(),
            },
        );
        config
    }

    /// Applies a change to a server's configuration and saves it.
    pub fn update_config(
        &self,
        guild_id: &str,
        change: impl FnOnce(&mut GuildConfig),
    ) -> io::Result<Arc<GuildConfig>> {
        let mut config = GuildConfig::load(&self.dir, guild_id)?;
        change(&mut config);
        config.save(&self.dir, guild_id)?;
        Ok(self.config(guild_id))
    }

    /// `#name` of a channel, for the images.
    pub fn channel_name(&self, guild_id: &str, channel_id: &str) -> String {
        let guilds = self.guilds.lock().unwrap();
        match guilds
            .get(guild_id)
            .and_then(|g| g.channels.get(channel_id))
        {
            Some(channel) => format!("#{}", channel.name),
            None => "#deleted-channel".into(),
        }
    }

    /// `@name` of a role, for the images.
    pub fn role_name(&self, guild_id: &str, role_id: &str) -> String {
        let guilds = self.guilds.lock().unwrap();
        match guilds.get(guild_id).and_then(|g| g.role_names.get(role_id)) {
            Some(name) => format!("@{name}"),
            None => "@deleted-role".into(),
        }
    }

    pub fn guild(&self, guild_id: &str) -> GuildInfo {
        self.guilds
            .lock()
            .unwrap()
            .get(guild_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Downloads a user's avatar as PNG, for the image cards.
    pub async fn avatar_png(&self, user: &Value) -> Option<Vec<u8>> {
        let response = self
            .web
            .get(avatar_url(user, 256))
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            return None;
        }
        Some(response.bytes().await.ok()?.to_vec())
    }

    /// Server owner or panel owner of this bot: may manage the admin list.
    pub fn is_super_admin(&self, guild_id: &str, user_id: &str) -> bool {
        self.owners.iter().any(|owner| owner == user_id)
            || self
                .guilds
                .lock()
                .unwrap()
                .get(guild_id)
                .is_some_and(|guild| guild.owner_id == user_id)
    }

    /// May this member use moderation and settings commands?
    ///
    /// Yes for panel owners, the server owner, Discord administrators, and
    /// the users and roles added as admins on the dashboard or with `/ownerbot`.
    pub fn is_admin(&self, guild_id: &str, user_id: &str, roles: &[String]) -> bool {
        if self.is_super_admin(guild_id, user_id) {
            return true;
        }
        let config = self.config(guild_id);
        if config.admins.users.iter().any(|id| id == user_id)
            || config.admins.roles.iter().any(|role| roles.contains(role))
        {
            return true;
        }
        let guilds = self.guilds.lock().unwrap();
        let Some(guild) = guilds.get(guild_id) else {
            return false;
        };
        // The @everyone role has the id of the server.
        let permissions = roles
            .iter()
            .map(String::as_str)
            .chain([guild_id])
            .filter_map(|role| guild.roles.get(role))
            .fold(0, |acc, bits| acc | bits);
        permissions & PERM_ADMINISTRATOR != 0
    }
}

// ---------------------------------------------------------------- JSON helpers

pub fn str_of<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or("")
}

/// Discord sends permission sets as decimal strings.
pub fn bits_of(value: &Value) -> u64 {
    value.as_str().and_then(|s| s.parse().ok()).unwrap_or(0)
}

pub fn roles_of(member: &Value) -> Vec<String> {
    member["roles"]
        .as_array()
        .map(|roles| {
            roles
                .iter()
                .filter_map(|role| role.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// Server nickname, else global display name, else username.
pub fn display_name(user: &Value, member: &Value) -> String {
    member["nick"]
        .as_str()
        .or(user["global_name"].as_str())
        .or(user["username"].as_str())
        .unwrap_or("Unknown")
        .to_owned()
}

pub fn avatar_url(user: &Value, size: u32) -> String {
    let id = str_of(user, "id");
    match user["avatar"].as_str() {
        Some(hash) => format!("https://cdn.discordapp.com/avatars/{id}/{hash}.png?size={size}"),
        None => {
            let index = (id.parse::<u64>().unwrap_or(0) >> 22) % 6;
            format!("https://cdn.discordapp.com/embed/avatars/{index}.png")
        }
    }
}

// ---------------------------------------------------------------- time helpers

/// Unix seconds -> `2026-11-11T10:12:33Z` (what Discord expects for timeouts).
pub fn iso8601(timestamp: i64) -> String {
    let days = timestamp.div_euclid(86_400);
    let secs = timestamp.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        secs / 3600,
        secs % 3600 / 60,
        secs % 60
    )
}

/// Parses `90s`, `10m`, `2h`, `7d` or `1w` into seconds.
pub fn parse_duration(input: &str) -> Option<i64> {
    let input = input.trim().to_lowercase();
    let split = input.find(|c: char| !c.is_ascii_digit())?;
    let (digits, unit) = input.split_at(split);
    let value: i64 = digits.parse().ok()?;
    let factor = match unit.trim() {
        "s" | "sec" | "secs" => 1,
        "m" | "min" | "mins" => 60,
        "h" | "hour" | "hours" => 3600,
        "d" | "day" | "days" => 86_400,
        "w" | "week" | "weeks" => 604_800,
        _ => return None,
    };
    (value > 0).then(|| value.saturating_mul(factor))
}

/// 5400 -> "1 h 30 min".
pub fn format_duration(secs: i64) -> String {
    let (d, h, m, s) = (
        secs / 86_400,
        secs % 86_400 / 3600,
        secs % 3600 / 60,
        secs % 60,
    );
    let parts: Vec<String> = [(d, "d"), (h, "h"), (m, "min"), (s, "s")]
        .into_iter()
        .filter(|(value, _)| *value > 0)
        .map(|(value, unit)| format!("{value} {unit}"))
        .collect();
    if parts.is_empty() {
        "0 s".into()
    } else {
        parts.join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps() {
        assert_eq!(iso8601(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso8601(951_827_696), "2000-02-29T12:34:56Z");
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("10m"), Some(600));
        assert_eq!(parse_duration(" 2H "), Some(7200));
        assert_eq!(parse_duration("1 day"), Some(86_400));
        assert_eq!(parse_duration("0m"), None);
        assert_eq!(parse_duration("soon"), None);
        assert_eq!(parse_duration("10"), None);
        assert_eq!(format_duration(5400), "1 h 30 min");
        assert_eq!(format_duration(86_400), "1 d");
    }

    #[test]
    fn message_cache_evicts_oldest() {
        let mut cache = MessageCache::default();
        for i in 0..=MESSAGE_CACHE_SIZE {
            cache.insert(
                i.to_string(),
                CachedMessage {
                    channel_id: "1".into(),
                    author_id: "2".into(),
                    content: "x".into(),
                    attachments: 0,
                    from_bot: false,
                },
            );
        }
        assert!(cache.get("0").is_none());
        assert!(cache.get("1").is_some());
        assert!(cache.remove(&MESSAGE_CACHE_SIZE.to_string()).is_some());
    }

    #[test]
    fn avatars() {
        let user = serde_json::json!({ "id": "175928847299117063", "avatar": null });
        assert!(avatar_url(&user, 64).contains("/embed/avatars/"));
        let user = serde_json::json!({ "id": "1", "avatar": "abc" });
        assert_eq!(
            avatar_url(&user, 64),
            "https://cdn.discordapp.com/avatars/1/abc.png?size=64"
        );
    }
}
