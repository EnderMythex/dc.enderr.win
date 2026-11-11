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
    guilds::{DANGER, MODULES},
    routes::BotView,
};

const BRAND: &str = "dc.enderr.win";
/// Stylesheet URL carrying a hash of its content, so a new build is never
/// served from the browser cache.
static STYLESHEET: LazyLock<String> = LazyLock::new(|| {
    let hash = dc_core::crypto::sha256_hex(include_str!("../assets/app.css"));
    format!("/assets/app.css?v={}", &hash[..12])
});
const CDN: &str = "https://cdn.discordapp.com";

// ---------------------------------------------------------------- icons

const ICON_LOGO: &str = r#"<svg viewBox="0 0 24 24" width="22" height="22" aria-hidden="true"><rect width="24" height="24" rx="6" fill="currentColor"/><path d="M7 8.5h10M7 12h10M7 15.5h6" stroke="black" stroke-width="1.8" stroke-linecap="round"/></svg>"#;
const ICON_DISCORD: &str = r#"<svg viewBox="0 0 24 24" width="18" height="18" fill="currentColor" aria-hidden="true"><path d="M19.6 5.3A17.5 17.5 0 0 0 15.3 4l-.5 1.1a16.2 16.2 0 0 0-5.6 0L8.7 4a17.5 17.5 0 0 0-4.3 1.3C1.7 9.4.9 13.3 1.3 17.2a17.6 17.6 0 0 0 5.3 2.7l1.1-1.8c-.6-.2-1.2-.5-1.8-.9l.4-.3a12.5 12.5 0 0 0 11.4 0l.4.3c-.6.4-1.2.7-1.8.9l1.1 1.8a17.6 17.6 0 0 0 5.3-2.7c.5-4.5-.8-8.4-3.1-11.9ZM8.7 14.8c-1 0-1.9-1-1.9-2.1s.8-2.1 1.9-2.1 1.9 1 1.9 2.1-.8 2.1-1.9 2.1Zm6.6 0c-1 0-1.9-1-1.9-2.1s.8-2.1 1.9-2.1 1.9 1 1.9 2.1-.8 2.1-1.9 2.1Z"/></svg>"#;
const ICON_PLAY: &str = r#"<svg viewBox="0 0 24 24" width="15" height="15" fill="currentColor" aria-hidden="true"><path d="M7 4.5v15l12-7.5z"/></svg>"#;
const ICON_STOP: &str = r#"<svg viewBox="0 0 24 24" width="15" height="15" fill="currentColor" aria-hidden="true"><rect x="6" y="6" width="12" height="12" rx="1.5"/></svg>"#;
const ICON_RESTART: &str = r#"<svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M20 11a8 8 0 1 0-2.3 6.3"/><path d="M20 4v7h-7"/></svg>"#;
const ICON_EXTERNAL: &str = r#"<svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M14 5h5v5M19 5l-8 8M11 6H6v12h12v-5"/></svg>"#;
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
            }
            body class=(body_class) { (body) }
        }
    }
}

fn footer() -> Markup {
    html! {
        footer.foot {
            span { (BRAND) }
            span { "Discord bot panel" }
        }
    }
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

pub fn login(error: Option<&str>) -> Markup {
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
                }
                (rule())
                div.band.spacer {}
                (footer())
            }
        },
    )
}

