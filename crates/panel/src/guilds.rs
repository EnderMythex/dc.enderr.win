//! Per-server configuration of a bot: admins, welcome, rules, tickets,
//! levels, logs and link lock. Saved to `bots/<slug>/guilds/<id>.json`, which
//! the bot picks up within a few seconds.

use axum::{
    extract::{Multipart, Path, Query, State},
    http::header,
    response::{IntoResponse, Redirect},
};
use axum_extra::extract::Form;
use std::sync::Arc;

use dc_core::{
    cards::{self, Tone},
    guild::{GuildConfig, Reward, Xp},
    render,
    requests::{Answer, Request},
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    AppState, audit,
    auth::{CsrfForm, Session},
    bridge,
    discord::{DiscordError, GuildContext},
    error::AppError,
    routes::{BotView, Flash, Page, owned_bot},
    views,
};

/// Sections of a server's configuration: `(path segment, label)`.
pub const MODULES: &[(&str, &str)] = &[
    ("admins", "Admins"),
    ("welcome", "Welcome"),
    ("rules", "Rules"),
    ("verification", "Verification"),
    ("tickets", "Tickets"),
    ("levels", "Levels"),
    ("logs", "Logs"),
    ("links", "Link lock"),
    ("security", "Anti-scam"),
    ("roles", "Role panel"),
    ("assistant", "AI assistant"),
    ("twitch", "Twitch"),
    ("reminders", "Safety reminders"),
];

fn module_label(module: &str) -> &str {
    MODULES
        .iter()
        .find(|(id, _)| *id == module)
        .map_or(module, |(_, label)| label)
}

/// Extra page next to the sections: reset of the whole server.
pub const DANGER: &str = "danger";
/// Extra page: export and import of the settings as a file.
pub const BACKUP: &str = "backup";

/// Sections whose output can be sent to Discord as a test.
const TESTABLE: &[&str] = &[
    "welcome",
    "rules",
    "verification",
    "tickets",
    "levels",
    "logs",
    "roles",
    "reminders",
    "twitch",
];

/// Checks access and loads everything a server page needs. 404 when the bot
/// is not on that server, so ids cannot be probed.
async fn page_data(
    state: &AppState,
    session: &Session,
    slug: &str,
    guild_id: &str,
    module: &str,
) -> Result<(BotView, Arc<GuildContext>, GuildConfig), AppError> {
    let known = module == DANGER || module == BACKUP || MODULES.iter().any(|(id, _)| *id == module);
    if !dc_core::is_snowflake(guild_id) || !known {
        return Err(AppError::NotFound);
    }
    let def = owned_bot(state, session, slug).await?;
    let token = def.token().ok_or(AppError::NotFound)?;
    let context = state
        .discord
        .guild_context(&token, guild_id)
        .await
        .map_err(|error| match error {
            DiscordError::Unauthorized => AppError::NotFound,
            DiscordError::Status(status) if status.is_client_error() => AppError::NotFound,
            other => AppError::Discord(other),
        })?;
    let config = GuildConfig::load(&def.dir, guild_id)?;
    let bot = BotView::load(state, def).await;
    Ok((bot, context, config))
}

pub async fn home(
    state: State<AppState>,
    session: Session,
    Path((slug, guild_id)): Path<(String, String)>,
    flash: Query<Flash>,
) -> Page {
    let first = MODULES[0].0.to_owned();
    module(state, session, Path((slug, guild_id, first)), flash).await
}

pub async fn module(
    State(state): State<AppState>,
    session: Session,
    Path((slug, guild_id, module)): Path<(String, String, String)>,
    Query(flash): Query<Flash>,
) -> Page {
    let (bot, context, config) = page_data(&state, &session, &slug, &guild_id, &module).await?;
    // The answer of the bot to a test or a reset, shown once.
    let answer = state.notices.take(session.key());
    let (flash, error) = match &answer {
        Some(answer) if answer.ok => (Some(answer.message.as_str()), None),
        Some(answer) => (None, Some(answer.message.as_str())),
        None => (flash.text(), None),
    };
    let page = views::guild_config(&session, &bot, &context, &config, &module, flash, error);
    Ok(page.into_response())
}

