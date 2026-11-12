//! HTML pages, server-rendered with Maud. No inline style or script (the CSP
//! forbids them): all visuals live in `assets/app.css`.
//!
//! Layout: every page sits in a `.frame` with dashed vertical rails. Pages are
//! split into horizontal bands separated by full-width dashed rules, with a
//! square marker where each rule crosses a rail.

use std::sync::LazyLock;

use dc_core::{
    guild::GuildConfig,
    settings::{ACTIVITY_TYPES, BotSettings, PRESENCE_STATUSES},
    thousands,
};
use maud::{DOCTYPE, Markup, PreEscaped, html};

use crate::{
    auth::Session,
    discord::{Channel, Discord, Guild, GuildContext, Role},
    docker::{ActionError, HealthState, LogLine, Status},
    guilds::{BACKUP, DANGER, MODULES},
    routes::BotView,
};

const BRAND: &str = "dc.enderr.win";
/// Stylesheet URL carrying a hash of its content, so a new build is never
/// served from the browser cache.
static STYLESHEET: LazyLock<String> = LazyLock::new(|| {
    let hash = dc_core::crypto::sha256_hex(include_str!("../assets/app.css"));
    format!("/assets/app.css?v={}", &hash[..12])
});

/// `?v=` of the verification page's script and stylesheet. Discord's proxy
/// caches the Activity's files: a new version needs a new URL.
static VERIFY_VERSION: LazyLock<String> = LazyLock::new(|| {
    let both = format!(
        "{}{}",
        include_str!("../assets/activity.js"),
        include_str!("../assets/app.css")
    );
    dc_core::crypto::sha256_hex(&both)[..12].to_owned()
});

static SCRIPT: LazyLock<String> = LazyLock::new(|| {
    let hash = dc_core::crypto::sha256_hex(include_str!("../assets/app.js"));
    format!("/assets/app.js?v={}", &hash[..12])
});
const CDN: &str = "https://cdn.discordapp.com";

// ---------------------------------------------------------------- icons

const ICON_LOGO: &str = r#"<svg viewBox="0 0 24 24" width="22" height="22" aria-hidden="true"><rect width="24" height="24" rx="6" fill="currentColor"/><path d="M7 8.5h10M7 12h10M7 15.5h6" stroke="black" stroke-width="1.8" stroke-linecap="round"/></svg>"#;
const ICON_DISCORD: &str = r#"<svg viewBox="0 0 24 24" width="18" height="18" fill="currentColor" aria-hidden="true"><path d="M19.6 5.3A17.5 17.5 0 0 0 15.3 4l-.5 1.1a16.2 16.2 0 0 0-5.6 0L8.7 4a17.5 17.5 0 0 0-4.3 1.3C1.7 9.4.9 13.3 1.3 17.2a17.6 17.6 0 0 0 5.3 2.7l1.1-1.8c-.6-.2-1.2-.5-1.8-.9l.4-.3a12.5 12.5 0 0 0 11.4 0l.4.3c-.6.4-1.2.7-1.8.9l1.1 1.8a17.6 17.6 0 0 0 5.3-2.7c.5-4.5-.8-8.4-3.1-11.9ZM8.7 14.8c-1 0-1.9-1-1.9-2.1s.8-2.1 1.9-2.1 1.9 1 1.9 2.1-.8 2.1-1.9 2.1Zm6.6 0c-1 0-1.9-1-1.9-2.1s.8-2.1 1.9-2.1 1.9 1 1.9 2.1-.8 2.1-1.9 2.1Z"/></svg>"#;
const ICON_PLAY: &str = r#"<svg viewBox="0 0 24 24" width="15" height="15" fill="currentColor" aria-hidden="true"><path d="M7 4.5v15l12-7.5z"/></svg>"#;
const ICON_STOP: &str = r#"<svg viewBox="0 0 24 24" width="15" height="15" fill="currentColor" aria-hidden="true"><rect x="6" y="6" width="12" height="12" rx="1.5"/></svg>"#;
const ICON_RESTART: &str = r#"<svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M20 11a8 8 0 1 0-2.3 6.3"/><path d="M20 4v7h-7"/></svg>"#;
const ICON_EXTERNAL: &str = r#"<svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M14 5h5v5M19 5l-8 8M11 6H6v12h12v-5"/></svg>"#;
const ICON_DOWNLOAD: &str = r#"<svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M12 4v11M7 10l5 5 5-5M5 20h14"/></svg>"#;
const ICON_BACK: &str = r#"<svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M15 6l-6 6 6 6"/></svg>"#;
const ICON_ARROW: &str = r#"<svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M9 6l6 6-6 6"/></svg>"#;

fn icon(svg: &'static str) -> Markup {
    PreEscaped(svg.to_owned())
}

// ---------------------------------------------------------------- helpers

pub fn flash_text(code: &str) -> Option<&'static str> {
    Some(match code {
        "saved" => "Settings saved. The bot applies them within a few seconds.",
        "guild" => "Saved. The bot applies the change within a few seconds.",
        "started" => "The bot is starting.",
        "stopped" => "The bot has been stopped.",
        "restarted" => "The bot is restarting.",
        "profile" => "Profile updated. Discord can take a minute to show it everywhere.",
        "reset" => "All settings of this server were reset.",
        "background" => "Card background saved. The bot uses it within a few seconds.",
        _ => return None,
    })
}

fn avatar_url(id: &str, hash: Option<&str>) -> String {
    match hash {
        Some(hash) => format!("{CDN}/avatars/{id}/{hash}.png?size=128"),
        None => default_avatar(id),
    }
}

fn default_avatar(seed: &str) -> String {
    let index = seed
        .bytes()
        .fold(0u32, |acc, b| acc.wrapping_add(u32::from(b)))
        % 6;
    format!("{CDN}/embed/avatars/{index}.png")
}

fn bot_avatar(bot: &BotView) -> String {
    match &bot.identity {
        Some(user) => avatar_url(&user.id, user.avatar.as_deref()),
        None => default_avatar(&bot.def.slug),
    }
}

fn status_label(status: &Status) -> (&'static str, &'static str) {
    match status {
        Status::Running {
            health: Some(HealthState::Unhealthy),
            ..
        } => ("Unhealthy", "badge bad"),
        Status::Running {
            health: Some(HealthState::Starting),
            ..
        } => ("Starting", "badge warn"),
        Status::Running { .. } => ("Online", "badge ok"),
        Status::Restarting => ("Restarting", "badge warn"),
        Status::Paused => ("Paused", "badge warn"),
        Status::Stopped { .. } => ("Stopped", "badge"),
        Status::Missing => ("Not deployed", "badge"),
        Status::Unmanaged => ("Misconfigured", "badge bad"),
        Status::Unavailable => ("Unknown", "badge"),
    }
}

fn badge(status: &Status) -> Markup {
    let (label, class) = status_label(status);
    html! { span class=(class) { span.dot {} (label) } }
}

/// "2026-11-11T10:12:33.12Z" -> "2026-11-11 10:12 UTC".
fn format_started(rfc3339: &str) -> Option<String> {
    let short = rfc3339.get(..16)?;
    (short.as_bytes()[10] == b'T' && !short.starts_with("0001"))
        .then(|| format!("{} UTC", short.replace('T', " ")))
}

fn presence_label(value: &str) -> &'static str {
    match value {
        "online" => "Online",
        "idle" => "Idle",
        "dnd" => "Do not disturb",
        "invisible" => "Invisible",
        _ => "—",
    }
}

fn activity_label(value: &str) -> &'static str {
    match value {
        "none" => "None",
        "playing" => "Playing…",
        "watching" => "Watching…",
        "listening" => "Listening to…",
        "competing" => "Competing in…",
        "custom" => "Custom status",
        "sol_price" => "SOL price (Watching SOL: $…)",
        _ => "—",
    }
}

fn csrf_field(session: &Session) -> Markup {
    html! { input type="hidden" name="csrf" value=(session.csrf); }
}

/// Full-width dashed line with square markers on the frame rails.
fn rule() -> Markup {
    html! { div.rule aria-hidden="true" { span.mk.l {} span.mk.r {} } }
}

// ---------------------------------------------------------------- layout

fn document(title: &str, body_class: &str, body: Markup) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                meta name="color-scheme" content="dark";
                meta name="robots" content="noindex";
                title { (title) " · " (BRAND) }
                link rel="stylesheet" href=(STYLESHEET.as_str());
                link rel="icon" type="image/svg+xml" href="/assets/favicon.svg";
                script src=(SCRIPT.as_str()) defer {}
            }
            body class=(body_class) { (body) }
        }
    }
}

fn footer() -> Markup {
    html! {
        footer.foot {
            span { (BRAND) }
            nav.foot-links aria-label="Legal" {
                a href="/legal" { "Legal notice" }
                a href="/privacy" { "Privacy" }
                a href="/terms" { "Terms" }
            }
        }
    }
}

/// A legal page (notice, privacy, terms): readable text in the site's frame.
pub fn legal_page(title: &str, updated: &str, body: Markup) -> Markup {
    document(
        title,
        "public",
        html! {
            div.frame {
                div.band.legal-head {
                    a.back href="/" { (icon(ICON_BACK)) "Back to " (BRAND) }
                    p.eyebrow { "Legal" }
                    h1 { (title) }
                    p.muted { "Last updated: " (updated) }
                }
                (rule())
                main.band.prose { (body) }
                (rule())
                (footer())
            }
        },
    )
}

/// Page of a logged-in user: top bar, a header band and a content band.
fn app_page(
    title: &str,
    session: &Session,
    flash: Option<&str>,
    head_class: &str,
    head: Markup,
    body: Markup,
) -> Markup {
    let user = &session.user;
    document(
        title,
        "",
        html! {
            div.frame {
                header.topbar {
                    div.bar {
                        a.brand href="/dashboard" { (icon(ICON_LOGO)) span { (BRAND) } }
                        div.account {
                            @if session.is_admin { span.tag { "Admin" } }
                            img.avatar.sm src=(avatar_url(&user.id, user.avatar.as_deref()))
                                alt="" width="28" height="28";
                            span.account-name { (user.display_name()) }
                            form method="post" action="/auth/logout" {
                                (csrf_field(session))
                                button.btn.ghost.small type="submit" { "Log out" }
                            }
                        }
                    }
                    (rule())
                }
                section class={ "band head " (head_class) } {
                    @if let Some(message) = flash {
                        div.notice.ok role="status" { (message) }
                    }
                    (head)
                }
                (rule())
                main.band.body { (body) }
                (rule())
                (footer())
            }
        },
    )
}

