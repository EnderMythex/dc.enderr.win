use axum::{
    Form,
    extract::{Multipart, Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Redirect, Response},
};
use axum_extra::extract::cookie::CookieJar;
use base64::Engine;
use dc_core::{
    registry::{self, BotDef},
    settings::BotSettings,
};
use futures_util::future::join_all;
use serde::Deserialize;

use crate::{
    AppState,
    auth::{self, Session},
    discord::{DiscordError, DiscordUser},
    docker::{Action, Status},
    error::AppError,
    views,
};

pub type Page = Result<Response, AppError>;

/// A bot as displayed: its definition, its Discord account and its container.
pub struct BotView {
    pub def: BotDef,
    pub identity: Option<DiscordUser>,
    pub status: Status,
}

impl BotView {
    pub async fn load(state: &AppState, def: BotDef) -> Self {
        let token = def.token();
        let (identity, status) = tokio::join!(
            state.discord.bot_identity(token.as_deref()),
            state.docker.status(&def),
        );
        Self {
            def,
            identity,
            status,
        }
    }
}

/// Confirmation message passed through a redirect (`?m=code`).
#[derive(Deserialize)]
pub struct Flash {
    m: Option<String>,
}

impl Flash {
    pub fn text(&self) -> Option<&'static str> {
        views::flash_text(self.m.as_deref()?)
    }
}

// ---------------------------------------------------------------- static

pub async fn stylesheet() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/css; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        include_str!("../assets/app.css"),
    )
}

pub async fn favicon() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "image/svg+xml"),
            (header::CACHE_CONTROL, "public, max-age=86400"),
        ],
        include_str!("../assets/favicon.svg"),
    )
}

pub async fn not_found() -> AppError {
    AppError::NotFound
}

// ---------------------------------------------------------------- home

#[derive(Deserialize)]
pub struct IndexQuery {
    e: Option<String>,
}

pub async fn index(
    State(state): State<AppState>,
    jar: CookieJar,
    Query(query): Query<IndexQuery>,
) -> Page {
    if auth::load_session(&state, &jar).await?.is_some() {
        return Ok(Redirect::to("/dashboard").into_response());
    }
    let error = match query.e.as_deref() {
        Some("denied") => {
            Some("Login cancelled. Allow access to your Discord account to continue.")
        }
        Some("expired") => Some("The login attempt expired. Please try again."),
        _ => None,
    };
    Ok(views::login(error).into_response())
}

pub async fn dashboard(
    State(state): State<AppState>,
    session: Session,
    Query(flash): Query<Flash>,
) -> Page {
    let (defs, errors) = registry::load(&state.cfg.bots_dir);
    for error in &errors {
        tracing::warn!("invalid bot ignored: {error}");
    }
    let visible = defs
        .into_iter()
        .filter(|def| session.is_admin || def.is_owned_by(&session.user.id));
    let bots = join_all(visible.map(|def| BotView::load(&state, def))).await;
    // Configuration errors are only shown to admins.
    let errors = if session.is_admin { errors } else { Vec::new() };
    Ok(views::dashboard(&session, &bots, &errors, flash.text()).into_response())
}

// ---------------------------------------------------------------- bot pages

/// Loads the bot if the logged-in user may manage it; 404 otherwise, so the
/// existence of other users' bots is never revealed.
pub async fn owned_bot(
    state: &AppState,
    session: &Session,
    slug: &str,
) -> Result<BotDef, AppError> {
    let def = registry::find(&state.cfg.bots_dir, slug)
        .map_err(|error| {
            tracing::warn!("invalid bot: {error}");
            AppError::NotFound
        })?
        .ok_or(AppError::NotFound)?;
    if session.is_admin || def.is_owned_by(&session.user.id) {
        Ok(def)
    } else {
        Err(AppError::NotFound)
    }
}

pub async fn bot_overview(
    State(state): State<AppState>,
    session: Session,
    Path(slug): Path<String>,
    Query(flash): Query<Flash>,
) -> Page {
    let def = owned_bot(&state, &session, &slug).await?;
    let settings = BotSettings::load(&def.dir)?;
    let bot = BotView::load(&state, def).await;
    Ok(views::bot_overview(&session, &bot, &settings, flash.text(), None).into_response())
}

