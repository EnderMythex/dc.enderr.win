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
pub const MAX_REPOS: usize = 10;
pub const MAX_STATUS_SITES: usize = 8;
/// GitHub activities a server can follow.
pub const GITHUB_EVENTS: &[&str] = &["push", "pull_request", "issues", "star", "fork", "tag"];
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
    pub releases: Releases,
    pub github: GitHubFeed,
    pub status: StatusPanel,
    pub minecraft: Minecraft,
}

/// Announces the new versions (GitHub releases) of a repository.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Releases {
    pub enabled: bool,
    pub channel: Option<String>,
    /// Role mentioned with each new version.
    pub role: Option<String>,
    /// `owner/name`, e.g. `Misty-MC/Misty`.
    pub repo: String,
    /// Pre-releases are announced too.
    pub prereleases: bool,
    /// "Download" button of the announcements and of `/download`; empty:
    /// the release page on GitHub.
    pub download_url: String,
    /// Website button of `/download` (optional).
    pub website: String,
}

/// Commits, pull requests, issues, stars, forks and tags of repositories.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GitHubFeed {
    pub enabled: bool,
    pub channel: Option<String>,
    /// `owner/name` of each repository followed.
    pub repos: Vec<String>,
    /// Among [`GITHUB_EVENTS`].
    pub events: Vec<String>,
}

impl Default for GitHubFeed {
    fn default() -> Self {
        Self {
            enabled: false,
            channel: None,
            repos: Vec::new(),
            events: vec!["issues".into(), "pull_request".into(), "push".into()],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusSite {
    pub name: String,
    pub url: String,
}

/// `Name | https://url` (or `Name=https://url`) -> a monitored website.
pub fn parse_status_site(line: &str) -> Option<StatusSite> {
    let (name, url) = line.split_once('|').or_else(|| line.split_once('='))?;
    let (name, url) = (name.trim(), url.trim());
    let url_ok = (url.starts_with("https://") || url.starts_with("http://"))
        && url.len() <= 300
        && !url.contains(char::is_whitespace);
    (!name.is_empty() && url_ok).then(|| StatusSite {
        name: name.chars().take(24).collect(),
        url: url.to_owned(),
    })
}

/// A message kept up to date: websites up or down, latest version,
/// downloads, players and GitHub stars.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StatusPanel {
    pub enabled: bool,
    pub channel: Option<String>,
    pub sites: Vec<StatusSite>,
    /// Repository for the version, downloads and stars; empty: the one of
    /// the release announcements.
    pub repo: String,
    /// Address answering `{"players": 123}` (optional).
    pub players_url: String,
}

impl StatusPanel {
    /// The repository shown, falling back to the announced one.
    pub fn repo_or<'a>(&'a self, releases: &'a Releases) -> &'a str {
        if self.repo.is_empty() {
            &releases.repo
        } else {
            &self.repo
        }
    }
}

/// A Minecraft Java server: `/smp` and a panel kept up to date.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Minecraft {
    pub enabled: bool,
    /// `play.example.com` or `1.2.3.4:25565` (SRV records are followed).
    pub address: String,
    pub name: String,
    pub description: String,
    /// Shows the names of the connected players.
    pub show_players: bool,
    /// Channel of the panel (optional: `/smp` works without it).
    pub channel: Option<String>,
}

impl Default for Minecraft {
    fn default() -> Self {
        Self {
            enabled: false,
            address: String::new(),
            name: "Minecraft server".into(),
            description: String::new(),
            show_players: true,
            channel: None,
        }
    }
}

/// `https://github.com/Owner/Name` or `Owner/Name` -> `Owner/Name`.
pub fn normalize_repo(input: &str) -> Option<String> {
    let s = input.trim();
    let s = s
        .strip_prefix("https://github.com/")
        .or_else(|| s.strip_prefix("http://github.com/"))
        .or_else(|| s.strip_prefix("github.com/"))
        .unwrap_or(s);
    let s = s.trim_end_matches('/');
    let s = s.strip_suffix(".git").unwrap_or(s);
    let (owner, name) = s.split_once('/')?;
    let ok = |part: &str| {
        !part.is_empty()
            && part.len() <= 100
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    };
    (ok(owner) && ok(name)).then(|| format!("{owner}/{name}"))
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

/// The rules message and the verification. The Verify button goes under the
/// rules when both share a channel (or no verification channel is chosen),
/// and in its own message in the verification channel otherwise.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Rules {
    /// The rules message is posted.
    pub enabled: bool,
    pub channel: Option<String>,
    pub title: String,
    pub text: String,
    /// Verification is on. `None` only in files written before rules and
    /// verification were split; `sanitize` then derives it.
    #[serde(default)]
    pub verify_enabled: Option<bool>,
    /// Channel of the verification message; `None`: under the rules.
    pub verify_channel: Option<String>,
    pub verify_title: String,
    pub verify_text: String,
    /// One of [`VERIFY_MODES`] (`none` is stored as verification off).
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
            verify_enabled: Some(false),
            verify_channel: None,
            verify_title: "Verification".into(),
            verify_text: "Click the button below to get access to the server.".into(),
            verification: "button".into(),
            verified_role: None,
            remove_role: None,
            captcha_style: "text".into(),
        }
    }
}

