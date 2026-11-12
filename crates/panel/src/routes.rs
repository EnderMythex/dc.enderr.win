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
    render,
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
    let activity = crate::audit::recent(&state, &bot.def.slug)
        .await
        .map_err(|error| tracing::warn!(%error, "panel activity unavailable"))
        .unwrap_or_default();
    Ok(views::bot_logs(&session, &bot, logs, &activity).into_response())
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
    let previous = BotSettings::load(&def.dir).unwrap_or_default();
    // Saved by its own form (Card background).
    let settings = BotSettings {
        background_dim: previous.background_dim,
        ..settings
    };
    settings.save(&def.dir)?;
    tracing::info!(user = %session.user.id, bot = %def.slug, "settings saved");
    if previous != settings {
        let mut changed = Vec::new();
        if previous.presence_status != settings.presence_status {
            changed.push(format!("status → {}", settings.presence_status));
        }
        if previous.activity_type != settings.activity_type
            || previous.activity_text != settings.activity_text
        {
            changed.push(format!(
                "activity → {} “{}”",
                settings.activity_type, settings.activity_text
            ));
        }
        let action = format!("Presence changed: {}", changed.join(", "));
        crate::audit::record(&state, &session, &def.slug, action).await;
    }

    Ok(Redirect::to(&format!("/bots/{}/settings?m=saved", def.slug)).into_response())
}

/// Largest picture accepted for a bot's avatar.
const MAX_AVATAR_BYTES: usize = 2 * 1024 * 1024;
/// Largest picture accepted for a bot's banner.
const MAX_BANNER_BYTES: usize = 4 * 1024 * 1024;
/// Longest bio Discord accepts for an application.
pub const MAX_BIO_CHARS: usize = 400;