pub async fn bot_servers(
    State(state): State<AppState>,
    session: Session,
    Path(slug): Path<String>,
) -> Page {
    let def = owned_bot(&state, &session, &slug).await?;
    let token = def.token();
    let bot = BotView::load(&state, def).await;
    let guilds = match token {
        None => Err("No token is configured for this bot yet."),
        Some(token) => state
            .discord
            .bot_guilds(&token)
            .await
            .map_err(|error| match error {
                DiscordError::Unauthorized => "Discord rejected this bot's token.",
                DiscordError::RateLimited => "Too many requests to Discord. Try again in a moment.",
                _ => {
                    tracing::warn!(bot = %bot.def.slug, %error, "listing servers failed");
                    "Discord cannot be reached right now."
                }
            }),
    };
    Ok(views::bot_servers(&session, &bot, guilds).into_response())
}

pub async fn bot_logs(
    State(state): State<AppState>,
    session: Session,
    Path(slug): Path<String>,
) -> Page {
    let def = owned_bot(&state, &session, &slug).await?;
    let bot = BotView::load(&state, def).await;
    let logs = state.docker.logs(&bot.def).await;
    Ok(views::bot_logs(&session, &bot, logs).into_response())
}

pub async fn bot_settings(
    State(state): State<AppState>,
    session: Session,
    Path(slug): Path<String>,
    Query(flash): Query<Flash>,
) -> Page {
    let def = owned_bot(&state, &session, &slug).await?;
    let settings = BotSettings::load(&def.dir)?;
    let bot = BotView::load(&state, def).await;
    Ok(views::bot_settings(&session, &bot, &settings, flash.text(), None).into_response())
}

#[derive(Deserialize)]
pub struct SettingsForm {
    csrf: String,
    presence_status: String,
    activity_type: String,
    activity_text: String,
}

pub async fn save_settings(
    State(state): State<AppState>,
    session: Session,
    Path(slug): Path<String>,
    Form(form): Form<SettingsForm>,
) -> Page {
    session.check_csrf(&form.csrf)?;
    let def = owned_bot(&state, &session, &slug).await?;
    let parsed = BotSettings::parse(
        &form.presence_status,
        &form.activity_type,
        &form.activity_text,
    );
    let settings = match parsed {
        Ok(settings) => settings,
        Err(error) => {
            let current = BotSettings::load(&def.dir)?;
            let bot = BotView::load(&state, def).await;
            let page =
                views::bot_settings(&session, &bot, &current, None, Some(&error.to_string()));
            return Ok((StatusCode::UNPROCESSABLE_ENTITY, page).into_response());
        }
    };
    settings.save(&def.dir)?;
    tracing::info!(user = %session.user.id, bot = %def.slug, "settings saved");

    Ok(Redirect::to(&format!("/bots/{}/settings?m=saved", def.slug)).into_response())
}

/// Largest picture accepted for a bot's avatar.
const MAX_AVATAR_BYTES: usize = 2 * 1024 * 1024;

/// MIME type of an image, from its first bytes.
fn image_type(bytes: &[u8]) -> Option<&'static str> {
    match bytes {
        [0x89, b'P', b'N', b'G', ..] => Some("image/png"),
        [0xFF, 0xD8, 0xFF, ..] => Some("image/jpeg"),
        [b'G', b'I', b'F', b'8', ..] => Some("image/gif"),
        [
            b'R',
            b'I',
            b'F',
            b'F',
            _,
            _,
            _,
            _,
            b'W',
            b'E',
            b'B',
            b'P',
            ..,
        ] => Some("image/webp"),
        _ => None,
    }
}