impl Rules {
    /// Is the verification on?
    pub fn verifying(&self) -> bool {
        self.verify_enabled == Some(true)
    }

    /// Does the Verify button go under the rules message?
    pub fn verify_under_rules(&self) -> bool {
        self.verifying()
            && self.enabled
            && self.channel.is_some()
            && (self.verify_channel.is_none() || self.verify_channel == self.channel)
    }

    /// Channel of a separate verification message, if there is one.
    pub fn verify_message_channel(&self) -> Option<&str> {
        self.verify_channel
            .as_deref()
            .filter(|_| self.verifying() && !self.verify_under_rules())
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

/// Keeps an https link, empties anything else.
fn clean_url(url: &mut String) {
    let trimmed = url.trim();
    let ok = trimmed.starts_with("https://")
        && trimmed.len() <= 300
        && !trimmed.contains(char::is_whitespace);
    *url = if ok {
        trimmed.to_owned()
    } else {
        String::new()
    };
}

fn clean_repo(repo: &mut String) {
    *repo = normalize_repo(repo).unwrap_or_default();
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
        let rules = &mut self.rules;
        // Older files: the verification was on with the rules panel.
        let on = rules
            .verify_enabled
            .unwrap_or(rules.enabled && rules.verification != "none");
        rules.verify_enabled = Some(on && rules.verification != "none");
        if rules.verification == "none" {
            rules.verification = "button".into();
        }
        clean_id(&mut rules.verify_channel);
        clean_text(
            &mut rules.verify_title,
            MAX_TITLE,
            &defaults.rules.verify_title,
        );
        clean_text(
            &mut rules.verify_text,
            MAX_TEXT,
            &defaults.rules.verify_text,
        );
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

        let releases = &mut self.releases;
        clean_id(&mut releases.channel);
        clean_id(&mut releases.role);
        clean_repo(&mut releases.repo);
        clean_url(&mut releases.download_url);
        clean_url(&mut releases.website);

        let github = &mut self.github;
        clean_id(&mut github.channel);
        let mut repos: Vec<String> = Vec::new();
        for repo in github.repos.iter().filter_map(|repo| normalize_repo(repo)) {
            if !repos.contains(&repo) {
                repos.push(repo);
            }
        }
        repos.truncate(MAX_REPOS);
        github.repos = repos;
        github
            .events
            .retain(|event| GITHUB_EVENTS.contains(&event.as_str()));
        github.events.sort();
        github.events.dedup();

        let status = &mut self.status;
        clean_id(&mut status.channel);
        status
            .sites
            .retain(|site| parse_status_site(&format!("{}|{}", site.name, site.url)).is_some());
        status.sites.truncate(MAX_STATUS_SITES);
        clean_repo(&mut status.repo);
        let players = status.players_url.trim();
        status.players_url = if (players.starts_with("https://") || players.starts_with("http://"))
            && players.len() <= 300
            && !players.contains(char::is_whitespace)
        {
            players.to_owned()
        } else {
            String::new()
        };

        let minecraft = &mut self.minecraft;
        clean_id(&mut minecraft.channel);
        minecraft.address = minecraft
            .address
            .trim()
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '_'))
            .take(120)
            .collect();
        clean_text(&mut minecraft.name, MAX_TITLE, &defaults.minecraft.name);
        minecraft.description = minecraft
            .description
            .trim()
            .chars()
            .take(MAX_TEXT)
            .collect();
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
        assert!(!c.rules.verifying());
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
    fn rules_and_verification_split() {
        let channel = |n: u8| Some(format!("17592884729911706{n}"));
        // Before the split: verification followed the rules panel.
        let old =
            r#"{"rules":{"enabled":true,"channel":"175928847299117061","verification":"captcha"}}"#;
        let mut c: GuildConfig = serde_json::from_str(old).unwrap();
        c.sanitize();
        assert!(c.rules.verifying() && c.rules.verify_under_rules());
        assert_eq!(c.rules.verify_message_channel(), None);
        let old = r#"{"rules":{"enabled":true,"verification":"none"}}"#;
        let mut c: GuildConfig = serde_json::from_str(old).unwrap();
        c.sanitize();
        assert!(!c.rules.verifying());
        assert_eq!(c.rules.verification, "button");

        let mut r = Rules {
            channel: channel(1),
            verify_enabled: Some(true),
            verify_channel: channel(2),
            enabled: true,
            ..Rules::default()
        };
        assert!(!r.verify_under_rules());
        assert_eq!(r.verify_message_channel(), channel(2).as_deref());
        // Rules off: verification alone.
        r.enabled = false;
        assert_eq!(r.verify_message_channel(), channel(2).as_deref());
        // Same channel: one message.
        r.enabled = true;
        r.verify_channel = channel(1);
        assert!(r.verify_under_rules());
        assert_eq!(r.verify_message_channel(), None);
        r.verify_enabled = Some(false);
        assert!(!r.verify_under_rules());
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
    fn integrations_are_bounded() {
        assert_eq!(
            normalize_repo("https://github.com/Misty-MC/Misty.git").as_deref(),
            Some("Misty-MC/Misty")
        );
        assert_eq!(
            normalize_repo("Misty-MC/Misty-Web").as_deref(),
            Some("Misty-MC/Misty-Web")
        );
        assert!(normalize_repo("nope").is_none() && normalize_repo("a/b/c").is_none());
        let site = parse_status_site("Site = https://misty-launcher.online/").unwrap();
        assert_eq!(site.name, "Site");
        assert!(parse_status_site("Bad | ftp://x").is_none());

        let mut c = GuildConfig::default();
        c.github.repos = vec![
            "Misty-MC/Misty".into(),
            "bad".into(),
            "Misty-MC/Misty".into(),
        ];
        c.github.events = vec!["push".into(), "magic".into()];
        c.releases.repo = "https://github.com/Misty-MC/Misty".into();
        c.releases.download_url = "javascript:alert(1)".into();
        c.minecraft.address = " play.misty.fr:25565 <b> ".into();
        c.sanitize();
        assert_eq!(c.github.repos, vec!["Misty-MC/Misty"]);
        assert_eq!(c.github.events, vec!["push"]);
        assert_eq!(c.releases.repo, "Misty-MC/Misty");
        assert_eq!(c.releases.download_url, "");
        assert_eq!(c.minecraft.address, "play.misty.fr:25565b");
        assert_eq!(c.status.repo_or(&c.releases), "Misty-MC/Misty");
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
