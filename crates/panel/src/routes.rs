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

pub async fn script() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        include_str!("../assets/app.js"),
    )
}

/// The service worker; served from the root so it covers every page.
pub async fn service_worker() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        include_str!("../assets/sw.js"),
    )
}

pub async fn offline() -> impl IntoResponse {
    views::offline()
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
    Ok(views::login(error, state.stats.get()).into_response())
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
    let notice = state.notices.take(session.key());
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
    let flash = notice.as_ref().map(|answer| answer.message.as_str());
    Ok(views::bot_servers(&session, &bot, guilds, flash).into_response())
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
    let bio = match def.token() {
        Some(token) => state.discord.bot_bio(&token).await,
        None => None,
    };
    let bot = BotView::load(&state, def).await;
    let page = views::bot_settings(
        &session,
        &bot,
        &settings,
        bio.as_deref(),
        flash.text(),
        None,
    );
    Ok(page.into_response())
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
            let bio = match def.token() {
                Some(token) => state.discord.bot_bio(&token).await,
                None => None,
            };
            let bot = BotView::load(&state, def).await;
            let message = error.to_string();
            let page = views::bot_settings(
                &session,
                &bot,
                &current,
                bio.as_deref(),
                None,
                Some(&message),
            );
            return Ok((StatusCode::UNPROCESSABLE_ENTITY, page).into_response());
        }
    };
    settings.save(&def.dir)?;
    tracing::info!(user = %session.user.id, bot = %def.slug, "settings saved");

    Ok(Redirect::to(&format!("/bots/{}/settings?m=saved", def.slug)).into_response())
}

/// Largest picture accepted for a bot's avatar.
const MAX_AVATAR_BYTES: usize = 2 * 1024 * 1024;
/// Largest picture accepted for a bot's banner.
const MAX_BANNER_BYTES: usize = 4 * 1024 * 1024;
/// Longest bio Discord accepts for an application.
pub const MAX_BIO_CHARS: usize = 400;

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

/// `data:` URI of an uploaded picture, or why it is refused.
fn image_data(bytes: &[u8], max: usize, what: &str) -> Result<String, String> {
    if bytes.len() > max {
        return Err(format!(
            "The {what} is too large ({} MB at most).",
            max / 1024 / 1024
        ));
    }
    let mime = image_type(bytes)
        .ok_or_else(|| format!("The {what} must be a PNG, JPEG, GIF or WebP image."))?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    Ok(format!("data:{mime};base64,{encoded}"))
}

/// Normalizes a bio: Windows line breaks, spaces around.
fn clean_bio(bio: &str) -> String {
    bio.replace("\r\n", "\n").trim().to_owned()
}

#[derive(Default)]
struct ProfileForm {
    csrf: String,
    username: String,
    bio: Option<String>,
    avatar: Vec<u8>,
    banner: Vec<u8>,
    remove_banner: bool,
}