// ---------------------------------------------------------------- public pages

pub fn login(error: Option<&str>, totals: Option<crate::stats::Totals>) -> Markup {
    document(
        "Log in",
        "public",
        html! {
            div.frame {
                div.band.spacer {}
                (rule())
                main.band.hero {
                    div.login-mark { (icon(ICON_LOGO)) }
                    h1 { "Manage your Discord bots" }
                    p.lead {
                        "Start, stop and configure the bots assigned to you. "
                        "Log in with your Discord account to find them."
                    }
                    @if let Some(message) = error {
                        div.notice.bad role="alert" { (message) }
                    }
                    a.btn.primary.large href="/auth/login" {
                        (icon(ICON_DISCORD)) "Continue with Discord"
                    }
                    p.fineprint {
                        "Only your id, username and avatar are read. "
                        "The panel never accesses your messages or servers."
                    }
                    @if let Some(totals) = totals.filter(|t| t.servers > 0) {
                        div.stats aria-label="Bots in numbers" {
                            div.stat {
                                strong { (thousands(totals.servers as u64)) }
                                span { @if totals.servers == 1 { "server" } @else { "servers" } }
                            }
                            div.stat {
                                strong { (thousands(totals.members)) }
                                span { @if totals.members == 1 { "member" } @else { "members" } }
                            }
                        }
                    }
                }
                (rule())
                div.band.spacer {}
                (footer())
            }
        },
    )
}

/// Shown by the service worker while the panel restarts; reloads itself
/// once the panel answers again (see `assets/app.js`).
pub fn offline() -> Markup {
    document(
        "Reconnecting",
        "public",
        html! {
            div.frame data-offline {
                div.band.spacer {}
                (rule())
                main.band.hero {
                    div.login-mark { (icon(ICON_LOGO)) }
                    h1 { "Recovering connection" }
                    p.lead { "The panel is restarting. This page comes back by itself in a few seconds." }
                }
                (rule())
                div.band.spacer {}
                (footer())
            }
        },
    )
}

/// `retry`: a passing problem; the page reloads itself after a few seconds.
pub fn error_page(status: u16, title: &str, message: &str, retry: bool) -> Markup {
    document(
        title,
        "public",
        html! {
            div.frame {
                div.band.spacer {}
                (rule())
                main.band.hero data-retry=[retry.then_some("5")] {
                    p.error-code { (status) }
                    h1 { (title) }
                    p.lead { (message) }
                    div.hero-actions {
                        @if retry { a.btn.primary href="" { "Try again" } }
                        a.btn href="/" { "Back to home" }
                    }
                }
                (rule())
                div.band.spacer {}
                (footer())
            }
        },
    )
}

// ---------------------------------------------------------------- dashboard

pub fn dashboard(
    session: &Session,
    bots: &[BotView],
    config_errors: &[String],
    flash: Option<&str>,
) -> Markup {
    let online = bots
        .iter()
        .filter(|bot| matches!(bot.status, Status::Running { .. }))
        .count();
    let head = html! {
        p.eyebrow { "Dashboard" }
        h1 { "Your bots" }
        p.muted {
            (bots.len()) @if bots.len() == 1 { " bot" } @else { " bots" }
            " · " (online) " online"
        }
    };
    let body = html! {
        @if !config_errors.is_empty() {
            div.notice.warn role="alert" {
                strong { "Some bot folders are invalid and were skipped:" }
                ul.error-list { @for error in config_errors { li.mono { (error) } } }
            }
        }
        @if bots.is_empty() {
            section.empty {
                h2 { "No bots yet" }
                p.muted {
                    "No bot is assigned to your Discord account. "
                    "Once the administrator assigns one to you, it will show up here."
                }
                p.mono.muted { "Your Discord id: " (session.user.id) }
            }
        } @else {
            div.grid {
                @for bot in bots {
                    a.card.bot-card href={ "/bots/" (bot.def.slug) } {
                        div.bot-card-top {
                            img.avatar src=(bot_avatar(bot)) alt="" width="44" height="44";
                            div.bot-card-text {
                                span.bot-name { (bot.def.name) }
                                span.mono.muted { (bot.def.slug) }
                            }
                            span.chevron { (icon(ICON_ARROW)) }
                        }
                        @if let Some(description) = &bot.def.description {
                            p.muted.clamp { (description) }
                        }
                        div.bot-card-foot { (badge(&bot.status)) }
                    }
                }
            }
        }
    };
    app_page("Your bots", session, flash, "", head, body)
}

// ---------------------------------------------------------------- bot pages

#[derive(PartialEq)]
enum Tab {
    Overview,
    Servers,
    Commands,
    Logs,
    Settings,
}

/// Header shared by every tab of a bot.
fn bot_page(
    session: &Session,
    bot: &BotView,
    tab: Tab,
    flash: Option<&str>,
    content: Markup,
) -> Markup {
    let base = format!("/bots/{}", bot.def.slug);
    let tab_link = |target: Tab, href: String, label: &str| {
        let active = tab == target;
        html! {
            a.tab.active[active] href=(href) aria-current=[active.then_some("page")] { (label) }
        }
    };
    let head = html! {
        a.back href="/dashboard" { (icon(ICON_BACK)) "All bots" }
        div.bot-head {
            img.avatar.lg src=(bot_avatar(bot)) alt="" width="64" height="64";
            div.bot-head-text {
                h1 { (bot.def.name) }
                span.mono.muted {
                    @if let Some(user) = &bot.identity { "@" (user.username) " · " }
                    (bot.def.slug)
                }
            }
            (badge(&bot.status))
        }
        nav.tabs aria-label="Bot sections" {
            (tab_link(Tab::Overview, base.clone(), "Overview"))
            (tab_link(Tab::Servers, format!("{base}/servers"), "Servers"))
            (tab_link(Tab::Commands, format!("{base}/commands"), "Commands"))
            (tab_link(Tab::Logs, format!("{base}/logs"), "Logs"))
            (tab_link(Tab::Settings, format!("{base}/settings"), "Settings"))
        }
    };
    app_page(&bot.def.name, session, flash, "has-tabs", head, content)
}

/// Explains why the bot cannot be controlled, if that is the case.
fn status_notice(bot: &BotView) -> Markup {
    html! {
        @match &bot.status {
            Status::Missing => div.notice.warn role="status" {
                "This bot has not been deployed yet. Its controls will unlock "
                "as soon as its container exists."
            },
            Status::Unmanaged => div.notice.bad role="alert" {
                "The container " code { (bot.def.container) }
                " is not registered for this bot. Contact the administrator."
            },
            Status::Unavailable => div.notice.warn role="status" {
                "Docker cannot be reached right now, so the bot status is unknown "
                "and its controls are disabled."
            },
            Status::Stopped { error: Some(error), .. } => div.notice.bad role="alert" {
                "The bot failed to run: " (error)
            },
            _ => {}
        }
    }
}

pub fn bot_overview(
    session: &Session,
    bot: &BotView,
    settings: &BotSettings,
    flash: Option<&str>,
    error: Option<&str>,
) -> Markup {
    let status = &bot.status;
    let power_button =
        |action: &str, class: &str, svg: &'static str, label: &str, enabled: bool| {
            html! {
                button class=(class) type="submit" name="action" value=(action) disabled[!enabled] {
                    (icon(svg)) (label)
                }
            }
        };
    let content = html! {
        @if let Some(message) = error {
            div.notice.bad role="alert" { (message) }
        }
        (status_notice(bot))
        div.columns {
            section.card {
                h2 { "Power" }
                p.muted { "The bot runs in its own container. A stopped bot stays stopped until you start it again." }
                form.actions method="post" action={ "/bots/" (bot.def.slug) "/power" } {
                    (csrf_field(session))
                    (power_button("start", "btn primary", ICON_PLAY, "Start", status.can_start()))
                    (power_button("restart", "btn", ICON_RESTART, "Restart", status.can_stop()))
                    (power_button("stop", "btn danger", ICON_STOP, "Stop", status.can_stop()))
                }
            }
            section.card {
                h2 { "Invite" }
                @if let Some(user) = &bot.identity {
                    p.muted { "Add the bot to a server where you have the Manage Server permission." }
                    div.actions {
                        a.btn href=(Discord::invite_url(&user.id)) target="_blank" rel="noopener noreferrer" {
                            "Invite to a server " (icon(ICON_EXTERNAL))
                        }
                    }
                } @else {
                    p.muted { "The invite link will be available once the bot's Discord account is configured." }
                }
            }
        }
        section.card {
            h2 { "Details" }
            dl.details {
                div { dt { "Status" } dd { (status_label(status).0) } }
                @if let Status::Running { since: Some(since), .. } = status
                    && let Some(started) = format_started(since) {
                    div { dt { "Running since" } dd { (started) } }
                }
                @if let Status::Stopped { exit_code: Some(code), .. } = status {
                    div { dt { "Last exit code" } dd.mono { (code) } }
                }
                div { dt { "Presence" } dd { (presence_label(&settings.presence_status)) } }
                div {
                    dt { "Activity" }
                    dd {
                        (activity_label(&settings.activity_type))
                        @if settings.activity_type != "none" && !settings.activity_text.is_empty() {
                            " " span.muted { (settings.activity_text) }
                        }
                    }
                }
                @if let Some(user) = &bot.identity {
                    div { dt { "Application id" } dd.mono { (user.id) } }
                }
                @if session.is_admin {
                    div { dt { "Container" } dd.mono { (bot.def.container) } }
                    div {
                        dt { "Owners" }
                        dd.mono {
                            @for (i, owner) in bot.def.owners.iter().enumerate() {
                                @if i > 0 { ", " }
                                (owner)
                            }
                            @if bot.def.owners.is_empty() { "—" }
                        }
                    }
                }
            }
        }
    };
    bot_page(session, bot, Tab::Overview, flash, content)
}