/// One form type for every section: each section reads the fields it uses.
/// Checkboxes are present when ticked and absent otherwise.
#[derive(Deserialize, Default)]
#[serde(default)]
pub struct GuildForm {
    csrf: String,
    enabled: Option<String>,
    users: String,
    roles: Vec<String>,
    channels: Vec<String>,
    channel: String,
    log_channel: String,
    category: String,
    role: String,
    remove_role: String,
    captcha_style: String,
    ping_seconds: String,
    dm_enabled: Option<String>,
    dm_text: String,
    dm_text_en: String,
    dm_safety: Option<String>,
    dm_links: String,
    scope: String,
    listed_channels: Vec<String>,
    x_channels: Vec<String>,
    review_channel: String,
    timeout: String,
    invites: Option<String>,
    insider: Option<String>,
    images: Option<String>,
    button_role: Vec<String>,
    button_label: Vec<String>,
    button_emoji: Vec<String>,
    button_description: Vec<String>,
    context: String,
    max_length: String,
    logins: String,
    channel_fr: String,
    channel_en: String,
    interval: String,
    title: String,
    text: String,
    image: Option<String>,
    verification: String,
    topics: String,
    max_open: String,
    min_xp: String,
    max_xp: String,
    cooldown: String,
    announce: String,
    reward_level: Vec<String>,
    reward_role: Vec<String>,
    domains: String,
    log_message_delete: Option<String>,
    log_message_edit: Option<String>,
    log_channels: Option<String>,
    log_members: Option<String>,
    log_moderation: Option<String>,
}

fn optional(value: String) -> Option<String> {
    Some(value).filter(|v| !v.trim().is_empty())
}

/// Splits a free-text list on spaces, commas and new lines.
fn words(text: &str) -> Vec<String> {
    text.split(|c: char| c.is_whitespace() || c == ',')
        .filter(|word| !word.is_empty())
        .map(str::to_owned)
        .collect()
}