/// POST /bots/{slug}/profile — the bot's name and picture on Discord.
pub async fn save_profile(
    State(state): State<AppState>,
    session: Session,
    Path(slug): Path<String>,
    mut form: Multipart,
) -> Page {
    let too_large = AppError::BadRequest("The picture is too large (2 MB at most).");
    let (mut csrf, mut username, mut picture) = (String::new(), String::new(), Vec::new());
    while let Some(field) = form.next_field().await.map_err(|_| too_large_or_bad())? {
        match field.name() {
            Some("csrf") => csrf = field.text().await.map_err(|_| too_large_or_bad())?,
            Some("username") => username = field.text().await.map_err(|_| too_large_or_bad())?,
            Some("avatar") => {
                picture = field
                    .bytes()
                    .await
                    .map_err(|_| too_large_or_bad())?
                    .to_vec();
            }
            _ => {}
        }
    }
    session.check_csrf(&csrf)?;
    let def = owned_bot(&state, &session, &slug).await?;
    let token = def.token().ok_or(AppError::NotFound)?;
    let current = state.discord.bot_identity(Some(&token)).await;

    let username = username.trim();
    let new_name = (!username.is_empty()
        && current
            .as_ref()
            .is_none_or(|user| user.username != username))
    .then_some(username);
    let mut problem = None;
    if new_name.is_some_and(|name| !(2..=32).contains(&name.chars().count())) {
        problem = Some("The name must be 2 to 32 characters long.".to_owned());
    }
    let mut avatar = None;
    if !picture.is_empty() {
        if picture.len() > MAX_AVATAR_BYTES {
            return Err(too_large);
        }
        match image_type(&picture) {
            Some(mime) => {
                let encoded = base64::engine::general_purpose::STANDARD.encode(&picture);
                avatar = Some(format!("data:{mime};base64,{encoded}"));
            }
            None => problem = Some("The picture must be a PNG, JPEG, GIF or WebP image.".into()),
        }
    }

    if problem.is_none() && (new_name.is_some() || avatar.is_some()) {
        let updated = state
            .discord
            .update_bot_profile(&token, new_name, avatar.as_deref())
            .await;
        match updated {
            Ok(_) => tracing::info!(
                user = %session.user.id, bot = %def.slug,
                name = new_name.is_some(), avatar = avatar.is_some(), "bot profile updated"
            ),
            Err(DiscordError::Rejected(reason)) => problem = Some(reason),
            Err(DiscordError::RateLimited) => {
                problem = Some(
                    "Discord limits how often a bot's profile can change. Try again later.".into(),
                );
            }
            Err(error) => return Err(error.into()),
        }
    }

    if let Some(problem) = problem {
        let settings = BotSettings::load(&def.dir)?;
        let bot = BotView::load(&state, def).await;
        let page = views::bot_settings(&session, &bot, &settings, None, Some(&problem));
        return Ok((StatusCode::UNPROCESSABLE_ENTITY, page).into_response());
    }
    Ok(Redirect::to(&format!("/bots/{}/settings?m=profile", def.slug)).into_response())
}

/// A multipart body that could not be read: almost always an oversized file.
fn too_large_or_bad() -> AppError {
    AppError::BadRequest("The form could not be read. The picture must be 2 MB at most.")
}

#[derive(Deserialize)]
pub struct PowerForm {
    csrf: String,
    action: String,
}

pub async fn power(
    State(state): State<AppState>,
    session: Session,
    Path(slug): Path<String>,
    Form(form): Form<PowerForm>,
) -> Page {
    session.check_csrf(&form.csrf)?;
    let def = owned_bot(&state, &session, &slug).await?;
    let (action, flash) = match form.action.as_str() {
        "start" => (Action::Start, "started"),
        "stop" => (Action::Stop, "stopped"),
        "restart" => (Action::Restart, "restarted"),
        _ => return Err(AppError::BadRequest("Unknown action.")),
    };

    if let Err(error) = state.docker.run(&def, action).await {
        let settings = BotSettings::load(&def.dir)?;
        let bot = BotView::load(&state, def).await;
        let page = views::bot_overview(&session, &bot, &settings, None, Some(&error.to_string()));
        return Ok((StatusCode::BAD_GATEWAY, page).into_response());
    }
    tracing::info!(user = %session.user.id, bot = %def.slug, action = form.action, "power action");
    Ok(Redirect::to(&format!("/bots/{}?m={flash}", def.slug)).into_response())
}