pub fn bot_servers(
    session: &Session,
    bot: &BotView,
    guilds: Result<Vec<Guild>, &str>,
    flash: Option<&str>,
) -> Markup {
    let content = html! {
        @match guilds {
            Err(message) => { div.notice.bad role="alert" { (message) } }
            Ok(guilds) if guilds.is_empty() => {
                section.empty {
                    h2 { "No servers" }
                    p.muted { "This bot has not joined any server yet." }
                    @if let Some(user) = &bot.identity {
                        a.btn href=(Discord::invite_url(&user.id)) target="_blank" rel="noopener noreferrer" {
                            "Invite to a server " (icon(ICON_EXTERNAL))
                        }
                    }
                }
            }
            Ok(guilds) => {
                section.card.flush {
                    div.card-head {
                        h2 { "Servers" }
                        span.muted { (guilds.len()) }
                    }
                    ul.rows {
                        @for guild in &guilds {
                            li {
                                a.row.row-link href={ "/bots/" (bot.def.slug) "/servers/" (guild.id) } {
                                @if let Some(hash) = &guild.icon {
                                    img.avatar.sq src={ (CDN) "/icons/" (guild.id) "/" (hash) ".png?size=64" }
                                        alt="" width="36" height="36" loading="lazy";
                                } @else {
                                    span.avatar.sq.placeholder aria-hidden="true" {
                                        (guild.name.chars().next().unwrap_or('?'))
                                    }
                                }
                                div.row-text {
                                    span { (guild.name) }
                                    span.mono.muted { (guild.id) }
                                }
                                @if let Some(members) = guild.approximate_member_count {
                                    span.muted.nowrap { (thousands(members)) " members" }
                                }
                                span.chevron { (icon(ICON_ARROW)) }
                                }
                            }
                        }
                    }
                }
            }
        }
    };
    bot_page(session, bot, Tab::Servers, flash, content)
}

pub fn bot_logs(
    session: &Session,
    bot: &BotView,
    logs: Result<Vec<LogLine>, ActionError>,
    activity: &[crate::audit::Entry],
) -> Markup {
    let content = html! {
        @match logs {
            Err(error) => { div.notice.warn role="status" { (error) } }
            Ok(lines) => {
                section.card.flush {
                    div.card-head {
                        h2 { "Console" }
                        a.btn.small href={ "/bots/" (bot.def.slug) "/logs" } { (icon(ICON_RESTART)) "Refresh" }
                    }
                    @if lines.is_empty() {
                        p.muted.logs-empty { "No output yet." }
                    } @else {
                        // column-reverse keeps the scroll pinned to the latest line.
                        div.logs tabindex="0" aria-label="Bot console output" {
                            div {
                                @for line in &lines {
                                    div.ll {
                                        @if let Some(time) = &line.time { span.lt { (time) } }
                                        span { (line.text) }
                                    }
                                }
                            }
                        }
                    }
                }
                p.hint { "Last 300 lines of the container output." }
            }
        }
        section.card.flush {
            div.card-head {
                h2 { "Panel activity" }
            }
            @if activity.is_empty() {
                p.muted.logs-empty { "Nothing done on the panel yet." }
            } @else {
                ul.audit {
                    @for entry in activity {
                        li {
                            span.audit-time { (crate::audit::format_time(entry.at)) }
                            span.audit-who title=(entry.user_id) { (entry.user_name) }
                            span.audit-what { (entry.action) }
                        }
                    }
                }
            }
        }
        p.hint {
            "Changes made on this panel: profile, presence, power, server settings, tests and backups. Latest "
            (crate::audit::SHOWN) " entries."
        }
    };
    bot_page(session, bot, Tab::Logs, None, content)
}

pub fn bot_settings(
    session: &Session,
    bot: &BotView,
    settings: &BotSettings,
    bio: Option<&str>,
    flash: Option<&str>,
    error: Option<&str>,
) -> Markup {
    let option = |value: &str, label: &str, current: &str| {
        html! { option value=(value) selected[value == current] { (label) } }
    };
    let content = html! {
        @if let Some(message) = error {
            div.notice.bad role="alert" { (message) }
        }
        @if let Some(user) = &bot.identity {
            form.card.form method="post" enctype="multipart/form-data" data-track
                action={ "/bots/" (bot.def.slug) "/profile" } {
                (csrf_field(session))
                h2 { "Profile" }
                p.muted { "How the bot's profile looks on Discord." }
                @if let Some(hash) = &user.banner {
                    img.banner-preview src={ (CDN) "/banners/" (user.id) "/" (hash) ".png?size=600" }
                        alt="Current banner" width="600" height="240";
                }
                div.profile {
                    img.avatar.lg src=(bot_avatar(bot)) alt="" width="64" height="64";
                    div.field.grow {
                        label for="username" { "Name" }
                        input #username type="text" name="username" value=(user.username)
                            minlength="2" maxlength="32" required autocomplete="off";
                        p.hint { "Discord allows two name changes per hour." }
                    }
                }
                @match bio {
                    Some(bio) => div.field {
                        label for="bio" { "Bio" }
                        textarea #bio name="bio" rows="4" maxlength=(crate::routes::MAX_BIO_CHARS) { (bio) }
                        p.hint { "The \"About me\" of the bot's profile, 400 characters at most. Discord formatting works." }
                    },
                    None => p.hint { "The bio cannot be read right now; it is left unchanged." },
                }
                div.field-row {
                    div.field {
                        label for="avatar" { "New picture" }
                        input #avatar type="file" name="avatar" accept="image/png,image/jpeg,image/gif,image/webp";
                        p.hint { "PNG, JPEG, GIF or WebP, 2 MB at most." }
                    }
                    div.field {
                        label for="banner" { "New banner" }
                        input #banner type="file" name="banner" accept="image/png,image/jpeg,image/gif,image/webp";
                        p.hint { "Wide image (about 600 × 240), 4 MB at most." }
                    }
                }
                @if user.banner.is_some() {
                    label.check {
                        input type="checkbox" name="remove_banner" value="on";
                        span { "Remove the current banner" }
                    }
                }
                p.hint { "Leave the files empty to keep the current images." }
                div.form-actions {
                    button.btn.primary type="submit" { "Save profile" }
                }
            }
        }
        form.card.form method="post" action={ "/bots/" (bot.def.slug) "/settings" } data-track {
            (csrf_field(session))
            h2 { "Presence" }
            p.muted { "How the bot appears on Discord. Server features are configured per server, in the Servers tab." }
            div.field-row {
                div.field {
                    label for="presence_status" { "Presence" }
                    select #presence_status name="presence_status" {
                        @for value in PRESENCE_STATUSES {
                            (option(value, presence_label(value), &settings.presence_status))
                        }
                    }
                }
            }
            div.field-row {
                div.field {
                    label for="activity_type" { "Activity" }
                    select #activity_type name="activity_type" {
                        @for value in ACTIVITY_TYPES {
                            (option(value, activity_label(value), &settings.activity_type))
                        }
                    }
                }
                div.field.grow {
                    label for="activity_text" { "Activity text" }
                    input #activity_text type="text" name="activity_text" value=(settings.activity_text)
                        maxlength="128" autocomplete="off" placeholder="e.g. your commands";
                }
            }
            div.form-actions {
                button.btn.primary type="submit" { "Save" }
            }
        }
        @let has_background = bot.def.dir.join(dc_core::render::BACKGROUND_FILE).is_file();
        form.card.form method="post" enctype="multipart/form-data" data-track
            action={ "/bots/" (bot.def.slug) "/background" } {
            (csrf_field(session))
            h2 { "Card background" }
            p.muted {
                "A picture drawn behind every image the bot sends: error and success messages, "
                "welcome card, rank card, leaderboard, settings panel. Darkened so the text stays readable."
            }
            div.preview-pair {
                img.dimg src={ "/bots/" (bot.def.slug) "/background/notice.png" } alt="Sample message" width="1000" height="266" loading="lazy";
                img.dimg src={ "/bots/" (bot.def.slug) "/background/welcome.png" } alt="Sample welcome card" width="1000" height="360" loading="lazy";
            }
            div.field-row {
                div.field {
                    label for="picture" { "New picture" }
                    input #picture type="file" name="picture" accept="image/png,image/jpeg,image/gif,image/webp";
                    p.hint { "PNG, JPEG, GIF or WebP, 3 MB at most. Wide pictures fit best (about 1000 × 300); it is cropped to each card." }
                }
                div.field {
                    label for="dim" { "Darkening (%)" }
                    input #dim type="number" name="dim" min="0" max="90" step="5" value=(settings.background_dim);
                    p.hint { "0: the picture as is. 50 to 70 keeps the text easy to read." }
                }
            }
            @if has_background {
                label.check {
                    input type="checkbox" name="remove" value="on";
                    span { "Remove the picture (back to the plain black cards)" }
                }
            }
            div.form-actions {
                button.btn.primary type="submit" { "Save background" }
            }
        }
    };
    bot_page(session, bot, Tab::Settings, flash, content)
}

/// Every slash command of the bot, with who can use it.
pub fn bot_commands(
    session: &Session,
    bot: &BotView,
    commands: &[dc_core::commands::Listed],
) -> Markup {
    let group = |public: bool| {
        html! {
            ul.commands {
                @for command in commands.iter().filter(|c| c.public == public) {
                    li {
                        div.command-line {
                            code.command-name { (command.usage) }
                            @for option in &command.options {
                                @if let Some(name) = option.strip_suffix('?') {
                                    span.command-option.optional title="Optional" { (name) }
                                } @else {
                                    span.command-option title="Required" { (option) }
                                }
                            }
                        }
                        p.muted { (command.description) }
                    }
                }
            }
        }
    };
    let content = html! {
        section.card.flush {
            div.card-head { h2 { "Everyone" } }
            (group(true))
        }
        section.card.flush {
            div.card-head { h2 { "Bot admins" } }
            p.hint.commands-hint {
                "The server owner, members with the Administrator permission, the bot's owners and the "
                "admins of the server's Admins section (or added with /ownerbot)."
            }
            (group(false))
        }
        p.hint {
            "Type / in a channel of the server to use them. Options in grey are optional. "
            "Moderation commands only work on members below the bot's role."
        }
    };
    bot_page(session, bot, Tab::Commands, None, content)
}

// ---------------------------------------------------------------- server configuration

/// On/off switch (a styled checkbox).
fn switch(name: &str, label: &str, hint: &str, checked: bool) -> Markup {
    html! {
        label.switch {
            input type="checkbox" name=(name) value="on" checked[checked];
            span.switch-track aria-hidden="true" {}
            span.switch-text {
                strong { (label) }
                @if !hint.is_empty() { span.muted { (hint) } }
            }
        }
    }
}