fn lines(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

fn number(text: &str, fallback: u32) -> u32 {
    text.trim().parse().unwrap_or(fallback)
}

/// Copies the submitted fields of one section into the configuration.
/// Out-of-range or malformed values are fixed by `GuildConfig::sanitize`.
fn apply_form(config: &mut GuildConfig, module: &str, form: GuildForm) {
    let enabled = form.enabled.is_some();
    match module {
        "admins" => {
            config.admins.users = words(&form.users);
            config.admins.roles = form.roles;
        }
        "welcome" => {
            config.welcome.enabled = enabled;
            config.welcome.channel = optional(form.channel);
            config.welcome.message = form.text;
            config.welcome.image = form.image.is_some();
            config.welcome.auto_roles = form.roles;
            config.welcome.ping_channels = form.channels;
            config.welcome.ping_seconds = number(&form.ping_seconds, 3);
            config.welcome.dm_enabled = form.dm_enabled.is_some();
            config.welcome.dm_text = form.dm_text;
            config.welcome.dm_text_en = form.dm_text_en;
            config.welcome.dm_safety = form.dm_safety.is_some();
            config.welcome.dm_links = lines(&form.dm_links)
                .iter()
                .filter_map(|line| dc_core::guild::parse_link_button(line))
                .collect();
        }
        "rules" => {
            config.rules.enabled = enabled;
            config.rules.channel = optional(form.channel);
            config.rules.title = form.title;
            config.rules.text = form.text;
        }
        "verification" => {
            config.rules.verify_enabled = Some(enabled);
            config.rules.verify_channel = optional(form.channel);
            config.rules.verify_title = form.title;
            config.rules.verify_text = form.text;
            config.rules.verification = form.verification;
            config.rules.verified_role = optional(form.role);
            config.rules.remove_role = optional(form.remove_role);
            config.rules.captcha_style = form.captcha_style;
        }
        "tickets" => {
            config.tickets.enabled = enabled;
            config.tickets.panel_channel = optional(form.channel);
            config.tickets.category = optional(form.category);
            config.tickets.log_channel = optional(form.log_channel);
            config.tickets.staff_roles = form.roles;
            config.tickets.title = form.title;
            config.tickets.text = form.text;
            config.tickets.topics = lines(&form.topics);
            config.tickets.max_open = number(&form.max_open, 1);
        }
        "levels" => {
            let defaults = Xp::default();
            config.xp.enabled = enabled;
            config.xp.min_xp = number(&form.min_xp, defaults.min_xp);
            config.xp.max_xp = number(&form.max_xp, defaults.max_xp);
            config.xp.cooldown_secs = number(&form.cooldown, defaults.cooldown_secs);
            config.xp.announce = form.announce;
            config.xp.announce_channel = optional(form.channel);
            config.xp.ignored_channels = form.channels;
            config.xp.rewards = form
                .reward_level
                .iter()
                .zip(&form.reward_role)
                .filter_map(|(level, role)| {
                    Some(Reward {
                        level: level.trim().parse().ok()?,
                        role: role.clone(),
                    })
                })
                .collect();
        }
        "logs" => {
            config.logs.enabled = enabled;
            config.logs.channel = optional(form.channel);
            config.logs.message_delete = form.log_message_delete.is_some();
            config.logs.message_edit = form.log_message_edit.is_some();
            config.logs.channels = form.log_channels.is_some();
            config.logs.members = form.log_members.is_some();
            config.logs.moderation = form.log_moderation.is_some();
        }
        "links" => {
            config.lock_url.enabled = enabled;
            config.lock_url.exempt_roles = form.roles;
            config.lock_url.exempt_channels = form.channels;
            config.lock_url.allowed_domains = lines(&form.domains);
            config.lock_url.scope = form.scope;
            config.lock_url.channels = form.listed_channels;
            config.lock_url.x_blocked_channels = form.x_channels;
            config.lock_url.review_channel = optional(form.review_channel);
            config.lock_url.timeout_secs = number(&form.timeout, 0);
        }
        "security" => {
            config.anti_scam.invites = form.invites.is_some();
            config.anti_scam.insider = form.insider.is_some();
            config.anti_scam.images = form.images.is_some();
            config.anti_scam.staff_channel = optional(form.channel);
        }
        "roles" => {
            config.role_panel.enabled = enabled;
            config.role_panel.channel = optional(form.channel);
            config.role_panel.title = form.title;
            let field =
                |list: &[String], index: usize| list.get(index).cloned().unwrap_or_default();
            config.role_panel.buttons = (0..form.button_role.len())
                .filter_map(|index| {
                    let role = field(&form.button_role, index);
                    let label = field(&form.button_label, index);
                    (!role.is_empty()).then(|| dc_core::guild::RoleButton {
                        label: if label.trim().is_empty() {
                            "Role".into()
                        } else {
                            label
                        },
                        role,
                        emoji: field(&form.button_emoji, index),
                        description: field(&form.button_description, index),
                    })
                })
                .collect();
        }
        "assistant" => {
            config.assistant.enabled = enabled;
            config.assistant.context = form.context;
            config.assistant.channels = form.channels;
            config.assistant.max_length = number(&form.max_length, 600);
            config.assistant.cooldown_secs = number(&form.cooldown, 8);
        }
        "twitch" => {
            config.twitch.enabled = enabled;
            config.twitch.channel = optional(form.channel);
            config.twitch.role = optional(form.role);
            config.twitch.logins = lines(&form.logins);
        }
        "reminders" => {
            config.reminders.enabled = enabled;
            config.reminders.channel_fr = optional(form.channel_fr);
            config.reminders.channel_en = optional(form.channel_en);
            config.reminders.interval_hours = number(&form.interval, 48);
        }
        _ => {}
    }
}

pub async fn save(
    State(state): State<AppState>,
    session: Session,
    Path((slug, guild_id, module)): Path<(String, String, String)>,
    Form(form): Form<GuildForm>,
) -> Page {
    session.check_csrf(&form.csrf)?;
    if module == DANGER || module == BACKUP {
        return Err(AppError::NotFound);
    }
    let (bot, context, mut config) = page_data(&state, &session, &slug, &guild_id, &module).await?;
    let before = serde_json::to_value(&config).unwrap_or_default();
    apply_form(&mut config, &module, form);
    config.save(&bot.def.dir, &guild_id)?;
    tracing::info!(
        user = %session.user.id, bot = %bot.def.slug, guild = %guild_id, module,
        "server settings saved"
    );
    // What the bot will use: the saved file, once sanitized.
    let saved = GuildConfig::load(&bot.def.dir, &guild_id).unwrap_or(config);
    let after = serde_json::to_value(&saved).unwrap_or_default();
    let changed = audit::changes(&before, &after, &context);
    if !changed.is_empty() {
        let label = module_label(&module);
        let action = format!(
            "{label} saved on {}: {}",
            context.guild.name,
            changed.join("; ")
        );
        audit::record(&state, &session, &bot.def.slug, action).await;
    }
    let to = format!("/bots/{}/servers/{guild_id}/{module}?m=guild", bot.def.slug);
    Ok(Redirect::to(&to).into_response())
}

/// POST .../{module}/test — asks the bot to post a sample of a section.
pub async fn test(
    State(state): State<AppState>,
    session: Session,
    Path((slug, guild_id, module)): Path<(String, String, String)>,
    Form(form): Form<CsrfForm>,
) -> Page {
    session.check_csrf(&form.csrf)?;
    if !TESTABLE.contains(&module.as_str()) {
        return Err(AppError::NotFound);
    }
    let (bot, context, _) = page_data(&state, &session, &slug, &guild_id, &module).await?;
    let request = Request::Test {
        guild_id: guild_id.clone(),
        module: module.clone(),
        user_id: session.user.id.clone(),
    };
    let answer = bridge::ask_bot(&bot.def.dir, &request).await;
    tracing::info!(
        user = %session.user.id, bot = %bot.def.slug, guild = %guild_id, module,
        ok = answer.ok, "test requested"
    );
    let outcome = if answer.ok { "sent" } else { "failed" };
    let action = format!(
        "{} test {outcome} on {}",
        module_label(&module),
        context.guild.name
    );
    audit::record(&state, &session, &bot.def.slug, action).await;
    state.notices.put(session.key(), answer);
    let to = format!("/bots/{}/servers/{guild_id}/{module}", bot.def.slug);
    Ok(Redirect::to(&to).into_response())
}

/// Marks an exported file, so a wrong file is recognized on import.
const EXPORT_FORMAT: &str = "dc.enderr.win/server-settings";
/// Largest settings file accepted.
const MAX_IMPORT: usize = 512 * 1024;

/// GET .../export — every setting of the server as a JSON file.
pub async fn export(
    State(state): State<AppState>,
    session: Session,
    Path((slug, guild_id)): Path<(String, String)>,
) -> Page {
    let (bot, context, config) = page_data(&state, &session, &slug, &guild_id, BACKUP).await?;
    let file = json!({
        "format": EXPORT_FORMAT,
        "version": 1,
        "bot": bot.def.slug,
        "server": { "id": guild_id, "name": context.guild.name },
        "exported_at": dc_core::now(),
        "settings": config,
    });
    let body = serde_json::to_string_pretty(&file).map_err(std::io::Error::other)?;
    let name = format!("{}-{guild_id}-settings.json", bot.def.slug);
    tracing::info!(user = %session.user.id, bot = %bot.def.slug, guild = %guild_id, "settings exported");
    let action = format!("Settings of {} exported", context.guild.name);
    audit::record(&state, &session, &bot.def.slug, action).await;
    Ok((
        [
            (
                header::CONTENT_TYPE,
                "application/json; charset=utf-8".to_owned(),
            ),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{name}\""),
            ),
            (header::CACHE_CONTROL, "no-store".to_owned()),
        ],
        body + "\n",
    )
        .into_response())
}