pub fn error_page(status: u16, title: &str, message: &str) -> Markup {
    document(
        title,
        "public",
        html! {
            div.frame {
                div.band.spacer {}
                (rule())
                main.band.hero {
                    p.error-code { (status) }
                    h1 { (title) }
                    p.lead { (message) }
                    a.btn href="/" { "Back to home" }
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

pub fn bot_servers(session: &Session, bot: &BotView, guilds: Result<Vec<Guild>, &str>) -> Markup {
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
    bot_page(session, bot, Tab::Servers, None, content)
}

pub fn bot_logs(
    session: &Session,
    bot: &BotView,
    logs: Result<Vec<LogLine>, ActionError>,
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
            form.card.form method="post" enctype="multipart/form-data"
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
        form.card.form method="post" action={ "/bots/" (bot.def.slug) "/settings" } {
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
    };
    bot_page(session, bot, Tab::Settings, flash, content)
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

fn role_options(roles: &[Role], current: Option<&str>) -> Markup {
    html! {
        option value="" { "— None —" }
        @for role in roles {
            option value=(role.id) selected[current == Some(role.id.as_str())] { "@" (role.name) }
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
fn guild_fields(module: &str, context: &GuildContext, config: &GuildConfig) -> Markup {
    let text_channels = || context.channels.iter().filter(|c| c.is_text());
    let categories = || context.channels.iter().filter(|c| c.is_category());
    let roles = &context.roles;
    let role_items: Vec<(&str, String)> = roles
        .iter()
        .map(|role| (role.id.as_str(), format!("@{}", role.name)))
        .collect();
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
                (checks("roles", "Admin roles", "Every member with one of these roles is an admin.", &role_items, &admins.roles))
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
                (checks("roles", "Roles given on arrival", "The bot's role must be above these roles.", &role_items, &welcome.auto_roles))
            }
        }
        "rules" => {
            let rules = &config.rules;
            let modes = [
                ("none", "None — rules only"),
                ("button", "Button — one click to accept"),
                ("captcha", "Captcha — type the code from an image"),
                ("activity", "Activity — captcha in a Discord Activity"),
            ];
            html! {
                h2 { "Rules and verification" }
                (switch("enabled", "Post the rules panel", "The bot posts the panel in the channel below and keeps it up to date.", rules.enabled))
                (channel_select("channel", "Rules channel", "", text_channels(), &rules.channel))
                (field("title", "Title", "", html! {
                    input #title type="text" name="title" value=(rules.title) maxlength="80" required;
                }))
                (textarea("text", "Rules", "Discord formatting works: **bold**, lists, links.", &rules.text, 9, 1800))
                (choice("verification", "Verification", "How members prove they accept the rules. The Activity opens a small window inside Discord; it needs a one-time setup by the administrator and falls back to the image captcha until then.", &modes, &rules.verification))
                (field("role", "Verified role", "Given once verified. Use it to unlock the rest of the server.", html! {
                    select #role name="role" { (role_options(roles, rules.verified_role.as_deref())) }
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
                (checks("roles", "Staff roles", "These roles can see, claim and close tickets.", &role_items, &tickets.staff_roles))
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
                            select name="reward_role" aria-label="Role" {
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
        _ => {
            let links = &config.lock_url;
            html! {
                h2 { "Link lock" }
                (switch("enabled", "Block links", "Messages with a link are deleted. Bot admins are never affected. Same as the /lockurl command.", links.enabled))
                (textarea("domains", "Allowed websites", "One domain per line, for example youtube.com. Their subdomains are allowed too.", &links.allowed_domains.join("\n"), 4, 2000))
                (checks("roles", "Roles allowed to post links", "", &role_items, &links.exempt_roles))
                (checks("channels", "Channels where links are allowed", "", &channel_items, &links.exempt_channels))
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
            (
                blocks,
                html! {},
                "Send a test",
                "Sends this message to the welcome channel, as if you had just joined.",
            )
        }
        "rules" => {
            let rules = &config.rules;
            let blocks = html! {
                p.dtitle { (rules.title) }
                p.dtext { (rules.text) }
                @if rules.verification != "none" {
                    div.dsplit {
                        p.dtext { strong { "Accept the rules" } " to get access to the server." }
                        span.dbtn.ok { "Verify" }
                    }
                }
            };
            let extra = html! {
                @if matches!(rules.verification.as_str(), "captcha" | "activity") {
                    p.hint { "After clicking Verify, members type the code of an image like this one:" }
                    img.captcha src={ (base) "/preview/captcha.png" } alt="Sample captcha" width="460" height="170" loading="lazy";
                }
            };
            (
                blocks,
                extra,
                "Post the panel again",
                "Deletes the rules panel and posts it again at the bottom of its channel.",
            )
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
                "Puts every section of this server back to its defaults: admins, welcome, rules, "
                "tickets, levels, logs and link lock. The bot takes its rules and ticket panels "
                "down. This cannot be undone."
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
    let pill = |id: &str, label: &str| {
        html! {
            a.pill.active[id == module].danger[id == DANGER] href={ (base) "/" (id) }
                aria-current=[(id == module).then_some("page")] { (label) }
        }
    };
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
        nav.pills aria-label="Server settings" {
            @for (id, label) in MODULES { (pill(id, label)) }
            (pill(DANGER, "Danger zone"))
        }
        @if module == DANGER {
            (danger_zone(session, &base))
        } @else {
            form.card.form method="post" action={ (base) "/" (module) } {
                (csrf_field(session))
                (guild_fields(module, context, config))
                div.form-actions {
                    button.btn.primary type="submit" { "Save" }
                }
            }
            (guild_preview(session, bot, context, config, module, &base))
        }
    };
    bot_page(session, bot, Tab::Servers, flash, content)
}

// ---------------------------------------------------------------- activity

/// Verification page loaded inside Discord (see `activity.rs`). `base` is the
/// prefix of its own URLs, which differs behind Discord's proxy.
pub fn activity(client_id: &str, bot_name: &str, base: &str) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                meta name="color-scheme" content="dark";
                meta name="robots" content="noindex";
                title { "Verification · " (bot_name) }
                link rel="stylesheet" href={ (base) "app.css" };
            }
            body.public.activity data-base=(base) data-client-id=(client_id) {
                main.activity-card {
                    div.login-mark { (icon(ICON_LOGO)) }
                    h1 { "Verification" }
                    section #loading {
                        p.lead #loading-text { "Checking your Discord account…" }
                    }
                    section #challenge hidden {
                        p.lead { "Hi " strong #who {} ". Type the characters shown in the image to get access to the server." }
                        img.captcha #captcha alt="Captcha" width="460" height="170";
                        form #form autocomplete="off" {
                            input #answer type="text" name="answer" maxlength="5" required
                                autocapitalize="characters" spellcheck="false"
                                placeholder="ABC12" aria-label="Code shown in the image";
                            div.actions {
                                button.btn #another type="button" { "Another image" }
                                button.btn.primary #submit type="submit" { "Verify" }
                            }
                        }
                        p.activity-error #challenge-text role="alert" {}
                    }
                    section #done hidden {
                        p.lead #done-text { "You are verified. Welcome! This window closes by itself." }
                    }
                    section #failed hidden {
                        p.lead #failed-text {}
                        button.btn #retry type="button" { "Try again" }
                    }
                }
                script src={ (base) "app.js" } defer {}
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