fn field(id: &str, label: &str, hint: &str, control: Markup) -> Markup {
    html! {
        div.field {
            label for=(id) { (label) }
            (control)
            @if !hint.is_empty() { p.hint { (hint) } }
        }
    }
}

/// Dropdown of channels; `None` is always offered.
fn channel_select<'a>(
    name: &str,
    label: &str,
    hint: &str,
    channels: impl Iterator<Item = &'a Channel>,
    current: &Option<String>,
) -> Markup {
    field(
        name,
        label,
        hint,
        html! {
            select #(name) name=(name) {
                option value="" { "— None —" }
                @for channel in channels {
                    option value=(channel.id) selected[current.as_deref() == Some(channel.id.as_str())] {
                        @if channel.is_text() { "# " }
                        (channel.name)
                    }
                }
            }
        },
    )
}

/// The options of a role menu. Each carries why giving it to members may be
/// a mistake (`data-give-risk`), read by the warning before saving.
fn role_options(roles: &[Role], current: Option<&str>) -> Markup {
    html! {
        option value="" { "— None —" }
        @for role in roles {
            option value=(role.id) selected[current == Some(role.id.as_str())]
                data-give-risk=[role.staff_risk()] { "@" (role.name) }
        }
    }
}

/// A role with what could make picking it a mistake: given to members
/// (`give`), or trusted as staff (`staff`).
struct RoleRisk<'a> {
    role: &'a Role,
    give: Option<String>,
    staff: Option<String>,
}

fn role_risks<'a>(roles: &'a [Role], config: &GuildConfig) -> Vec<RoleRisk<'a>> {
    roles
        .iter()
        .map(|role| {
            let staff = if config.welcome.auto_roles.contains(&role.id) {
                Some("it is given to every new member (Welcome)".to_owned())
            } else if config.rules.verified_role.as_deref() == Some(role.id.as_str()) {
                Some("it is given to every verified member (Verification)".to_owned())
            } else if role.sounds_common() {
                Some("its name sounds like a role every member has".to_owned())
            } else {
                None
            };
            RoleRisk {
                role,
                give: role.staff_risk(),
                staff,
            }
        })
        .collect()
}

/// Checkboxes of roles. `warn` is `give` when the roles are handed to
/// members, `staff` when they get staff rights: risky picks ask for a
/// confirmation before saving.
fn role_checks(
    name: &str,
    legend: &str,
    hint: &str,
    roles: &[RoleRisk],
    selected: &[String],
    warn: &str,
) -> Markup {
    html! {
        fieldset.checks data-warn=(warn) data-warn-label=(legend) {
            legend { (legend) }
            @if !hint.is_empty() { p.hint { (hint) } }
            @if roles.is_empty() {
                p.muted { "Nothing to choose from on this server." }
            } @else {
                div.checks-list {
                    @for risk in roles {
                        @let id = risk.role.id.as_str();
                        label.check {
                            input type="checkbox" name=(name) value=(id)
                                checked[selected.iter().any(|s| s == id)]
                                data-role=(risk.role.name)
                                data-give-risk=[risk.give.as_deref()]
                                data-staff-risk=[risk.staff.as_deref()];
                            span { "@" (risk.role.name) }
                        }
                    }
                }
            }
        }
    }
}

/// Scrollable list of checkboxes: `(id, label)`.
fn checks(
    name: &str,
    legend: &str,
    hint: &str,
    items: &[(&str, String)],
    selected: &[String],
) -> Markup {
    html! {
        fieldset.checks {
            legend { (legend) }
            @if !hint.is_empty() { p.hint { (hint) } }
            @if items.is_empty() {
                p.muted { "Nothing to choose from on this server." }
            } @else {
                div.checks-list {
                    @for (id, label) in items {
                        label.check {
                            input type="checkbox" name=(name) value=(id)
                                checked[selected.iter().any(|s| s == id)];
                            span { (label) }
                        }
                    }
                }
            }
        }
    }
}

fn textarea(name: &str, label: &str, hint: &str, value: &str, rows: u8, max: u32) -> Markup {
    field(
        name,
        label,
        hint,
        html! {
            textarea #(name) name=(name) rows=(rows) maxlength=(max) { (value) }
        },
    )
}

fn number_input(name: &str, label: &str, hint: &str, value: u32, min: u32, max: u32) -> Markup {
    field(
        name,
        label,
        hint,
        html! {
            input #(name) type="number" name=(name) value=(value) min=(min) max=(max) required;
        },
    )
}

fn choice(name: &str, label: &str, hint: &str, options: &[(&str, &str)], current: &str) -> Markup {
    field(
        name,
        label,
        hint,
        html! {
            select #(name) name=(name) {
                @for (value, text) in options {
                    option value=(value) selected[*value == current] { (text) }
                }
            }
        },
    )
}

/// The form fields of one section of a server's configuration.
/// Is a secret configured for this bot (`.env` or `KEY_<SLUG>`)?
fn has_secret(bot: &BotView, key: &str) -> bool {
    dc_core::registry::read_env(&bot.def.dir, key).is_some()
}

/// GitHub works without a token, but checks less often.
fn github_token_hint(bot: &BotView) -> Markup {
    let key = format!(
        "GITHUB_TOKEN_{}",
        bot.def.slug.to_uppercase().replace('-', "_")
    );
    html! {
        @if !has_secret(bot, "GITHUB_TOKEN") {
            div.notice role="status" {
                "Without " code { (key) } " (a GitHub token with no permission) GitHub allows 60 requests per hour: "
                "the bot then checks every 10 to 15 minutes instead of every 1 to 2."
            }
        }
    }
}

fn missing_secret(what: &str, keys: &str) -> Markup {
    html! {
        div.notice.warn role="status" {
            (what) " needs " code { (keys) } " in the bot's settings on the server (Coolify "
            "environment variables). Until then it stays off, even when enabled here."
        }
    }
}