/// The settings inside an imported file: an exported file, or the bare
/// settings object. Only the sections present replace the current ones.
fn merge_import(current: &GuildConfig, raw: &str) -> Result<(GuildConfig, Vec<String>), String> {
    let parsed: Value = serde_json::from_str(raw.trim_start_matches('\u{feff}'))
        .map_err(|error| format!("This is not a valid JSON file ({error})."))?;
    let settings = match parsed.get("format") {
        Some(format) if format != EXPORT_FORMAT => {
            return Err("This file is not a server settings file of this panel.".into());
        }
        Some(_) => parsed.get("settings").cloned().unwrap_or(Value::Null),
        None => parsed,
    };
    let Value::Object(sections) = settings else {
        return Err("The file has no settings in it.".into());
    };
    let mut merged = serde_json::to_value(current).map_err(|error| error.to_string())?;
    let mut imported = Vec::new();
    for (key, value) in sections {
        // Unknown keys are ignored rather than refused: newer files still load.
        if merged.get(&key).is_some() && value.is_object() {
            merged[&key] = value;
            imported.push(key);
        }
    }
    if imported.is_empty() {
        return Err("The file has no known section (welcome, rules, tickets…).".into());
    }
    let mut config: GuildConfig = serde_json::from_value(merged)
        .map_err(|error| format!("The file has a value of the wrong type ({error})."))?;
    config.sanitize();
    imported.sort();
    Ok((config, imported))
}