/// `data:` URI of an uploaded picture, or why it is refused.
fn image_data(bytes: &[u8], max: usize, what: &str) -> Result<String, String> {
    if bytes.len() > max {
        return Err(format!(
            "The {what} is too large ({} MB at most).",
            max / 1024 / 1024
        ));
    }
    let mime = render::image_type(bytes)
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

    let mut done = Vec::new();
    if problem.is_none() && !changes.is_empty() {
        let fields: Vec<String> = changes.keys().cloned().collect();
        let previous_name = current.as_ref().map(|user| user.username.clone());
        match state.discord.update_bot_profile(&token, changes).await {
            Ok(_) => {
                tracing::info!(
                    user = %session.user.id, bot = %def.slug, fields = %fields.join(","),
                    "bot profile updated"
                );
                for field in &fields {
                    done.push(match field.as_str() {
                        "username" => format!(
                            "name changed{} → {username}",
                            previous_name
                                .as_deref()
                                .map(|name| format!(" from {name}"))
                                .unwrap_or_default()
                        ),
                        "avatar" => "profile picture changed".to_owned(),
                        _ if form.banner.is_empty() => "banner removed".to_owned(),
                        _ => "banner changed".to_owned(),
                    });
                }
            }
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
            Ok(()) => {
                tracing::info!(user = %session.user.id, bot = %def.slug, "bot bio updated");
                done.push("bio changed".to_owned());
            }
            Err(error) => problem = Some(profile_problem(error)?),
        }
    }

    if !done.is_empty() {
        let action = format!("Profile: {}", done.join(", "));
        crate::audit::record(&state, &session, &def.slug, action).await;
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

/// GET /bots/{slug}/commands — every slash command of the bot.
pub async fn bot_commands(
    State(state): State<AppState>,
    session: Session,
    Path(slug): Path<String>,
) -> Page {
    let def = owned_bot(&state, &session, &slug).await?;
    let commands = dc_core::commands::listing(&def.packs);
    let bot = BotView::load(&state, def).await;
    Ok(views::bot_commands(&session, &bot, &commands).into_response())
}

/// The bot's card background with its darkening, if it has one.
pub fn background(def: &dc_core::registry::BotDef) -> Option<render::Background> {
    let dim = BotSettings::load(&def.dir)
        .map(|settings| settings.background_dim)
        .unwrap_or(dc_core::settings::BACKGROUND_DIM);
    render::Background::load(&def.dir, dim)
}

#[derive(Default)]
struct BackgroundForm {
    csrf: String,
    picture: Vec<u8>,
    remove: bool,
    dim: Option<u8>,
}

/// POST /bots/{slug}/background — the picture behind the bot's cards.
pub async fn save_background(
    State(state): State<AppState>,
    session: Session,
    Path(slug): Path<String>,
    mut multipart: Multipart,
) -> Page {
    let mut form = BackgroundForm::default();
    let unreadable =
        || AppError::BadRequest("The form could not be read. The picture must be 3 MB at most.");
    while let Some(field) = multipart.next_field().await.map_err(|_| unreadable())? {
        let name = field.name().unwrap_or("").to_owned();
        match name.as_str() {
            "picture" => form.picture = field.bytes().await.map_err(|_| unreadable())?.to_vec(),
            "csrf" | "remove" | "dim" => {
                let text = field.text().await.map_err(|_| unreadable())?;
                match name.as_str() {
                    "csrf" => form.csrf = text,
                    "remove" => form.remove = true,
                    _ => form.dim = text.trim().parse::<u8>().ok().map(|dim| dim.min(90)),
                }
            }
            _ => {}
        }
    }
    session.check_csrf(&form.csrf)?;
    let def = owned_bot(&state, &session, &slug).await?;
    let file = def.dir.join(render::BACKGROUND_FILE);
    let mut done = Vec::new();

    if !form.picture.is_empty() {
        let problem = if form.picture.len() > render::BACKGROUND_MAX_BYTES {
            Some("The picture is too large (3 MB at most).")
        } else if render::image_type(&form.picture).is_none() {
            Some("The picture must be a PNG, JPEG, GIF or WebP image.")
        } else {
            None
        };
        if let Some(problem) = problem {
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
                None,
                Some(problem),
            );
            return Ok((StatusCode::UNPROCESSABLE_ENTITY, page).into_response());
        }
        let tmp = def.dir.join(format!(".{}.tmp", render::BACKGROUND_FILE));
        std::fs::write(&tmp, &form.picture)?;
        std::fs::rename(&tmp, &file)?;
        done.push("picture changed".to_owned());
    } else if form.remove && file.exists() {
        std::fs::remove_file(&file)?;
        done.push("picture removed".to_owned());
    }
    if let Some(dim) = form.dim {
        let mut settings = BotSettings::load(&def.dir)?;
        if settings.background_dim != dim {
            settings.background_dim = dim;
            settings.save(&def.dir)?;
            done.push(format!("darkening → {dim} %"));
        }
    }
    if !done.is_empty() {
        tracing::info!(user = %session.user.id, bot = %def.slug, "card background updated");
        let action = format!("Card background: {}", done.join(", "));
        crate::audit::record(&state, &session, &def.slug, action).await;
    }
    Ok(Redirect::to(&format!("/bots/{}/settings?m=background", def.slug)).into_response())
}

/// GET /bots/{slug}/background/{image} — samples of the cards with the
/// bot's background: `notice.png` (an error message) and `welcome.png`.
pub async fn background_preview(
    State(state): State<AppState>,
    session: Session,
    Path((slug, image)): Path<(String, String)>,
) -> Page {
    let def = owned_bot(&state, &session, &slug).await?;
    let svg = match image.as_str() {
        "notice.png" => dc_core::cards::banner(
            dc_core::cards::Tone::Error,
            &def.name,
            "Something went wrong",
            "Error messages, confirmations and notices look like this.",
        ),
        "welcome.png" => {
            let user = &session.user;
            let picture = state
                .discord
                .avatar_png(&user.id, user.avatar.as_deref())
                .await;
            render::welcome(
                user.display_name(),
                "Member #128 · Your server",
                picture.as_deref(),
            )
        }
        _ => return Err(AppError::NotFound),
    };
    let background = background(&def);
    let png = tokio::task::spawn_blocking(move || render::png_with(&svg, background.as_ref()))
        .await
        .map_err(std::io::Error::other)?
        .map_err(std::io::Error::other)?;
    let headers = [
        (header::CONTENT_TYPE, "image/png"),
        (header::CACHE_CONTROL, "no-store"),
    ];
    Ok((headers, png).into_response())
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
    crate::audit::record(&state, &session, &def.slug, format!("Bot {flash}")).await;
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
        assert_eq!(render::image_type(&webp), Some("image/webp"));
    }

    #[test]
    fn bios() {
        assert_eq!(clean_bio("  Hello\r\nWorld  "), "Hello\nWorld");
    }
}