fn guild_fields(
    module: &str,
    bot: &BotView,
    context: &GuildContext,
    config: &GuildConfig,
) -> Markup {
    let text_channels = || context.channels.iter().filter(|c| c.is_text());
    let categories = || context.channels.iter().filter(|c| c.is_category());
    let roles = &context.roles;
    let risks = role_risks(roles, config);
    let channel_items: Vec<(&str, String)> = text_channels()
        .map(|c| (c.id.as_str(), format!("# {}", c.name)))
        .collect();

    match module {
        "admins" => {
            let admins = &config.admins;
            html! {
                h2 { "Bot admins" }
                p.muted {
                    "Admins can use the moderation and settings commands. The server owner, members with "
                    "Discord's Administrator permission and the owners of this bot are always admins."
                }
                (textarea("users", "Admin members", "One Discord user id per line. In Discord: enable Developer Mode, then right-click a member and choose Copy User ID. The /ownerbot command edits this list too.", &admins.users.join("\n"), 5, 1200))
                (role_checks("roles", "Admin roles", "Every member with one of these roles is an admin.", &risks, &admins.roles, "staff"))
            }
        }
        "welcome" => {
            let welcome = &config.welcome;
            html! {
                h2 { "Welcome" }
                (switch("enabled", "Welcome new members", "Posts a message when someone joins.", welcome.enabled))
                (channel_select("channel", "Welcome channel", "", text_channels(), &welcome.channel))
                (textarea("text", "Message", "Placeholders: {user} mentions the member, {name} their name, {server} the server name, {count} the member number.", &welcome.message, 4, 1800))
                (switch("image", "Attach the welcome image", "A card with the member's avatar, name and member number.", welcome.image))
                (role_checks("roles", "Roles given on arrival", "The bot's role must be above these roles.", &risks, &welcome.auto_roles, "give"))
                (checks("channels", "Mention new members in these channels", "Optional, up to 10. The bot mentions each new member in every channel ticked here, then deletes the mention after a few seconds: the channels light up for them without leaving messages behind. Works even if the welcome message is off.", &channel_items, &welcome.ping_channels))
                (number_input("ping_seconds", "Delete the mentions after (seconds)", "", welcome.ping_seconds, 1, 60))
                h2.section-title { "Private message" }
                (switch("dm_enabled", "Send a private message to new members", "Members who do not accept DMs simply do not get it. Works even if the welcome message is off.", welcome.dm_enabled))
                (textarea("dm_text", "Message", "Placeholders: {user}, {server}. Discord formatting works.", &welcome.dm_text, 5, 1800))
                (textarea("dm_text_en", "English version (optional)", "When filled in, the message gets a language button to switch between the two.", &welcome.dm_text_en, 5, 1800))
                (switch("dm_safety", "Add the anti-scam tips", "Staff never DMs first, never share your seed phrase, and how to turn off DMs (with a picture).", welcome.dm_safety))
                (textarea("dm_links", "Link buttons", "Up to 3, one per line: Label | https://link", &welcome.dm_links.iter().map(|l| format!("{} | {}", l.label, l.url)).collect::<Vec<_>>().join("\n"), 3, 1200))
            }
        }
        "rules" => {
            let rules = &config.rules;
            html! {
                h2 { "Rules" }
                (switch("enabled", "Post the rules message", "The bot posts it in the channel below and keeps it up to date. Turn it off to only use the verification.", rules.enabled))
                (channel_select("channel", "Rules channel", "", text_channels(), &rules.channel))
                (field("title", "Title", "", html! {
                    input #title type="text" name="title" value=(rules.title) maxlength="80" required;
                }))
                (textarea("text", "Rules", "Discord formatting works: **bold**, lists, links.", &rules.text, 9, 1800))
                p.hint {
                    @if rules.verify_under_rules() {
                        "The Verify button is shown under the rules. To post it in a message of its own, choose another channel in the Verification section."
                    } @else if rules.verifying() {
                        "The verification has its own message (Verification section)."
                    } @else {
                        "The verification is off (Verification section)."
                    }
                }
            }
        }
        "verification" => {
            let rules = &config.rules;
            let modes = [
                ("button", "Button — one click to accept"),
                ("colors", "Colors — click the button of the right color"),
                ("captcha", "Captcha — type the code from an image"),
                ("activity", "Activity — captcha in a Discord Activity"),
                ("web", "Website — captcha on a page of this site, by link"),
            ];
            let styles = [
                ("text", "Text — type the code shown in an image"),
                ("slide", "Slide — slide a puzzle piece into place"),
            ];
            html! {
                h2 { "Verification" }
                (switch("enabled", "Verify new members", "Members click Verify, pass the check below and get the verified role.", rules.verifying()))
                @if rules.verifying() && rules.verified_role.is_none() {
                    div.notice.warn role="status" { "Choose the verified role below, or nobody can be verified." }
                }
                @if rules.verifying() && !rules.verify_under_rules() && rules.verify_message_channel().is_none() {
                    div.notice.warn role="status" { "The Verify button is posted nowhere yet: choose a verification channel, or turn the rules message on." }
                }
                (channel_select("channel", "Verification channel", "Empty or the rules channel: the Verify button goes under the rules. Another channel: the bot posts a verification message of its own there, with the title and text below.", text_channels(), &rules.verify_channel))
                (field("title", "Title of the verification message", "", html! {
                    input #title type="text" name="title" value=(rules.verify_title) maxlength="80" required;
                }))
                (textarea("text", "Text of the verification message", "", &rules.verify_text, 3, 1800))
                (choice("verification", "How members verify", "Website gives each member a personal link to a captcha page. The Activity opens it inside Discord but needs a one-time setup by the administrator; until then members get the image captcha.", &modes, &rules.verification))
                (choice("captcha_style", "Captcha on the website and in the Activity", "", &styles, &rules.captcha_style))
                (field("role", "Verified role", "Given once verified. Use it to unlock the rest of the server.", html! {
                    select #role name="role" data-warn="give" data-warn-label="Verified role" { (role_options(roles, rules.verified_role.as_deref())) }
                }))
                (field("remove_role", "Role removed once verified", "Optional. For example an \"Unverified\" role given on arrival in the Welcome section.", html! {
                    select #remove_role name="remove_role" { (role_options(roles, rules.remove_role.as_deref())) }
                }))
            }
        }
        "tickets" => {
            let tickets = &config.tickets;
            html! {
                h2 { "Tickets" }
                (switch("enabled", "Enable tickets", "The bot posts a panel members use to open a private channel with the staff.", tickets.enabled))
                (channel_select("channel", "Panel channel", "", text_channels(), &tickets.panel_channel))
                (channel_select("category", "Ticket category", "New ticket channels are created in this category.", categories(), &tickets.category))
                (channel_select("log_channel", "Transcript channel", "A transcript is sent here when a ticket is closed.", text_channels(), &tickets.log_channel))
                (role_checks("roles", "Staff roles", "These roles can see, claim and close tickets.", &risks, &tickets.staff_roles, "staff"))
                (field("title", "Panel title", "", html! {
                    input #title type="text" name="title" value=(tickets.title) maxlength="80" required;
                }))
                (textarea("text", "Panel text", "", &tickets.text, 4, 1800))
                (textarea("topics", "Topics", "Optional, one per line (10 max). Members then pick a topic from a menu.", &tickets.topics.join("\n"), 4, 900))
                (number_input("max_open", "Open tickets per member", "", tickets.max_open, 1, 5))
            }
        }
        "levels" => {
            let xp = &config.xp;
            let announce = [
                ("off", "Do not announce"),
                ("same", "In the channel where it happened"),
                ("channel", "In the channel below"),
            ];
            let rows = (xp.rewards.len() + 1).clamp(5, dc_core::guild::MAX_REWARDS);
            html! {
                h2 { "Levels" }
                (switch("enabled", "Enable levels", "Members earn XP by chatting. Commands: /rank and /leaderboard.", xp.enabled))
                div.field-row {
                    (number_input("min_xp", "Minimum XP per message", "", xp.min_xp, 1, 500))
                    (number_input("max_xp", "Maximum XP per message", "", xp.max_xp, 1, 500))
                    (number_input("cooldown", "Cooldown (seconds)", "", xp.cooldown_secs, 0, 3600))
                }
                (choice("announce", "Level-up announcements", "", &announce, &xp.announce))
                (channel_select("channel", "Announcement channel", "Used when announcements go to a specific channel.", text_channels(), &xp.announce_channel))
                fieldset.checks {
                    legend { "Role rewards" }
                    p.hint { "Give a role when a member reaches a level. Leave a level empty to skip the row." }
                    @for index in 0..rows {
                        @let reward = xp.rewards.get(index);
                        div.reward-row {
                            input type="number" name="reward_level" min="1" max="500" placeholder="Level"
                                aria-label="Level" value=[reward.map(|r| r.level)];
                            select name="reward_role" aria-label="Role" data-warn="give" data-warn-label="Role rewards" {
                                (role_options(roles, reward.map(|r| r.role.as_str())))
                            }
                        }
                    }
                }
                (checks("channels", "Channels without XP", "", &channel_items, &xp.ignored_channels))
            }
        }
        "logs" => {
            let logs = &config.logs;
            html! {
                h2 { "Logs" }
                (switch("enabled", "Enable logs", "", logs.enabled))
                (channel_select("channel", "Log channel", "Keep it private to the staff.", text_channels(), &logs.channel))
                (switch("log_message_delete", "Deleted messages", "", logs.message_delete))
                (switch("log_message_edit", "Edited messages", "With the text before and after.", logs.message_edit))
                (switch("log_channels", "Channels", "Created, deleted, renamed or topic changed.", logs.channels))
                (switch("log_members", "Members", "Arrivals, departures and verifications.", logs.members))
                (switch("log_moderation", "Moderation", "Mutes, kicks, bans, locks and setting changes made with the bot.", logs.moderation))
            }
        }
        "security" => {
            let rules = &config.anti_scam;
            html! {
                h2 { "Anti-scam" }
                p.muted { "Automatic checks on every message. Bot admins and members who can manage messages, kick or manage the server are never affected." }
                (switch("invites", "Block Discord invites", "Even disguised ones (line breaks, look-alike dots, invisible characters). The message is deleted and its author kicked (1 hour timeout if the kick fails).", rules.invites))
                (switch("insider", "Block the \"insider\" scam pitch", "The recurring \"new to crypto? join an insider, text me\" message, even reworded. Deleted, author banned.", rules.insider))
                (switch("images", "Check pictures with AI", "Recruiting ads such as \"I make 10 people rich, DM me on Telegram\" are deleted and their author banned. A ban needs both a promise of gains and an invitation to get in touch, written in the picture.", rules.images))
                @if rules.images && !has_secret(bot, "GEMINI_API_KEY") {
                    (missing_secret("Checking pictures", &format!("GEMINI_API_KEY_{}", bot.def.slug.to_uppercase().replace('-', "_"))))
                }
                (channel_select("channel", "Staff channel", "Scam pictures are reposted here with an Unban button, and failed sanctions are reported. Keep it private.", text_channels(), &rules.staff_channel))
                p.hint { "A ban falls back to a kick, then to a 28 day timeout, when Discord refuses it." }
            }
        }
        "roles" => {
            let panel = &config.role_panel;
            let rows = (panel.buttons.len() + 1).clamp(5, dc_core::guild::MAX_ROLE_BUTTONS);
            html! {
                h2 { "Role panel" }
                (switch("enabled", "Post the role panel", "Members click a button to take or drop a role. The bot posts the panel and keeps it up to date.", panel.enabled))
                (channel_select("channel", "Panel channel", "", text_channels(), &panel.channel))
                (field("title", "Title", "", html! {
                    input #title type="text" name="title" value=(panel.title) maxlength="80" required;
                }))
                fieldset.checks {
                    legend { "Buttons" }
                    p.hint { "Up to 10. Roles with moderation or admin permissions are never given, and the bot's role must be above these roles. Leave the role empty to skip a row." }
                    @for index in 0..rows {
                        @let button = panel.buttons.get(index);
                        div.role-row {
                            select name="button_role" aria-label="Role" data-warn="give" data-warn-label="Role panel buttons" {
                                (role_options(roles, button.map(|b| b.role.as_str())))
                            }
                            input type="text" name="button_label" placeholder="Label" maxlength="80" aria-label="Label"
                                value=[button.map(|b| b.label.as_str())];
                            input type="text" name="button_emoji" placeholder="Emoji" maxlength="8" aria-label="Emoji"
                                value=[button.map(|b| b.emoji.as_str())];
                            input type="text" name="button_description" placeholder="Description (optional)" maxlength="200"
                                aria-label="Description" value=[button.map(|b| b.description.as_str())];
                        }
                    }
                }
            }
        }
        "assistant" => {
            let assistant = &config.assistant;
            html! {
                h2 { "AI assistant" }
                (switch("enabled", "Answer members who mention the bot", "Short answers (1 to 3 sentences) in the language of the question, powered by Gemini.", assistant.enabled))
                @if !has_secret(bot, "GEMINI_API_KEY") {
                    (missing_secret("The assistant", &format!("GEMINI_API_KEY_{}", bot.def.slug.to_uppercase().replace('-', "_"))))
                }
                (textarea("context", "What the assistant knows", "Examples of questions and answers, facts about the server, links it may give, rules to follow. Empty: the bot's default knowledge.", &assistant.context, 12, 12000))
                (checks("channels", "Channels where it answers", "None ticked: everywhere.", &channel_items, &assistant.channels))
                div.field-row {
                    (number_input("max_length", "Longest question (characters)", "", assistant.max_length, 50, 2000))
                    (number_input("cooldown", "Seconds between two questions of a member", "", assistant.cooldown_secs, 1, 600))
                }
            }
        }
        "twitch" => {
            let twitch = &config.twitch;
            html! {
                h2 { "Twitch alerts" }
                (switch("enabled", "Announce when streamers go live", "Checked every minute.", twitch.enabled))
                @if !has_secret(bot, "TWITCH_CLIENT_ID") || !has_secret(bot, "TWITCH_CLIENT_SECRET") {
                    @let slug = bot.def.slug.to_uppercase().replace('-', "_");
                    (missing_secret("Twitch alerts", &format!("TWITCH_CLIENT_ID_{slug} and TWITCH_CLIENT_SECRET_{slug}")))
                }
                (channel_select("channel", "Alert channel", "", text_channels(), &twitch.channel))
                (field("role", "Role mentioned", "Optional, e.g. a \"Twitch alerts\" role members can take in the role panel.", html! {
                    select #role name="role" { (role_options(roles, twitch.role.as_deref())) }
                }))
                (textarea("logins", "Streamers", "Twitch user names, one per line (10 at most), e.g. dh_trenches.", &twitch.logins.join("\n"), 4, 400))
            }
        }
        "reminders" => {
            let reminders = &config.reminders;
            html! {
                h2 { "Safety reminders" }
                (switch("enabled", "Post an anti-scam reminder regularly", "Staff never DMs first, never share your seed phrase, how to turn off DMs.", reminders.enabled))
                (channel_select("channel_fr", "French channel", "", text_channels(), &reminders.channel_fr))
                (channel_select("channel_en", "English channel", "", text_channels(), &reminders.channel_en))
                (number_input("interval", "Every (hours)", "", reminders.interval_hours, 6, 336))
            }
        }
        "releases" => {
            let releases = &config.releases;
            html! {
                h2 { "Release announcements" }
                (switch("enabled", "Announce new versions", "Each new GitHub release of the repository is posted with its notes, file sizes and download buttons. Checked every 2 minutes.", releases.enabled))
                (github_token_hint(bot))
                (field("repo", "Repository", "owner/name of a public repository, e.g. Misty-MC/Misty.", html! {
                    input #repo type="text" name="repo" value=(releases.repo) maxlength="200" placeholder="owner/name";
                }))
                (channel_select("channel", "Announcement channel", "", text_channels(), &releases.channel))
                (field("role", "Role mentioned", "Optional.", html! {
                    select #role name="role" { (role_options(roles, releases.role.as_deref())) }
                }))
                (switch("prereleases", "Announce pre-releases too", "", releases.prereleases))
                (field("download_url", "Download link", "Optional. The Download button; empty: the release page on GitHub. Also used by /download and the status panel.", html! {
                    input #download_url type="url" name="download_url" value=(releases.download_url) maxlength="300" placeholder="https://";
                }))
                (field("website", "Website", "Optional. Shown by /download and the status panel.", html! {
                    input #website type="url" name="website" value=(releases.website) maxlength="300" placeholder="https://";
                }))
                @if bot.def.packs.iter().any(|p| p == dc_core::commands::MISTY) {
                    p.hint { "/release and /download show the latest version of this repository." }
                }
            }
        }
        "github" => {
            let feed = &config.github;
            let events = [
                ("push", "Commits"),
                ("pull_request", "Pull requests"),
                ("issues", "Issues"),
                ("star", "Stars"),
                ("fork", "Forks"),
                ("tag", "Tags and branches"),
            ];
            let items: Vec<(&str, String)> = events
                .iter()
                .map(|(id, label)| (*id, (*label).to_owned()))
                .collect();
            html! {
                h2 { "GitHub activity" }
                (switch("enabled", "Post the activity of repositories", "Commits, pull requests, issues, stars, forks and tags, with the author's picture. Checked every minute; the first check only records where each repository stands.", feed.enabled))
                (github_token_hint(bot))
                (channel_select("channel", "Channel", "", text_channels(), &feed.channel))
                (textarea("repos", "Repositories", "Public repositories, one per line (10 at most): owner/name or their GitHub link.", &feed.repos.join("\n"), 4, 2000))
                (checks("events", "What to post", "", &items, &feed.events))
            }
        }
        "status" => {
            let status = &config.status;
            let sites: Vec<String> = status
                .sites
                .iter()
                .map(|s| format!("{} | {}", s.name, s.url))
                .collect();
            html! {
                h2 { "Status panel" }
                (switch("enabled", "Keep a status panel up to date", "One message, edited every 5 minutes: each website up or down with its recent history, and the numbers below. Members can refresh it.", status.enabled))
                (channel_select("channel", "Channel", "", text_channels(), &status.channel))
                (textarea("sites", "Websites", "Up to 8, one per line: Name | https://address", &sites.join("\n"), 5, 3000))
                (field("repo", "Repository for the numbers", "Optional: latest version, downloads and stars. Empty: the repository of the release announcements.", html! {
                    input #repo type="text" name="repo" value=(status.repo) maxlength="200" placeholder="owner/name";
                }))
                (field("players_url", "Player count", "Optional: an address answering {\"players\": 123}.", html! {
                    input #players_url type="url" name="players_url" value=(status.players_url) maxlength="300" placeholder="https://";
                }))
            }
        }
        "minecraft" => {
            let minecraft = &config.minecraft;
            html! {
                h2 { "Minecraft server" }
                (switch("enabled", "Show the Minecraft server", "Its state, players, version and ping (Java edition). A panel stays up to date in the channel below, refreshed every 5 minutes.", minecraft.enabled))
                (field("address", "Server address", "play.example.com or 1.2.3.4:25565; SRV records are followed.", html! {
                    input #address type="text" name="address" value=(minecraft.address) maxlength="120" placeholder="play.example.com";
                }))
                (field("name", "Name", "", html! {
                    input #name type="text" name="name" value=(minecraft.name) maxlength="80" required;
                }))
                (textarea("description", "Description", "Optional, shown under the picture.", &minecraft.description, 3, 1800))
                (switch("show_players", "Show the names of the players online", "Servers only give a few of them.", minecraft.show_players))
                (channel_select("channel", "Panel channel", "Optional: without it, only /smp shows the server.", text_channels(), &minecraft.channel))
            }
        }
        _ => {
            let links = &config.lock_url;
            let scopes = [
                ("all", "Every channel, except the exempt ones below"),
                ("listed", "Only the channels listed below"),
            ];
            html! {
                h2 { "Link lock" }
                (switch("enabled", "Block links", "Messages with a link are deleted. Bot admins and staff are never affected. Same as the /lockurl command.", links.enabled))
                (choice("scope", "Where", "", &scopes, &links.scope))
                (checks("listed_channels", "Watched channels", "Used when \"Only the channels listed\" is chosen.", &channel_items, &links.channels))
                (checks("channels", "Channels where links are allowed", "Used when \"Every channel\" is chosen.", &channel_items, &links.exempt_channels))
                (textarea("domains", "Allowed websites", "One domain per line, for example tenor.com. Their subdomains are allowed too.", &links.allowed_domains.join("\n"), 4, 2000))
                (checks("x_channels", "Channels where x.com is blocked anyway", "x.com and twitter.com links are removed there even if allowed above.", &channel_items, &links.x_blocked_channels))
                (role_checks("roles", "Roles allowed to post links", "", &risks, &links.exempt_roles, "staff"))
                (number_input("timeout", "Timeout of the author (seconds)", "0: none.", links.timeout_secs, 0, 3600))
                (channel_select("review_channel", "Link review channel", "Optional. The member can send the removed link to the staff, who accepts it (the bot posts it) or refuses it here.", text_channels(), &links.review_channel))
            }
        }
    }
}