/// Channel and role ids of the settings that do not exist on the server
/// (member ids are not checked).
fn unknown_ids(config: &GuildConfig, context: &GuildContext) -> usize {
    fn walk(value: &Value, out: &mut Vec<String>) {
        match value {
            Value::String(text) if dc_core::is_snowflake(text) => out.push(text.clone()),
            Value::Array(items) => items.iter().for_each(|item| walk(item, out)),
            Value::Object(map) => map.values().for_each(|item| walk(item, out)),
            _ => {}
        }
    }
    let mut value = serde_json::to_value(config).unwrap_or_default();
    value["admins"]["users"] = Value::Null;
    let mut ids = Vec::new();
    walk(&value, &mut ids);
    ids.sort();
    ids.dedup();
    ids.iter()
        .filter(|id| {
            **id != context.guild.id
                && !context.channels.iter().any(|c| &c.id == *id)
                && !context.roles.iter().any(|r| &r.id == *id)
        })
        .count()
}

/// POST .../import — a settings file, uploaded or pasted.
pub async fn import(
    State(state): State<AppState>,
    session: Session,
    Path((slug, guild_id)): Path<(String, String)>,
    mut form: Multipart,
) -> Page {
    let unreadable =
        || AppError::BadRequest("The file could not be read. It must be 512 KB at most.");
    let (mut csrf, mut file, mut pasted) = (String::new(), String::new(), String::new());
    while let Some(field) = form.next_field().await.map_err(|_| unreadable())? {
        let name = field.name().unwrap_or("").to_owned();
        if !matches!(name.as_str(), "csrf" | "file" | "json") {
            continue;
        }
        let bytes = field.bytes().await.map_err(|_| unreadable())?;
        if bytes.len() > MAX_IMPORT {
            return Err(unreadable());
        }
        let text = String::from_utf8_lossy(&bytes).into_owned();
        match name.as_str() {
            "csrf" => csrf = text,
            "file" => file = text,
            _ => pasted = text,
        }
    }
    session.check_csrf(&csrf)?;
    let (bot, context, current) = page_data(&state, &session, &slug, &guild_id, BACKUP).await?;
    let raw = if file.trim().is_empty() { pasted } else { file };
    let answer = if raw.trim().is_empty() {
        Answer {
            ok: false,
            message: "Choose a file or paste the settings first.".into(),
        }
    } else {
        match merge_import(&current, &raw) {
            Ok((mut config, sections)) => {
                config.save(&bot.def.dir, &guild_id)?;
                tracing::warn!(
                    user = %session.user.id, bot = %bot.def.slug, guild = %guild_id,
                    sections = %sections.join(","), "settings imported"
                );
                let action = format!(
                    "Settings imported on {} ({})",
                    context.guild.name,
                    sections.join(", ")
                );
                audit::record(&state, &session, &bot.def.slug, action).await;
                let unknown = unknown_ids(&config, &context);
                let mut message = format!(
                    "Settings imported ({} section(s): {}).",
                    sections.len(),
                    sections.join(", ")
                );
                if unknown > 0 {
                    message.push_str(&format!(
                        " {unknown} channel or role id(s) do not exist on this server: check the sections."
                    ));
                }
                Answer { ok: true, message }
            }
            Err(message) => Answer { ok: false, message },
        }
    };
    state.notices.put(session.key(), answer);
    let to = format!("/bots/{}/servers/{guild_id}/{BACKUP}", bot.def.slug);
    Ok(Redirect::to(&to).into_response())
}

#[derive(Deserialize)]
pub struct ResetForm {
    csrf: String,
    confirm: Option<String>,
    xp: Option<String>,
}

