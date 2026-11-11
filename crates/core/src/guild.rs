//! Per-server configuration of a bot, stored in
//! `bots/<slug>/guilds/<guild_id>.json`.
//!
//! The panel writes it from the dashboard, the bot writes it from its
//! `/settings` commands, and the bot re-reads it whenever the file changes.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::is_snowflake;

pub const DIR_NAME: &str = "guilds";

/// `captcha` is an image captcha inside Discord; `activity` shows the
/// captcha in a Discord Activity and `web` on a page of the panel, reached
/// with a signed link (see the README).
pub const VERIFY_MODES: &[&str] = &["none", "button", "colors", "captcha", "activity", "web"];
/// Where the link lock applies: every channel but the exempt ones, or only
/// the listed ones.
pub const LINK_SCOPES: &[&str] = &["all", "listed"];
pub const MAX_ROLE_BUTTONS: usize = 10;
pub const MAX_DM_LINKS: usize = 3;
pub const MAX_TWITCH_LOGINS: usize = 10;
const MAX_CONTEXT: usize = 12_000;
/// Captcha of the Activity and of the web page: type a code, or slide a
/// puzzle piece into place.
pub const CAPTCHA_STYLES: &[&str] = &["text", "slide"];
pub const ANNOUNCE_MODES: &[&str] = &["off", "same", "channel"];

const MAX_IDS: usize = 50;
const MAX_TEXT: usize = 1800;
const MAX_TITLE: usize = 80;
pub const MAX_REWARDS: usize = 10;
pub const MAX_TOPICS: usize = 10;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GuildConfig {
    pub admins: Admins,
    pub lock_url: LockUrl,
    pub welcome: Welcome,
    pub rules: Rules,
    pub tickets: Tickets,
    pub xp: Xp,
    pub logs: Logs,
    pub anti_scam: AntiScam,
    pub role_panel: RolePanel,
    pub assistant: Assistant,
    pub twitch: Twitch,
    pub reminders: Reminders,
}

/// Who may use the moderation and settings commands, in addition to the
/// server owner, Discord administrators and the bot's panel owners.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Admins {
    pub users: Vec<String>,
    pub roles: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LockUrl {
    pub enabled: bool,
    pub exempt_roles: Vec<String>,
    pub exempt_channels: Vec<String>,
    /// Domains that stay allowed, e.g. `youtube.com`.
    pub allowed_domains: Vec<String>,
    /// One of [`LINK_SCOPES`].
    pub scope: String,
    /// The channels watched when `scope` is `listed`.
    pub channels: Vec<String>,
    /// Channels where x.com / twitter.com links are blocked too, even if
    /// allowed elsewhere.
    pub x_blocked_channels: Vec<String>,
    /// Members whose link is removed can send it to the staff, who accept
    /// (the bot posts it) or refuse it in this channel.
    pub review_channel: Option<String>,
    /// Seconds a member is timed out for posting a blocked link (0: none).
    pub timeout_secs: u32,
}

/// Automatic protection against scams. Sanctions never touch bot admins.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AntiScam {
    /// Discord invite links (even disguised): message deleted, author kicked.
    pub invites: bool,
    /// The recurring "join an insider, text me" pitch: deleted, author banned.
    pub insider: bool,
    /// Pictures checked by AI for "I make you rich, DM me" recruiting ads:
    /// deleted, author banned. Needs the bot's Gemini key.
    pub images: bool,
    /// Where the staff is told about scam images and failed sanctions, with
    /// an Unban button.
    pub staff_channel: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleButton {
    pub role: String,
    pub label: String,
    #[serde(default)]
    pub emoji: String,
    #[serde(default)]
    pub description: String,
}

/// A panel of buttons members click to take or drop roles.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RolePanel {
    pub enabled: bool,
    pub channel: Option<String>,
    pub title: String,
    pub buttons: Vec<RoleButton>,
}

impl Default for RolePanel {
    fn default() -> Self {
        Self {
            enabled: false,
            channel: None,
            title: "Pick your roles".into(),
            buttons: Vec::new(),
        }
    }
}

/// AI answers to members who mention the bot (Gemini).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Assistant {
    pub enabled: bool,
    /// Knowledge and tone given to the AI; empty: the bot's default context.
    pub context: String,
    /// Channels where it answers; empty: everywhere.
    pub channels: Vec<String>,
    /// Longer questions are refused, to protect the API quota.
    pub max_length: u32,
    /// Seconds between two questions of one member.
    pub cooldown_secs: u32,
}

impl Default for Assistant {
    fn default() -> Self {
        Self {
            enabled: false,
            context: String::new(),
            channels: Vec::new(),
            max_length: 600,
            cooldown_secs: 8,
        }
    }
}