/// Fills the placeholders of the welcome text the way the bot does.
fn fill_welcome(template: &str, name: &str, server: &str, count: u64) -> String {
    template
        .replace("{user}", &format!("@{name}"))
        .replace("{name}", name)
        .replace("{server}", server)
        .replace("{count}", &thousands(count))
}

/// Mock of a message of the bot, as it appears on Discord.
fn discord_message(bot: &BotView, blocks: Markup) -> Markup {
    let name = match &bot.identity {
        Some(user) => user.username.as_str(),
        None => bot.def.name.as_str(),
    };
    html! {
        div.dmsg {
            img.avatar src=(bot_avatar(bot)) alt="" width="40" height="40";
            div.dmsg-body {
                div.dmsg-head { strong { (name) } span.dtag { "APP" } }
                div.dbox { (blocks) }
            }
        }
    }
}

/// What a section posts on Discord, built from the saved settings, and the
/// button that makes the bot send it for real.
fn guild_preview(
    session: &Session,
    bot: &BotView,
    context: &GuildContext,
    config: &GuildConfig,
    module: &str,
    base: &str,
) -> Markup {
    let guild = &context.guild;
    let me = session.user.display_name();
    let image = |file: &str, alt: &str, width: u32, height: u32| {
        html! {
            img.dimg src={ (base) "/preview/" (file) } alt=(alt) width=(width) height=(height) loading="lazy";
        }
    };
    let (blocks, extra, test_label, test_hint) = match module {
        "welcome" => {
            let welcome = &config.welcome;
            let count = guild.approximate_member_count.unwrap_or(1);
            let blocks = html! {
                p.dtext { (fill_welcome(&welcome.message, me, &guild.name, count)) }
                @if welcome.image { (image("welcome.png", "Welcome image", 1000, 360)) }
            };
            let public = preview_card(
                session,
                bot,
                blocks,
                html! {},
                base,
                module,
                "Send a test",
                "Sends this message to the welcome channel, as if you had just joined.",
            );
            return html! {
                (public)
                @if welcome.dm_enabled { (dm_preview(bot, config, me, &guild.name, count, base)) }
            };
        }
        "roles" => {
            let panel = &config.role_panel;
            let blocks = html! {
                p.dtitle.small { (panel.title) }
                @if panel.buttons.is_empty() {
                    p.dtext.muted { "Add buttons and save to see them here." }
                }
                @for button in &panel.buttons {
                    div.dsplit {
                        p.dtext {
                            strong { (button.label) }
                            @if !button.description.is_empty() { "\n" (button.description) }
                        }
                        span.dbtn {
                            @if !button.emoji.is_empty() { (button.emoji) " " }
                            (button.label)
                        }
                    }
                }
                p.dsmall { "Click a button to take or drop the role." }
            };
            (
                blocks,
                html! {},
                "Post the panel again",
                "Deletes the role panel and posts it again at the bottom of its channel.",
            )
        }
        "reminders" => {
            let reminders = &config.reminders;
            let french = reminders.channel_fr.is_some() || reminders.channel_en.is_none();
            let blocks = html! {
                (image(if french { "reminder-fr.png" } else { "reminder-en.png" }, "Banner", 1000, 232))
                (tips(french, base))
            };
            let extra = html! {
                @if reminders.channel_fr.is_some() && reminders.channel_en.is_some() {
                    p.hint { "The English channel gets the same message in English." }
                }
            };
            (
                blocks,
                extra,
                "Send now",
                "Posts the reminder in its channels right away.",
            )
        }
        "releases" => {
            let releases = &config.releases;
            let name = releases
                .repo
                .rsplit('/')
                .next()
                .filter(|n| !n.is_empty())
                .unwrap_or("Project");
            let role = releases
                .role
                .as_deref()
                .and_then(|id| context.roles.iter().find(|r| r.id == id));
            let blocks = html! {
                p.dtitle { (name) " v1.2.0" }
                p.dsmall { "New version" }
                @if let Some(role) = role { p.dtext { span.dmention { "@" (role.name) } } }
                p.dtext { "The notes of the release, as written on GitHub." }
                p.dsmall { "Setup.exe 9.0 MB · Portable.zip 10.0 MB" }
                div.dbtns { span.dbtn { "Download ↗" } span.dbtn { "Release notes ↗" } }
            };
            (
                blocks,
                html! {},
                "Send the latest",
                "Posts the latest release of the repository in the channel (without mentioning the role).",
            )
        }
        "github" => {
            let repo = config
                .github
                .repos
                .first()
                .map_or("owner/name", String::as_str);
            let blocks = html! {
                p.dsmall { (repo) " · just now" }
                p.dtext { strong { "octocat" } " pushed " strong { "2" } " commits to " code { "main" } }
                p.dtext { code { "2222222" } " Fix the launcher update\n" code { "1111111" } " Add a setting" }
                span.dbtn { "View on GitHub ↗" }
            };
            (
                blocks,
                html! {},
                "Send a sample",
                "Posts the latest commit of the first repository as a sample.",
            )
        }
        "status" | "minecraft" => {
            let blocks = html! {
                p.dtext {
                    @if module == "status" {
                        "A picture with each website (green: up, amber: slow, red: down) and its last checks, then the latest version, downloads, players and stars."
                    } @else {
                        "A picture with the server's icon, name, address, state, message of the day, players, version and ping, then the players online."
                    }
                }
                span.dbtn { "Refresh" }
            };
            (
                blocks,
                html! {},
                "Post the panel",
                "Checks right away and posts the panel in its channel (the previous one is deleted).",
            )
        }
        "twitch" => {
            let twitch = &config.twitch;
            let login = twitch.logins.first().map_or("streamer", String::as_str);
            let role = twitch
                .role
                .as_deref()
                .and_then(|id| context.roles.iter().find(|r| r.id == id));
            let blocks = html! {
                @if let Some(role) = role { p.dtext { span.dmention { "@" (role.name) } } }
                p.dtitle.small { "🔴 " (login) " is live on Twitch!" }
                p.dtext { "Title of the stream" }
                p.dsmall { "Game of the stream" }
                div.dthumb { "Stream picture" }
                span.dbtn { "Watch the stream ↗" }
            };
            (
                blocks,
                html! {},
                "Send a test",
                "Sends a sample alert to the alert channel.",
            )
        }
        "rules" | "verification" => {
            let rules = &config.rules;
            let own = module == "verification" && !rules.verify_under_rules();
            let blocks = if own {
                html! {
                    p.dtitle { (rules.verify_title) }
                    p.dtext { (rules.verify_text) }
                    span.dbtn.ok { "✅ Verify" }
                }
            } else {
                html! {
                    p.dtitle { (rules.title) }
                    p.dtext { (rules.text) }
                    @if rules.verify_under_rules() {
                        div.dsplit {
                            p.dtext { strong { "Accept the rules" } " to get access to the server." }
                            span.dbtn.ok { "Verify" }
                        }
                    }
                }
            };
            let extra = html! {
                @if rules.verification == "captcha"
                    || (matches!(rules.verification.as_str(), "activity" | "web") && rules.captcha_style == "text") {
                    p.hint { "After clicking Verify, members type the code of an image like this one:" }
                    img.captcha src={ (base) "/preview/captcha.png" } alt="Sample captcha" width="460" height="170" loading="lazy";
                }
            };
            let (label, hint) = if own {
                (
                    "Post the message again",
                    "Deletes the verification message and posts it again at the bottom of its channel.",
                )
            } else {
                (
                    "Post the panel again",
                    "Deletes the rules panel and posts it again at the bottom of its channel.",
                )
            };
            let module = if own { "verification" } else { "rules" };
            return preview_card(session, bot, blocks, extra, base, module, label, hint);
        }
        "tickets" => {
            let tickets = &config.tickets;
            let blocks = html! {
                p.dtitle { (tickets.title) }
                p.dtext { (tickets.text) }
                @if tickets.topics.is_empty() {
                    span.dbtn { "Open a ticket" }
                } @else {
                    span.dselect { "Choose a topic to open a ticket" }
                }
            };
            (
                blocks,
                html! {},
                "Post the panel again",
                "Deletes the ticket panel and posts it again at the bottom of its channel.",
            )
        }
        "levels" => {
            let level = config.xp.rewards.first().map_or(5, |reward| reward.level);
            let blocks = html! {
                p.dtitle.small { "Level up!" }
                p.dtext { "@" (me) " reached " strong { "level " (level) } "." }
            };
            let extra = html! {
                p.hint { "The card shown by the /rank command (sample numbers):" }
                (image("rank.png", "Rank card", 1000, 300))
            };
            (
                blocks,
                extra,
                "Send a test",
                "Sends a sample level-up to the announcement channel.",
            )
        }
        "logs" => {
            let blocks = html! {
                p.dtitle.small { "Message deleted" }
                p.dtext { strong { "Author:" } " @" (me) "\n" strong { "Channel:" } " #general\n> hello everyone" }
            };
            (
                blocks,
                html! {},
                "Send a test",
                "Sends a sample entry to the log channel.",
            )
        }
        _ => return html! {},
    };
    preview_card(
        session, bot, blocks, extra, base, module, test_label, test_hint,
    )
}