/// POST .../reset — the danger zone: every section back to its defaults,
/// and optionally the XP of every member erased.
pub async fn reset(
    State(state): State<AppState>,
    session: Session,
    Path((slug, guild_id)): Path<(String, String)>,
    Form(form): Form<ResetForm>,
) -> Page {
    session.check_csrf(&form.csrf)?;
    if form.confirm.is_none() {
        return Err(AppError::BadRequest("Tick the confirmation box to reset."));
    }
    let (bot, context, _) = page_data(&state, &session, &slug, &guild_id, DANGER).await?;
    // Saved as defaults rather than deleted, so the bot notices the change
    // and takes its panels down.
    GuildConfig::default().save(&bot.def.dir, &guild_id)?;
    tracing::warn!(
        user = %session.user.id, bot = %bot.def.slug, guild = %guild_id,
        xp = form.xp.is_some(), "server settings reset"
    );
    let what = if form.xp.is_some() {
        "settings and XP"
    } else {
        "settings"
    };
    let action = format!("Every {what} of {} reset", context.guild.name);
    audit::record(&state, &session, &bot.def.slug, action).await;
    let mut answer = Answer {
        ok: true,
        message: "All settings of this server were reset.".into(),
    };
    if form.xp.is_some() {
        let request = Request::ResetXp {
            guild_id: guild_id.clone(),
        };
        let erased = bridge::ask_bot(&bot.def.dir, &request).await;
        answer = if erased.ok {
            Answer {
                ok: true,
                message: format!("{} {}", answer.message, erased.message),
            }
        } else {
            Answer {
                ok: false,
                message: format!(
                    "The settings were reset, but the XP was not erased: {}",
                    erased.message
                ),
            }
        };
    }
    state.notices.put(session.key(), answer);
    let to = format!("/bots/{}/servers/{guild_id}/{DANGER}", bot.def.slug);
    Ok(Redirect::to(&to).into_response())
}

/// POST .../leave — the danger zone: the bot leaves the server. Its
/// settings for the server are kept, in case it is invited again.
pub async fn leave(
    State(state): State<AppState>,
    session: Session,
    Path((slug, guild_id)): Path<(String, String)>,
    Form(form): Form<ResetForm>,
) -> Page {
    session.check_csrf(&form.csrf)?;
    if form.confirm.is_none() {
        return Err(AppError::BadRequest(
            "Tick the confirmation box to remove the bot.",
        ));
    }
    let (bot, context, _) = page_data(&state, &session, &slug, &guild_id, DANGER).await?;
    let token = bot.def.token().ok_or(AppError::NotFound)?;
    state.discord.leave_guild(&token, &guild_id).await?;
    tracing::warn!(
        user = %session.user.id, bot = %bot.def.slug, guild = %guild_id,
        "bot removed from a server"
    );
    let action = format!("Bot removed from {}", context.guild.name);
    audit::record(&state, &session, &bot.def.slug, action).await;
    let message = format!("{} has left {}.", bot.def.name, context.guild.name);
    state
        .notices
        .put(session.key(), Answer { ok: true, message });
    Ok(Redirect::to(&format!("/bots/{}/servers", bot.def.slug)).into_response())
}

