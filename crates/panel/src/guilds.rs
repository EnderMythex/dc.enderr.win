//! Per-server configuration of a bot: admins, welcome, rules, tickets,
//! levels, logs and link lock. Saved to `bots/<slug>/guilds/<id>.json`, which
//! the bot picks up within a few seconds.

use axum::{
    extract::{Path, Query, State},
    http::header,
    response::{IntoResponse, Redirect},
};
use axum_extra::extract::Form;
use dc_core::{
    guild::{GuildConfig, Reward, Xp},
    render,
    requests::{Answer, Request},
};
use serde::Deserialize;

use crate::{
    AppState,
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
    ("tickets", "Tickets"),
    ("levels", "Levels"),
    ("logs", "Logs"),
    ("links", "Link lock"),
];

/// Extra page next to the sections: reset of the whole server.
pub const DANGER: &str = "danger";

/// Sections whose output can be sent to Discord as a test.
const TESTABLE: &[&str] = &["welcome", "rules", "tickets", "levels", "logs"];

/// Checks access and loads everything a server page needs. 404 when the bot
/// is not on that server, so ids cannot be probed.
async fn page_data(
    state: &AppState,
    session: &Session,
    slug: &str,
    guild_id: &str,
    module: &str,
) -> Result<(BotView, GuildContext, GuildConfig), AppError> {
    let known = module == DANGER || MODULES.iter().any(|(id, _)| *id == module);
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
        }
        "rules" => {
            config.rules.enabled = enabled;
            config.rules.channel = optional(form.channel);
            config.rules.title = form.title;
            config.rules.text = form.text;
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
    if module == DANGER {
        return Err(AppError::NotFound);
    }
    let (bot, _, mut config) = page_data(&state, &session, &slug, &guild_id, &module).await?;
    apply_form(&mut config, &module, form);
    config.save(&bot.def.dir, &guild_id)?;
    tracing::info!(
        user = %session.user.id, bot = %bot.def.slug, guild = %guild_id, module,
        "server settings saved"
    );
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
    let (bot, ..) = page_data(&state, &session, &slug, &guild_id, &module).await?;
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
    state.notices.put(session.key(), answer);
    let to = format!("/bots/{}/servers/{guild_id}/{module}", bot.def.slug);
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
    let (bot, ..) = page_data(&state, &session, &slug, &guild_id, DANGER).await?;
    // Saved as defaults rather than deleted, so the bot notices the change
    // and takes its panels down.
    GuildConfig::default().save(&bot.def.dir, &guild_id)?;
    tracing::warn!(
        user = %session.user.id, bot = %bot.def.slug, guild = %guild_id,
        xp = form.xp.is_some(), "server settings reset"
    );
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

    let svg = match image.as_str() {
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
            let token = def.token().ok_or(AppError::NotFound)?;
            let guild = state
                .discord
                .guild(&token, &guild_id)
                .await
                .map_err(|_| AppError::NotFound)?;
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
    fn rules_form() {
        let other = "175928847299117065";
        let config = saved(
            "rules",
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