/// Announces when Twitch streamers go live.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Twitch {
    pub enabled: bool,
    pub channel: Option<String>,
    /// Role mentioned with each alert.
    pub role: Option<String>,
    /// Twitch user names, e.g. `dh_trenches`.
    pub logins: Vec<String>,
}

/// A safety reminder about scams, posted regularly in French and English.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Reminders {
    pub enabled: bool,
    pub channel_fr: Option<String>,
    pub channel_en: Option<String>,
    pub interval_hours: u32,
}

impl Default for Reminders {
    fn default() -> Self {
        Self {
            enabled: false,
            channel_fr: None,
            channel_en: None,
            interval_hours: 48,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkButton {
    pub label: String,
    pub url: String,
}

/// `Label | https://url` -> a link button; `None` when malformed.
pub fn parse_link_button(line: &str) -> Option<LinkButton> {
    let (label, url) = line.split_once('|')?;
    let (label, url) = (label.trim(), url.trim());
    let url_ok =
        url.starts_with("https://") && url.len() <= 512 && !url.contains(char::is_whitespace);
    (!label.is_empty() && url_ok).then(|| LinkButton {
        label: label.chars().take(80).collect(),
        url: url.to_owned(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Welcome {
    pub enabled: bool,
    pub channel: Option<String>,
    /// Placeholders: `{user}`, `{name}`, `{server}`, `{count}`.
    pub message: String,
    pub image: bool,
    pub auto_roles: Vec<String>,
    /// Private message sent to each new member (with safety tips).
    pub dm_enabled: bool,
    pub dm_text: String,
    /// Optional English version: the DM then has a language button.
    pub dm_text_en: String,
    /// Adds the anti-scam tips and the "turn off your DMs" picture.
    pub dm_safety: bool,
    pub dm_links: Vec<LinkButton>,
    /// Channels where a new member is mentioned, the mention being deleted
    /// after [`Welcome::ping_seconds`]: it draws their attention to those
    /// channels. Empty: off. Works even when the welcome message is off.
    pub ping_channels: Vec<String>,
    pub ping_seconds: u32,
}

/// Most channels a new member can be mentioned in.
pub const MAX_PING_CHANNELS: usize = 10;

impl Default for Welcome {
    fn default() -> Self {
        Self {
            enabled: false,
            channel: None,
            message: "Welcome {user} to **{server}**! You are member #{count}.".into(),
            image: true,
            auto_roles: Vec::new(),
            ping_channels: Vec::new(),
            ping_seconds: 3,
            dm_enabled: false,
            dm_text: "Welcome to **{server}**! Glad to have you with us.".into(),
            dm_text_en: String::new(),
            dm_safety: true,
            dm_links: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Rules {
    pub enabled: bool,
    pub channel: Option<String>,
    pub title: String,
    pub text: String,
    /// One of [`VERIFY_MODES`].
    pub verification: String,
    pub verified_role: Option<String>,
    /// Role taken away once verified, e.g. an "Unverified" role.
    pub remove_role: Option<String>,
    /// One of [`CAPTCHA_STYLES`], for the `activity` and `web` modes.
    pub captcha_style: String,
}

impl Default for Rules {
    fn default() -> Self {
        Self {
            enabled: false,
            channel: None,
            title: "Server rules".into(),
            text: "1. Be respectful to everyone.\n2. No spam or self-promotion.\n\
                   3. Follow Discord's Terms of Service."
                .into(),
            verification: "button".into(),
            verified_role: None,
            remove_role: None,
            captcha_style: "text".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tickets {
    pub enabled: bool,
    pub panel_channel: Option<String>,
    /// Category the ticket channels are created in.
    pub category: Option<String>,
    pub staff_roles: Vec<String>,
    /// Where transcripts of closed tickets are sent.
    pub log_channel: Option<String>,
    pub title: String,
    pub text: String,
    /// Optional topics offered in a menu when opening a ticket.
    pub topics: Vec<String>,
    pub max_open: u32,
}

impl Default for Tickets {
    fn default() -> Self {
        Self {
            enabled: false,
            panel_channel: None,
            category: None,
            staff_roles: Vec::new(),
            log_channel: None,
            title: "Support".into(),
            text: "Need help? Open a ticket and the staff will answer you in a private channel."
                .into(),
            topics: Vec::new(),
            max_open: 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reward {
    pub level: u32,
    pub role: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Xp {
    pub enabled: bool,
    pub min_xp: u32,
    pub max_xp: u32,
    pub cooldown_secs: u32,
    /// One of [`ANNOUNCE_MODES`]: where level-ups are announced.
    pub announce: String,
    pub announce_channel: Option<String>,
    pub ignored_channels: Vec<String>,
    pub rewards: Vec<Reward>,
}

impl Default for Xp {
    fn default() -> Self {
        Self {
            enabled: false,
            min_xp: 15,
            max_xp: 25,
            cooldown_secs: 60,
            announce: "same".into(),
            announce_channel: None,
            ignored_channels: Vec::new(),
            rewards: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Logs {
    pub enabled: bool,
    pub channel: Option<String>,
    pub message_delete: bool,
    pub message_edit: bool,
    pub channels: bool,
    pub members: bool,
    pub moderation: bool,
}

impl Default for Logs {
    fn default() -> Self {
        Self {
            enabled: false,
            channel: None,
            message_delete: true,
            message_edit: true,
            channels: true,
            members: true,
            moderation: true,
        }
    }
}

fn clean_ids(ids: &mut Vec<String>) {
    ids.iter_mut().for_each(|id| *id = id.trim().to_owned());
    ids.retain(|id| is_snowflake(id));
    ids.sort();
    ids.dedup();
    ids.truncate(MAX_IDS);
}

fn clean_id(id: &mut Option<String>) {
    *id = id
        .take()
        .map(|v| v.trim().to_owned())
        .filter(|v| is_snowflake(v));
}

fn clean_text(text: &mut String, max: usize, fallback: &str) {
    let trimmed: String = text.trim().chars().take(max).collect();
    *text = if trimmed.is_empty() {
        fallback.to_owned()
    } else {
        trimmed
    };
}

fn clean_choice(value: &mut String, allowed: &[&str], fallback: &str) {
    if !allowed.contains(&value.as_str()) {
        *value = fallback.to_owned();
    }
}

impl GuildConfig {
    /// Brings any input (dashboard form, hand-edited file) back into valid
    /// bounds: bad ids are dropped, texts truncated, numbers clamped.
    pub fn sanitize(&mut self) {
        clean_ids(&mut self.admins.users);
        clean_ids(&mut self.admins.roles);

        clean_ids(&mut self.lock_url.exempt_roles);
        clean_ids(&mut self.lock_url.exempt_channels);
        let domains = &mut self.lock_url.allowed_domains;
        domains.iter_mut().for_each(|d| *d = normalize_domain(d));
        domains.retain(|d| is_domain(d));
        domains.sort();
        domains.dedup();
        domains.truncate(MAX_IDS);

        let defaults = Self::default();
        clean_id(&mut self.welcome.channel);
        clean_text(
            &mut self.welcome.message,
            MAX_TEXT,
            &defaults.welcome.message,
        );
        clean_ids(&mut self.welcome.auto_roles);
        clean_ids(&mut self.welcome.ping_channels);
        self.welcome.ping_channels.truncate(MAX_PING_CHANNELS);
        self.welcome.ping_seconds = self.welcome.ping_seconds.clamp(1, 60);

        clean_id(&mut self.rules.channel);
        clean_id(&mut self.rules.verified_role);
        clean_id(&mut self.rules.remove_role);
        // Giving and removing the same role would undo the verification.
        if self.rules.remove_role == self.rules.verified_role {
            self.rules.remove_role = None;
        }
        clean_text(&mut self.rules.title, MAX_TITLE, &defaults.rules.title);
        clean_text(&mut self.rules.text, MAX_TEXT, &defaults.rules.text);
        clean_choice(&mut self.rules.verification, VERIFY_MODES, "button");
        clean_choice(&mut self.rules.captcha_style, CAPTCHA_STYLES, "text");

        clean_id(&mut self.tickets.panel_channel);
        clean_id(&mut self.tickets.category);
        clean_id(&mut self.tickets.log_channel);
        clean_ids(&mut self.tickets.staff_roles);
        clean_text(&mut self.tickets.title, MAX_TITLE, &defaults.tickets.title);
        clean_text(&mut self.tickets.text, MAX_TEXT, &defaults.tickets.text);
        let topics = &mut self.tickets.topics;
        topics
            .iter_mut()
            .for_each(|t| *t = t.trim().chars().take(MAX_TITLE).collect());
        topics.retain(|t| !t.is_empty());
        topics.dedup();
        topics.truncate(MAX_TOPICS);
        self.tickets.max_open = self.tickets.max_open.clamp(1, 5);

        self.xp.min_xp = self.xp.min_xp.clamp(1, 500);
        self.xp.max_xp = self.xp.max_xp.clamp(self.xp.min_xp, 500);
        self.xp.cooldown_secs = self.xp.cooldown_secs.min(3600);
        clean_choice(&mut self.xp.announce, ANNOUNCE_MODES, "same");
        clean_id(&mut self.xp.announce_channel);
        clean_ids(&mut self.xp.ignored_channels);
        let rewards = &mut self.xp.rewards;
        rewards.retain(|r| (1..=500).contains(&r.level) && is_snowflake(&r.role));
        rewards.sort_by_key(|r| r.level);
        rewards.dedup_by_key(|r| r.level);
        rewards.truncate(MAX_REWARDS);

        clean_id(&mut self.logs.channel);

        let welcome = &mut self.welcome;
        clean_text(&mut welcome.dm_text, MAX_TEXT, &defaults.welcome.dm_text);
        welcome.dm_text_en = welcome.dm_text_en.trim().chars().take(MAX_TEXT).collect();
        welcome
            .dm_links
            .retain(|link| parse_link_button(&format!("{}|{}", link.label, link.url)).is_some());
        welcome.dm_links.truncate(MAX_DM_LINKS);

        let links = &mut self.lock_url;
        clean_choice(&mut links.scope, LINK_SCOPES, "all");
        clean_ids(&mut links.channels);
        clean_ids(&mut links.x_blocked_channels);
        clean_id(&mut links.review_channel);
        links.timeout_secs = links.timeout_secs.min(3600);

        clean_id(&mut self.anti_scam.staff_channel);

        let panel = &mut self.role_panel;
        clean_id(&mut panel.channel);
        clean_text(&mut panel.title, MAX_TITLE, &defaults.role_panel.title);
        panel
            .buttons
            .retain(|b| is_snowflake(&b.role) && !b.label.trim().is_empty());
        for button in &mut panel.buttons {
            button.label = button.label.trim().chars().take(80).collect();
            button.emoji = button.emoji.trim().chars().take(8).collect();
            button.description = button.description.trim().chars().take(200).collect();
        }
        panel.buttons.dedup_by(|a, b| a.role == b.role);
        panel.buttons.truncate(MAX_ROLE_BUTTONS);

        let assistant = &mut self.assistant;
        assistant.context = assistant.context.trim().chars().take(MAX_CONTEXT).collect();
        clean_ids(&mut assistant.channels);
        assistant.max_length = assistant.max_length.clamp(50, 2000);
        assistant.cooldown_secs = assistant.cooldown_secs.clamp(1, 600);

        let twitch = &mut self.twitch;
        clean_id(&mut twitch.channel);
        clean_id(&mut twitch.role);
        for login in &mut twitch.logins {
            *login = login
                .trim()
                .trim_start_matches("https://twitch.tv/")
                .trim_start_matches("https://www.twitch.tv/")
                .to_lowercase();
        }
        twitch.logins.retain(|login| {
            (3..=25).contains(&login.len())
                && login
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        });
        twitch.logins.sort();
        twitch.logins.dedup();
        twitch.logins.truncate(MAX_TWITCH_LOGINS);

        let reminders = &mut self.reminders;
        clean_id(&mut reminders.channel_fr);
        clean_id(&mut reminders.channel_en);
        reminders.interval_hours = reminders.interval_hours.clamp(6, 24 * 14);
    }

    pub fn path(bot_dir: &Path, guild_id: &str) -> PathBuf {
        bot_dir.join(DIR_NAME).join(format!("{guild_id}.json"))
    }

    /// Loads a server's configuration; defaults when it has none yet.
    pub fn load(bot_dir: &Path, guild_id: &str) -> io::Result<Self> {
        if !is_snowflake(guild_id) {
            return Err(io::Error::other("invalid guild id"));
        }
        match fs::read_to_string(Self::path(bot_dir, guild_id)) {
            Ok(raw) => {
                let mut config: Self = serde_json::from_str(&raw).map_err(io::Error::other)?;
                config.sanitize();
                Ok(config)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error),
        }
    }

    /// Sanitizes then writes atomically (temp file + rename).
    pub fn save(&mut self, bot_dir: &Path, guild_id: &str) -> io::Result<()> {
        if !is_snowflake(guild_id) {
            return Err(io::Error::other("invalid guild id"));
        }
        self.sanitize();
        let dir = bot_dir.join(DIR_NAME);
        fs::create_dir_all(&dir)?;
        let json = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
        let tmp = dir.join(format!(".{guild_id}.json.tmp"));
        fs::write(&tmp, json + "\n")?;
        fs::rename(&tmp, Self::path(bot_dir, guild_id))
    }
}

/// `https://www.YouTube.com/watch` -> `youtube.com`.
pub fn normalize_domain(input: &str) -> String {
    let s = input.trim().to_lowercase();
    let s = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"))
        .unwrap_or(&s);
    let s = s.split(['/', '?', '#', ':']).next().unwrap_or("");
    s.strip_prefix("www.").unwrap_or(s).to_owned()
}

fn is_domain(s: &str) -> bool {
    s.len() <= 100
        && s.contains('.')
        && !s.starts_with('.')
        && !s.ends_with('.')
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_or_partial_files_get_defaults() {
        let config: GuildConfig = serde_json::from_str(r#"{"xp":{"enabled":true}}"#).unwrap();
        assert!(config.xp.enabled);
        assert_eq!(config.xp.max_xp, 25);
        assert_eq!(config.rules.verification, "button");
        assert!(config.welcome.image);
    }

    #[test]
    fn sanitize_drops_bad_input() {
        let mut c = GuildConfig::default();
        c.admins.users = vec![
            " 175928847299117063 ".into(),
            "abc".into(),
            "175928847299117063".into(),
        ];
        c.welcome.channel = Some("../etc".into());
        c.rules.verification = "magic".into();
        c.rules.title = "   ".into();
        c.lock_url.allowed_domains =
            vec!["https://www.YouTube.com/watch?v=1".into(), "nope".into()];
        c.xp.min_xp = 300;
        c.xp.max_xp = 5;
        c.xp.rewards = vec![
            Reward {
                level: 10,
                role: "175928847299117063".into(),
            },
            Reward {
                level: 0,
                role: "175928847299117063".into(),
            },
            Reward {
                level: 5,
                role: "x".into(),
            },
        ];
        c.tickets.max_open = 99;
        c.welcome.ping_seconds = 0;
        c.welcome.ping_channels = (0..20).map(|i| format!("1759288472991170{i:02}")).collect();
        c.rules.verified_role = Some("175928847299117063".into());
        c.rules.remove_role = Some("175928847299117063".into());
        c.sanitize();

        assert_eq!(c.admins.users, vec!["175928847299117063"]);
        assert_eq!(c.welcome.channel, None);
        assert_eq!(c.rules.verification, "button");
        assert_eq!(c.rules.title, "Server rules");
        assert_eq!(c.lock_url.allowed_domains, vec!["youtube.com"]);
        assert_eq!((c.xp.min_xp, c.xp.max_xp), (300, 300));
        assert_eq!(c.xp.rewards.len(), 1);
        assert_eq!(c.tickets.max_open, 5);
        assert_eq!(c.welcome.ping_seconds, 1);
        assert_eq!(c.welcome.ping_channels.len(), MAX_PING_CHANNELS);
        assert!(c.rules.verified_role.is_some());
        assert_eq!(c.rules.remove_role, None);
    }

    #[test]
    fn new_sections_are_bounded() {
        let mut c = GuildConfig::default();
        c.twitch.logins = vec![
            " https://twitch.tv/DH_Trenches ".into(),
            "a".into(),
            "bad name".into(),
        ];
        c.role_panel.buttons = vec![
            RoleButton {
                role: "175928847299117063".into(),
                label: " FR ".into(),
                emoji: "🇫🇷".into(),
                description: String::new(),
            },
            RoleButton {
                role: "nope".into(),
                label: "x".into(),
                emoji: String::new(),
                description: String::new(),
            },
        ];
        c.lock_url.scope = "weird".into();
        c.assistant.max_length = 1;
        c.reminders.interval_hours = 1;
        c.sanitize();
        assert_eq!(c.twitch.logins, vec!["dh_trenches"]);
        assert_eq!(c.role_panel.buttons.len(), 1);
        assert_eq!(c.role_panel.buttons[0].label, "FR");
        assert_eq!(c.lock_url.scope, "all");
        assert_eq!(c.assistant.max_length, 50);
        assert_eq!(c.reminders.interval_hours, 6);
        assert!(parse_link_button("Axiom | https://axiom.trade/@x").is_some());
        assert!(parse_link_button("Bad | http://x.com").is_none());
        assert!(parse_link_button("no url").is_none());
    }

    #[test]
    fn save_and_load() {
        let dir = std::env::temp_dir().join(format!("dc-core-guild-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let id = "175928847299117063";

        assert_eq!(GuildConfig::load(&dir, id).unwrap(), GuildConfig::default());
        let mut c = GuildConfig::default();
        c.logs.enabled = true;
        c.logs.channel = Some(id.into());
        c.save(&dir, id).unwrap();
        assert_eq!(GuildConfig::load(&dir, id).unwrap(), c);
        assert!(GuildConfig::load(&dir, "../x").is_err());
        fs::remove_dir_all(dir).unwrap();
    }
}