/// GET .../preview/{image} — the images of the bot, drawn with the logged-in
/// user's own name and avatar.
pub async fn preview(
    State(state): State<AppState>,
    session: Session,
    Path((slug, guild_id, image)): Path<(String, String, String)>,
) -> Page {
    if !dc_core::is_snowflake(&guild_id) {
        return Err(AppError::NotFound);
    }
    let def = owned_bot(&state, &session, &slug).await?;
    let user = &session.user;
    let name = user.display_name().to_owned();
    let avatar = || state.discord.avatar_png(&user.id, user.avatar.as_deref());

    // The server, from the cache shared with the settings pages.
    let context = || async {
        let token = def.token().ok_or(AppError::NotFound)?;
        state
            .discord
            .guild_context(&token, &guild_id)
            .await
            .map_err(|_| AppError::NotFound)
    };
    let svg = match image.as_str() {
        "close-dms.png" => {
            let png = dc_core::tips::CLOSE_DMS_PNG;
            return Ok(([(header::CONTENT_TYPE, "image/png")], png).into_response());
        }
        // Banners of the welcome DM and of the safety reminder.
        "dm-banner-en.png" | "dm-banner-fr.png" => {
            let server = context().await?.guild.name.clone();
            let title = if image == "dm-banner-fr.png" {
                format!("Bienvenue sur {server}")
            } else {
                format!("Welcome to {server}")
            };
            cards::banner(Tone::Info, &def.name, &title, "")
        }
        "reminder-en.png" | "reminder-fr.png" => {
            let title = dc_core::tips::reminder_title(image == "reminder-fr.png");
            cards::banner(Tone::Warning, &def.name, title, "")
        }
        "captcha.png" => render::captcha(&render::captcha_code()),
        "rank.png" => {
            let picture = avatar().await;
            render::rank(&render::RankCard {
                name: &name,
                rank: 3,
                level: 12,
                xp_in_level: 340,
                xp_for_level: 1420,
                total_xp: 15_730,
                picture: picture.as_deref(),
            })
        }
        "welcome.png" => {
            let context = context().await?;
            let guild = &context.guild;
            let picture = avatar().await;
            let subtitle = format!(
                "Member #{} · {}",
                dc_core::thousands(guild.approximate_member_count.unwrap_or(1)),
                guild.name
            );
            render::welcome(&name, &subtitle, picture.as_deref())
        }
        _ => return Err(AppError::NotFound),
    };
    let png = tokio::task::spawn_blocking(move || render::png(&svg))
        .await
        .map_err(std::io::Error::other)?
        .map_err(std::io::Error::other)?;
    Ok(([(header::CONTENT_TYPE, "image/png")], png).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROLE: &str = "175928847299117063";
    const CHANNEL: &str = "175928847299117064";

    fn saved(module: &str, form: GuildForm) -> GuildConfig {
        let mut config = GuildConfig::default();
        apply_form(&mut config, module, form);
        config.sanitize();
        config
    }

    #[test]
    fn admins_form() {
        let config = saved(
            "admins",
            GuildForm {
                users: format!("{ROLE}, abc\n{CHANNEL} {ROLE}"),
                roles: vec![ROLE.into(), "<script>".into()],
                ..Default::default()
            },
        );
        assert_eq!(config.admins.users, vec![ROLE, CHANNEL]);
        assert_eq!(config.admins.roles, vec![ROLE]);
    }

    #[test]
    fn import_merges_sections() {
        let mut current = GuildConfig::default();
        current.logs.enabled = true;
        let exported = json!({
            "format": EXPORT_FORMAT,
            "settings": { "twitch": { "enabled": true, "logins": ["JunkoCry"] }, "nonsense": { "x": 1 } }
        })
        .to_string();
        let (config, sections) = merge_import(&current, &exported).unwrap();
        assert_eq!(sections, vec!["twitch"]);
        assert!(config.twitch.enabled && config.logs.enabled);
        assert_eq!(config.twitch.logins, vec!["junkocry"]);

        let bare = r#"{"anti_scam":{"invites":true}}"#;
        assert!(merge_import(&current, bare).unwrap().0.anti_scam.invites);
        assert!(merge_import(&current, "nope").is_err());
        assert!(merge_import(&current, r#"{"format":"other","settings":{}}"#).is_err());
        assert!(merge_import(&current, r#"{"twitch":{"enabled":"yes"}}"#).is_err());
        assert!(merge_import(&current, r#"{"x":{}}"#).is_err());

        let shipped = include_str!("../../../bots/scanline/import/scanline-settings.json");
        let (config, sections) = merge_import(&GuildConfig::default(), shipped).unwrap();
        assert!(sections.len() >= 8);
        assert!(config.anti_scam.insider && config.role_panel.buttons.len() == 5);
        assert!(config.rules.verifying() && config.rules.verify_channel.is_none());
    }

    #[test]
    fn rules_form() {
        let other = "175928847299117065";
        let config = saved(
            "verification",
            GuildForm {
                enabled: Some("on".into()),
                verification: "activity".into(),
                role: ROLE.into(),
                remove_role: other.into(),
                captcha_style: "slide".into(),
                ..Default::default()
            },
        );
        assert_eq!(config.rules.verification, "activity");
        assert_eq!(config.rules.verified_role.as_deref(), Some(ROLE));
        assert_eq!(config.rules.remove_role.as_deref(), Some(other));
        assert_eq!(config.rules.captcha_style, "slide");
    }

    #[test]
    fn scanline_sections() {
        let config = saved(
            "roles",
            GuildForm {
                enabled: Some("on".into()),
                channel: CHANNEL.into(),
                title: "Roles".into(),
                button_role: vec![ROLE.into(), "".into()],
                button_label: vec!["FR".into(), "skipped".into()],
                button_emoji: vec!["🇫🇷".into(), "".into()],
                button_description: vec!["".into(), "".into()],
                ..Default::default()
            },
        );
        assert_eq!(config.role_panel.buttons.len(), 1);
        assert_eq!(config.role_panel.buttons[0].label, "FR");

        let config = saved(
            "links",
            GuildForm {
                enabled: Some("on".into()),
                scope: "listed".into(),
                listed_channels: vec![CHANNEL.into()],
                x_channels: vec![CHANNEL.into()],
                review_channel: CHANNEL.into(),
                timeout: "60".into(),
                ..Default::default()
            },
        );
        assert_eq!(config.lock_url.scope, "listed");
        assert_eq!(config.lock_url.timeout_secs, 60);
        assert_eq!(config.lock_url.review_channel.as_deref(), Some(CHANNEL));

        let config = saved(
            "welcome",
            GuildForm {
                dm_enabled: Some("on".into()),
                dm_links:
                    "Axiom | https://axiom.trade/@x\nbad line\nFomo | https://fomo.family/r/x"
                        .into(),
                ..Default::default()
            },
        );
        assert!(config.welcome.dm_enabled && !config.welcome.dm_safety);
        assert_eq!(config.welcome.dm_links.len(), 2);

        let config = saved(
            "twitch",
            GuildForm {
                enabled: Some("on".into()),
                logins: "dh_trenches\nJunkoCry\n".into(),
                ..Default::default()
            },
        );
        assert_eq!(config.twitch.logins, vec!["dh_trenches", "junkocry"]);
    }

    #[test]
    fn join_mentions() {
        let config = saved(
            "welcome",
            GuildForm {
                channels: vec![CHANNEL.into(), ROLE.into(), "x".into()],
                ping_seconds: "999".into(),
                ..Default::default()
            },
        );
        assert_eq!(config.welcome.ping_channels, vec![ROLE, CHANNEL]);
        assert_eq!(config.welcome.ping_seconds, 60);
        // Works even with the welcome message off.
        assert!(!config.welcome.enabled);
    }

    #[test]
    fn unticked_checkboxes_turn_things_off() {
        let mut config = GuildConfig::default();
        config.welcome.enabled = true;
        apply_form(&mut config, "welcome", GuildForm::default());
        config.sanitize();
        assert!(!config.welcome.enabled && !config.welcome.image);
        // An emptied text falls back to the default instead of staying blank.
        assert!(!config.welcome.message.is_empty());
        assert_eq!(config.welcome.channel, None);
    }

    #[test]
    fn a_section_only_touches_its_own_settings() {
        let form = GuildForm {
            enabled: Some("on".into()),
            channel: CHANNEL.into(),
            ..Default::default()
        };
        let config = saved("logs", form);
        assert!(config.logs.enabled && !config.logs.message_delete);
        assert_eq!(config.logs.channel.as_deref(), Some(CHANNEL));
        assert_eq!(config.welcome, GuildConfig::default().welcome);
        assert_eq!(config.xp, GuildConfig::default().xp);
    }

    #[test]
    fn levels_form() {
        let config = saved(
            "levels",
            GuildForm {
                enabled: Some("on".into()),
                min_xp: "10".into(),
                max_xp: "abc".into(),
                cooldown: "999999".into(),
                announce: "nowhere".into(),
                reward_level: vec!["5".into(), "".into(), "10".into()],
                reward_role: vec![ROLE.into(), ROLE.into(), "".into()],
                ..Default::default()
            },
        );
        assert_eq!(
            (config.xp.min_xp, config.xp.max_xp, config.xp.cooldown_secs),
            (10, 25, 3600)
        );
        assert_eq!(config.xp.announce, "same");
        let expected = Reward {
            level: 5,
            role: ROLE.into(),
        };
        assert_eq!(config.xp.rewards, vec![expected]);
    }

    #[test]
    fn links_and_tickets_lists() {
        let form = GuildForm {
            domains: "https://YouTube.com/x\n\n  tenor.com \nbad".into(),
            ..Default::default()
        };
        let config = saved("links", form);
        assert_eq!(
            config.lock_url.allowed_domains,
            vec!["tenor.com", "youtube.com"]
        );

        let form = GuildForm {
            topics: "Billing\n\nReport a member\n".into(),
            max_open: "50".into(),
            ..Default::default()
        };
        let config = saved("tickets", form);
        assert_eq!(config.tickets.topics, vec!["Billing", "Report a member"]);
        assert_eq!(config.tickets.max_open, 5);
    }
}