async fn read_profile_form(mut form: Multipart) -> Result<ProfileForm, AppError> {
    let mut out = ProfileForm::default();
    while let Some(field) = form.next_field().await.map_err(|_| too_large_or_bad())? {
        let name = field.name().unwrap_or("").to_owned();
        match name.as_str() {
            "avatar" | "banner" => {
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|_| too_large_or_bad())?
                    .to_vec();
                if name == "avatar" {
                    out.avatar = bytes;
                } else {
                    out.banner = bytes;
                }
            }
            "csrf" | "username" | "bio" | "remove_banner" => {
                let text = field.text().await.map_err(|_| too_large_or_bad())?;
                match name.as_str() {
                    "csrf" => out.csrf = text,
                    "username" => out.username = text,
                    "bio" => out.bio = Some(text),
                    _ => out.remove_banner = true,
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

/// POST /bots/{slug}/profile — the bot's name, bio, picture and banner.
pub async fn save_profile(
    State(state): State<AppState>,
    session: Session,
    Path(slug): Path<String>,
    form: Multipart,
) -> Page {
    let form = read_profile_form(form).await?;
    session.check_csrf(&form.csrf)?;
    let def = owned_bot(&state, &session, &slug).await?;
    let token = def.token().ok_or(AppError::NotFound)?;
    let current = state.discord.bot_identity(Some(&token)).await;

    let mut changes = serde_json::Map::new();
    let mut problem = None;
    let username = form.username.trim();
    let renamed = !username.is_empty()
        && current
            .as_ref()
            .is_none_or(|user| user.username != username);
    if renamed {
        if (2..=32).contains(&username.chars().count()) {
            changes.insert("username".into(), username.into());
        } else {
            problem = Some("The name must be 2 to 32 characters long.".to_owned());
        }
    }
    if !form.avatar.is_empty() {
        match image_data(&form.avatar, MAX_AVATAR_BYTES, "picture") {
            Ok(data) => {
                changes.insert("avatar".into(), data.into());
            }
            Err(reason) => problem = Some(reason),
        }
    }
    if !form.banner.is_empty() {
        match image_data(&form.banner, MAX_BANNER_BYTES, "banner") {
            Ok(data) => {
                changes.insert("banner".into(), data.into());
            }
            Err(reason) => problem = Some(reason),
        }
    } else if form.remove_banner {
        changes.insert("banner".into(), serde_json::Value::Null);
    }
    let bio = form.bio.as_deref().map(clean_bio);
    if bio
        .as_ref()
        .is_some_and(|bio| bio.chars().count() > MAX_BIO_CHARS)
    {
        problem = Some(format!(
            "The bio must be {MAX_BIO_CHARS} characters at most."
        ));
    }

    if problem.is_none() && !changes.is_empty() {
        let fields: Vec<String> = changes.keys().cloned().collect();
        match state.discord.update_bot_profile(&token, changes).await {
            Ok(_) => tracing::info!(
                user = %session.user.id, bot = %def.slug, fields = %fields.join(","),
                "bot profile updated"
            ),
            Err(error) => problem = Some(profile_problem(error)?),
        }
    }
    if problem.is_none()
        && let Some(bio) = bio
        && state
            .discord
            .bot_bio(&token)
            .await
            .is_some_and(|current| current != bio)
    {
        match state.discord.update_bot_bio(&token, &bio).await {
            Ok(()) => tracing::info!(user = %session.user.id, bot = %def.slug, "bot bio updated"),
            Err(error) => problem = Some(profile_problem(error)?),
        }
    }

    if let Some(problem) = problem {
        let settings = BotSettings::load(&def.dir)?;
        let bio = state.discord.bot_bio(&token).await;
        let bot = BotView::load(&state, def).await;
        let page = views::bot_settings(
            &session,
            &bot,
            &settings,
            bio.as_deref(),
            None,
            Some(&problem),
        );
        return Ok((StatusCode::UNPROCESSABLE_ENTITY, page).into_response());
    }
    Ok(Redirect::to(&format!("/bots/{}/settings?m=profile", def.slug)).into_response())
}

/// What to tell the user when Discord refuses a profile change.
fn profile_problem(error: DiscordError) -> Result<String, AppError> {
    match error {
        DiscordError::Rejected(reason) => Ok(reason),
        DiscordError::RateLimited => {
            Ok("Discord limits how often a bot's profile can change. Try again later.".into())
        }
        error => Err(error.into()),
    }
}

/// A multipart body that could not be read: almost always an oversized file.
fn too_large_or_bad() -> AppError {
    AppError::BadRequest(
        "The form could not be read. The picture must be 2 MB at most, the banner 4 MB.",
    )
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uploaded_images() {
        let png = [0x89, b'P', b'N', b'G', 0, 0];
        assert!(
            image_data(&png, 10, "picture")
                .unwrap()
                .starts_with("data:image/png;base64,")
        );
        assert!(
            image_data(b"hello", 10, "banner")
                .unwrap_err()
                .contains("banner")
        );
        assert!(
            image_data(&png, 3, "banner")
                .unwrap_err()
                .contains("too large")
        );
        let webp = *b"RIFF\0\0\0\0WEBPVP8 ";
        assert_eq!(image_type(&webp), Some("image/webp"));
    }

    #[test]
    fn bios() {
        assert_eq!(clean_bio("  Hello\r\nWorld  "), "Hello\nWorld");
    }
}