/// The anti-scam tips of the welcome DM and the reminders.
fn tips(french: bool, base: &str) -> Markup {
    let (scam_title, scam_text) = dc_core::tips::scam(french);
    let (dm_title, dm_text) = dc_core::tips::dms(french);
    html! {
        p.dtext { strong { (scam_title) } "\n" (scam_text) }
        p.dtext { strong { (dm_title) } "\n" (dm_text) }
        img.dimg src={ (base) "/preview/close-dms.png" } alt="Where to turn off DMs" loading="lazy";
    }
}

/// The private message sent to new members.
fn dm_preview(
    bot: &BotView,
    config: &GuildConfig,
    me: &str,
    server: &str,
    count: u64,
    base: &str,
) -> Markup {
    let welcome = &config.welcome;
    let english = !welcome.dm_text_en.is_empty();
    let version = |french: bool| {
        let text = if french || !english {
            &welcome.dm_text
        } else {
            &welcome.dm_text_en
        };
        let banner = if french && english {
            "dm-banner-fr.png"
        } else {
            "dm-banner-en.png"
        };
        html! {
            img.dimg src={ (base) "/preview/" (banner) } alt="Banner" width="1000" height="232" loading="lazy";
            p.dtext { (fill_welcome(text, me, server, count)) }
            @if welcome.dm_safety { (tips(french || !english, base)) }
            @if !welcome.dm_links.is_empty() || english {
                div.dbtns {
                    @for link in &welcome.dm_links { span.dbtn { (link.label) " ↗" } }
                    @if english {
                        span.dbtn { @if french { "🇬🇧 English" } @else { "🇫🇷 Français" } }
                    }
                }
            }
        }
    };
    html! {
        section.card.form {
            h2 { "Private message preview" }
            p.muted {
                "Sent to each new member who accepts private messages."
                @if english { " The language button switches between the two versions." }
            }
            (discord_message(bot, version(true)))
            @if english {
                p.hint { "English version:" }
                (discord_message(bot, version(false)))
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn preview_card(
    session: &Session,
    bot: &BotView,
    blocks: Markup,
    extra: Markup,
    base: &str,
    module: &str,
    test_label: &str,
    test_hint: &str,
) -> Markup {
    html! {
        section.card.form {
            h2 { "Preview" }
            p.muted { "What the bot posts with the saved settings. Discord also renders formatting such as **bold**." }
            (discord_message(bot, blocks))
            (extra)
            form.test-row method="post" action={ (base) "/" (module) "/test" } {
                (csrf_field(session))
                p.hint { (test_hint) " Save your changes first." }
                button.btn type="submit" { (test_label) }
            }
        }
    }
}

fn danger_zone(session: &Session, base: &str) -> Markup {
    html! {
        form.card.form.danger-zone method="post" action={ (base) "/reset" } {
            (csrf_field(session))
            h2 { "Danger zone" }
            p.muted {
                "Puts every section of this server back to its defaults. The bot takes its "
                "panels down. This cannot be undone: export the settings first (Backup)."
            }
            label.check {
                input type="checkbox" name="xp" value="on";
                span { "Also erase the XP and levels of every member" }
            }
            label.check {
                input type="checkbox" name="confirm" value="on" required;
                span { "I understand that this cannot be undone" }
            }
            div.form-actions {
                button.btn.danger type="submit" { "Reset everything" }
            }
        }
        form.card.form.danger-zone method="post" action={ (base) "/leave" } {
            (csrf_field(session))
            h2 { "Remove the bot from this server" }
            p.muted {
                "The bot leaves the server right away: its commands, panels and logs stop "
                "there. Its settings for this server are kept, so inviting it again brings "
                "everything back."
            }
            label.check {
                input type="checkbox" name="confirm" value="on" required;
                span { "Yes, remove the bot from this server" }
            }
            div.form-actions {
                button.btn.danger type="submit" { "Remove the bot" }
            }
        }
    }
}

fn backup(session: &Session, base: &str, guild: &str) -> Markup {
    html! {
        section.card.form {
            h2 { "Export the settings" }
            p.muted {
                "Downloads every setting of " strong { (guild) } " as a JSON file: keep it as a "
                "backup, or import it on another server or another bot."
            }
            div.form-actions {
                a.btn.primary href={ (base) "/export" } download { (icon(ICON_DOWNLOAD)) "Download the settings" }
            }
        }
        form.card.form method="post" action={ (base) "/import" } enctype="multipart/form-data" {
            (csrf_field(session))
            h2 { "Import settings" }
            p.muted {
                "Only the sections in the file are replaced (a file with just "
                code { "twitch" } " only changes Twitch); the others stay as they are. "
                "Channel and role ids must exist on this server."
            }
            (field("file", "Settings file", "A file downloaded above, 512 KB at most.", html! {
                input #file type="file" name="file" accept=".json,application/json";
            }))
            (textarea("json", "Or paste the settings", "", "", 6, 500_000))
            div.form-actions {
                button.btn.primary type="submit" { "Import" }
            }
        }
    }
}

/// Is a section in use? `None` for sections without an on/off state.
fn section_on(config: &GuildConfig, id: &str) -> Option<bool> {
    Some(match id {
        "welcome" => {
            let w = &config.welcome;
            w.enabled || w.dm_enabled || !w.ping_channels.is_empty()
        }
        "rules" => config.rules.enabled,
        "verification" => config.rules.verifying(),
        "tickets" => config.tickets.enabled,
        "levels" => config.xp.enabled,
        "logs" => config.logs.enabled,
        "links" => config.lock_url.enabled,
        "security" => {
            let a = &config.anti_scam;
            a.invites || a.insider || a.images
        }
        "roles" => config.role_panel.enabled,
        "assistant" => config.assistant.enabled,
        "twitch" => config.twitch.enabled,
        "reminders" => config.reminders.enabled,
        "releases" => config.releases.enabled,
        "github" => config.github.enabled,
        "status" => config.status.enabled,
        "minecraft" => config.minecraft.enabled,
        _ => return None,
    })
}

/// Sections of the sidebar, by group.
const NAV_GROUPS: &[(&str, &[&str])] = &[
    ("Server", &["admins", "logs", BACKUP]),
    (
        "Members",
        &["welcome", "rules", "verification", "roles", "levels"],
    ),
    ("Protection", &["links", "security", "reminders"]),
    ("Community", &["tickets", "assistant"]),
    (
        "Integrations",
        &["twitch", "releases", "github", "status", "minecraft"],
    ),
];

fn section_icon(id: &str) -> &'static str {
    match id {
        "admins" => r#"<path d="M12 3l7 3v5c0 4.5-3 8-7 10-4-2-7-5.5-7-10V6z"/>"#,
        "logs" => r#"<path d="M9 6h11M9 12h11M9 18h11M4.5 6h.01M4.5 12h.01M4.5 18h.01"/>"#,
        "welcome" => {
            r#"<circle cx="9" cy="8" r="4"/><path d="M2 21c0-4 3-7 7-7s7 3 7 7M19 8v6M16 11h6"/>"#
        }
        "rules" => r#"<path d="M6 3h9l4 4v14H6z"/><path d="M9 12h7M9 16h7M9 8h3"/>"#,
        "verification" => r#"<circle cx="12" cy="12" r="9"/><path d="M8 12.5l3 3 5-6"/>"#,
        "roles" => r#"<path d="M3 12V3h9l9 9-9 9z"/><circle cx="7.5" cy="7.5" r="1.5"/>"#,
        "levels" => r#"<path d="M5 20v-7M12 20V5M19 20v-10"/>"#,
        "links" => {
            r#"<path d="M10 14a4 4 0 0 0 5.7 0l3-3a4 4 0 0 0-5.7-5.7l-1 1"/><path d="M14 10a4 4 0 0 0-5.7 0l-3 3a4 4 0 0 0 5.7 5.7l1-1"/>"#
        }
        "security" => r#"<path d="M12 3l9.5 17h-19z"/><path d="M12 10v4.5M12 17.5h.01"/>"#,
        "reminders" => r#"<path d="M6 16v-5a6 6 0 0 1 12 0v5l2 2H4z"/><path d="M10 21h4"/>"#,
        "tickets" => r#"<path d="M4 5h16v11H9l-5 4z"/>"#,
        "assistant" => {
            r#"<path d="M12 3l1.8 5.2L19 10l-5.2 1.8L12 17l-1.8-5.2L5 10l5.2-1.8z"/><path d="M19 15l.7 1.8 1.8.7-1.8.7L19 20l-.7-1.8-1.8-.7 1.8-.7z"/>"#
        }
        "twitch" => {
            r#"<rect x="3" y="4" width="18" height="13" rx="2"/><path d="M8 21h8M12 17v4"/>"#
        }
        "backup" => r#"<path d="M12 3v12M7 10l5 5 5-5M5 20h14"/>"#,
        "releases" => r#"<path d="M12 3l2.5 5.5L20 9l-4 4 1 6-5-3-5 3 1-6-4-4 5.5-.5z"/>"#,
        "github" => {
            r#"<circle cx="6" cy="6" r="2.5"/><circle cx="6" cy="18" r="2.5"/><circle cx="18" cy="8" r="2.5"/><path d="M6 8.5v7M18 10.5c0 4-6 3-11 6"/>"#
        }
        "status" => r#"<path d="M3 12h4l3-7 4 14 3-7h4"/>"#,
        "minecraft" => {
            r#"<rect x="4" y="4" width="16" height="16" rx="2"/><path d="M8 9h3v3H8zM13 9h3v3h-3zM10 13h4v4h-4z"/>"#
        }
        _ => r#"<path d="M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13"/>"#,
    }
}

fn section_label(id: &str) -> &'static str {
    match id {
        BACKUP => "Backup",
        DANGER => "Danger zone",
        _ => MODULES
            .iter()
            .find(|(module, _)| *module == id)
            .map_or("", |(_, label)| label),
    }
}

/// The sections of a server, grouped, with a dot for the ones in use.
fn side_nav(base: &str, config: &GuildConfig, module: &str) -> Markup {
    let link = |id: &str| {
        let active = id == module;
        html! {
            a.side-link.active[active].danger[id == DANGER] href={ (base) "/" (id) }
                aria-current=[active.then_some("page")] {
                svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor"
                    stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" {
                    (PreEscaped(section_icon(id)))
                }
                span.side-label { (section_label(id)) }
                @if let Some(on) = section_on(config, id) {
                    span.state-dot.on[on] title=(if on { "On" } else { "Off" }) {}
                }
            }
        }
    };
    html! {
        nav.side-nav aria-label="Server settings" {
            @for (group, ids) in NAV_GROUPS {
                p.side-group { (group) }
                @for id in *ids { (link(id)) }
            }
            div.side-sep {}
            (link(DANGER))
        }
    }
}

pub fn guild_config(
    session: &Session,
    bot: &BotView,
    context: &GuildContext,
    config: &GuildConfig,
    module: &str,
    flash: Option<&str>,
    error: Option<&str>,
) -> Markup {
    let guild = &context.guild;
    let servers = format!("/bots/{}/servers", bot.def.slug);
    let base = format!("{servers}/{}", guild.id);
    let content = html! {
        @if let Some(message) = error {
            div.notice.bad role="alert" { (message) }
        }
        div.server-head {
            @if let Some(hash) = &guild.icon {
                img.avatar.sq src={ (CDN) "/icons/" (guild.id) "/" (hash) ".png?size=64" }
                    alt="" width="36" height="36";
            } @else {
                span.avatar.sq.placeholder aria-hidden="true" { (guild.name.chars().next().unwrap_or('?')) }
            }
            div.row-text {
                strong { (guild.name) }
                span.mono.muted { (guild.id) }
            }
            a.btn.small href=(servers) { (icon(ICON_BACK)) "All servers" }
        }
        // Small screens: the sections fold into a menu.
        details.side-menu {
            summary {
                span.side-group { "Section" }
                strong { (section_label(module)) }
            }
            (side_nav(&base, config, module))
        }
        div.guild-layout {
            (side_nav(&base, config, module))
            div.guild-main {
                @if module == DANGER {
                    (danger_zone(session, &base))
                } @else if module == BACKUP {
                    (backup(session, &base, &guild.name))
                } @else {
                    form.card.form method="post" action={ (base) "/" (module) } data-track {
                        (csrf_field(session))
                        (guild_fields(module, bot, context, config))
                        div.form-actions {
                            button.btn.primary type="submit" { "Save" }
                        }
                    }
                    (guild_preview(session, bot, context, config, module, &base))
                }
            }
        }
    };
    bot_page(session, bot, Tab::Servers, flash, content)
}

// ---------------------------------------------------------------- verification

/// Verification page, inside a Discord Activity (`client_id` set) or opened
/// from a Verify link. `base` is the prefix of its own URLs, which differs
/// behind Discord's proxy. Both captcha styles are in the page; the script
/// shows the one the server uses.
pub fn verification(client_id: Option<&str>, bot_name: &str, base: &str) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                meta name="color-scheme" content="dark";
                meta name="robots" content="noindex";
                title { "Verification" @if !bot_name.is_empty() { " · " (bot_name) } }
                link rel="stylesheet" href={ (base) "app.css?v=" (VERIFY_VERSION.as_str()) };
            }
            body.public.activity data-base=(base) data-client-id=[client_id] {
                main.activity-card {
                    div.login-mark { (icon(ICON_LOGO)) }
                    p.eyebrow #bot-name { (bot_name) }
                    h1 { "Verification" }
                    section #loading {
                        p.lead #loading-text { "Checking your Discord account…" }
                    }
                    section #challenge hidden {
                        p.lead #intro {
                            "Hi " strong #who {} ". "
                            span #intro-text {}
                        }
                        div #text-captcha hidden {
                            img.captcha #captcha alt="Captcha" width="460" height="170";
                            form #form autocomplete="off" {
                                input #answer type="text" name="answer" maxlength="5"
                                    autocapitalize="characters" spellcheck="false"
                                    placeholder="ABC12" aria-label="Code shown in the image";
                                div.actions {
                                    button.btn #another type="button" { "Another image" }
                                    button.btn.primary #submit type="submit" { "Verify" }
                                }
                            }
                        }
                        div #slide-captcha hidden {
                            div.slide-frame #slide-frame {
                                img #slide-bg alt="Puzzle" draggable="false";
                                img.slide-piece #slide-piece alt="" draggable="false";
                            }
                            div.slider #slider role="slider" tabindex="0" aria-label="Slide the piece into place"
                                aria-valuemin="0" aria-valuenow="0" {
                                div.slider-fill #slider-fill {}
                                div.slider-handle #slider-handle { "→" }
                                span.slider-hint { "Slide to complete the puzzle" }
                            }
                            div.actions {
                                button.btn #another-slide type="button" { "Another puzzle" }
                            }
                        }
                        p.activity-error #challenge-text role="alert" {}
                    }
                    section #done hidden {
                        p.lead #done-text { "You are verified. Welcome!" }
                    }
                    section #failed hidden {
                        p.lead #failed-text {}
                        button.btn #retry type="button" { "Try again" }
                    }
                }
                script src={ (base) "app.js?v=" (VERIFY_VERSION.as_str()) } defer {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn started_dates() {
        assert_eq!(
            format_started("2026-11-11T10:12:33.123Z").as_deref(),
            Some("2026-11-11 10:12 UTC")
        );
        assert_eq!(format_started("0001-01-01T00:00:00Z"), None);
        assert_eq!(format_started("bad"), None);
    }

    #[test]
    fn avatars() {
        assert!(default_avatar("music").contains("/embed/avatars/"));
        assert!(avatar_url("1", Some("abc")).ends_with("/avatars/1/abc.png?size=128"));
    }
}
