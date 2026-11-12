//! Member verification on a page of the panel, reached two ways:
//!
//! - **Activity** (`activity` mode): the bot's Verify button opens the bot
//!   application's Activity. Every bot uses the same URL mapping, `/` to
//!   `/activity`: the page finds the application id in its own address
//!   (`<application id>.discordsays.com`) and then talks to
//!   `/activity/<application id>/...`. The older per-bot mapping, to
//!   `/activity/<slug>`, keeps working. The page proves who the member is
//!   with Discord's OAuth2, through the Activity's own RPC channel.
//! - **Link** (`web` mode): the Verify button gives the member a link to
//!   `/verify/<slug>?v=...`, signed by the bot for that member and server
//!   (see `dc_core::verify_link`).
//!
//! The page then shows a captcha (a code to type, or a puzzle piece to slide
//! into place) and, once solved, asks the bot to hand out the roles.
//!
//! These routes are public: they hold no panel session. A challenge is bound
//! to one member of one server, and only ever results in "this member passed
//! the captcha".

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axum::{
    Json,
    extract::{Path, Query, State},
    http::header,
    response::{IntoResponse, Response},
};
use dc_core::{
    crypto::random_token,
    guild::GuildConfig,
    registry::{self, BotDef},
    render,
    requests::Request,
    verify_link,
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{AppState, bridge, error::AppError, views};

const LIFETIME: Duration = Duration::from_secs(600);
const MAX_ATTEMPTS: u32 = 5;
const MAX_CHALLENGES: usize = 10_000;

enum Puzzle {
    /// Characters to type, drawn on an image.
    Text(String),
    /// A piece to slide to `x`; the images are kept until answered.
    Slide {
        x: u32,
        y: u32,
        background: String,
        piece: String,
    },
}

/// The picture of a bot's slide captcha, if one was chosen.
fn captcha_picture(state: &AppState, slug: &str) -> Option<Vec<u8>> {
    if !dc_core::registry::is_valid_slug(slug) {
        return None;
    }
    let path = state.cfg.bots_dir.join(slug).join(render::CAPTCHA_FILE);
    std::fs::read(path).ok()
}

impl Puzzle {
    fn new(style: &str, picture: Option<&[u8]>) -> Self {
        if style == "slide" {
            let puzzle = render::slide_puzzle_with(picture);
            Self::Slide {
                x: puzzle.x,
                y: puzzle.y,
                background: puzzle.background,
                piece: puzzle.piece,
            }
        } else {
            Self::Text(render::captcha_code())
        }
    }

    fn solved_by(&self, answer: &str) -> bool {
        match self {
            Self::Text(code) => *code == answer.trim().to_uppercase(),
            Self::Slide { x, .. } => answer
                .trim()
                .parse::<u32>()
                .is_ok_and(|answer| render::slide_matches(answer, *x)),
        }
    }

    /// What the page needs to show it.
    fn describe(&self) -> Value {
        match self {
            Self::Text(_) => json!({ "kind": "text" }),
            Self::Slide { y, .. } => json!({
                "kind": "slide",
                "width": render::SLIDE_WIDTH,
                "height": render::SLIDE_HEIGHT,
                "piece": render::PIECE_SIZE,
                "top": y - render::PIECE_RISE,
            }),
        }
    }
}

struct Challenge {
    slug: String,
    guild_id: String,
    user_id: String,
    style: String,
    puzzle: Puzzle,
    attempts: u32,
    expires: Instant,
}

/// Captchas waiting for an answer, by their random token.
#[derive(Clone, Default)]
pub struct Challenges(Arc<Mutex<HashMap<String, Challenge>>>);

impl Challenges {
    fn create(
        &self,
        (slug, guild_id, user_id): (&str, &str, &str),
        style: &str,
        picture: Option<&[u8]>,
    ) -> (String, Value) {
        let mut all = self.0.lock().unwrap();
        all.retain(|_, challenge| challenge.expires > Instant::now());
        // One challenge per member and server, and a hard cap on memory.
        all.retain(|_, c| !(c.slug == slug && c.guild_id == guild_id && c.user_id == user_id));
        if all.len() >= MAX_CHALLENGES {
            all.clear();
        }
        let token = random_token(24);
        let puzzle = Puzzle::new(style, picture);
        let description = puzzle.describe();
        all.insert(
            token.clone(),
            Challenge {
                slug: slug.to_owned(),
                guild_id: guild_id.to_owned(),
                user_id: user_id.to_owned(),
                style: style.to_owned(),
                puzzle,
                attempts: 0,
                expires: Instant::now() + LIFETIME,
            },
        );
        (token, description)
    }

    /// Runs `f` on a live challenge of that bot.
    fn with<T>(&self, slug: &str, token: &str, f: impl FnOnce(&mut Challenge) -> T) -> Option<T> {
        let mut all = self.0.lock().unwrap();
        let challenge = all.get_mut(token)?;
        (challenge.slug == slug && challenge.expires > Instant::now()).then(|| f(challenge))
    }

    fn remove(&self, token: &str) {
        self.0.lock().unwrap().remove(token);
    }
}

/// The bot and its token, by slug or by application id (the shared
/// Activity mapping only knows the latter).
async fn bot(state: &AppState, key: &str) -> Option<(BotDef, String)> {
    if let Some(def) = registry::find(&state.cfg.bots_dir, key).ok().flatten() {
        let token = def.token()?;
        return Some((def, token));
    }
    if !dc_core::is_snowflake(key) {
        return None;
    }
    let (bots, _) = registry::load(&state.cfg.bots_dir);
    for def in bots {
        let Some(token) = def.token() else { continue };
        if def.client_secret().is_none() {
            continue;
        }
        let identity = state.discord.bot_identity(Some(&token)).await;
        if identity.is_some_and(|identity| identity.id == key) {
            return Some((def, token));
        }
    }
    None
}

/// The slug behind a slug or an application id.
async fn slug_of(state: &AppState, key: &str) -> Option<String> {
    bot(state, key).await.map(|(def, _)| def.slug)
}

/// Is this server verifying members with that mode (`activity` or `web`)?
fn uses(config: &GuildConfig, mode: &str) -> bool {
    let rules = &config.rules;
    rules.verifying() && rules.verification == mode && rules.verified_role.is_some()
}

fn failure(message: &str) -> Json<Value> {
    Json(json!({ "ok": false, "error": message }))
}

/// "Start over": the member has to click Verify again.
fn restart(message: &str) -> Json<Value> {
    Json(json!({ "ok": false, "error": message, "restart": true }))
}

fn started(token: String, description: Value, name: &str) -> Json<Value> {
    let mut answer = json!({ "ok": true, "token": token, "name": name });
    if let (Some(answer), Some(extra)) = (answer.as_object_mut(), description.as_object()) {
        answer.extend(extra.clone());
    }
    Json(answer)
}

#[derive(Deserialize)]
pub struct PageQuery {
    /// Set by Discord when the page runs inside its client.
    frame_id: Option<String>,
}

/// GET /activity — the page shown inside Discord, for every bot. The
/// script finds the bot from the application id in the page's address.
pub async fn shared_page(Query(query): Query<PageQuery>) -> Response {
    // Inside Discord every request goes through its proxy, under `/.proxy/`,
    // mapped to `/activity/` here.
    let base = if query.frame_id.is_some() {
        "/.proxy/"
    } else {
        "/activity/"
    };
    views::verification(None, "", base).into_response()
}

/// GET /activity/{slug} — the page shown inside Discord (per-bot mapping).
pub async fn page(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    Query(query): Query<PageQuery>,
) -> Result<Response, AppError> {
    let (def, token) = bot(&state, &slug).await.ok_or(AppError::NotFound)?;
    if def.client_secret().is_none() {
        return Err(AppError::NotFound);
    }
    let identity = state
        .discord
        .bot_identity(Some(&token))
        .await
        .ok_or(AppError::NotFound)?;
    // Inside Discord every request goes through its proxy, under `/.proxy/`
    // (mapped to this bot's folder here). Opened directly, plain relative
    // links work.
    let base = if query.frame_id.is_some() {
        "/.proxy/".to_owned()
    } else {
        format!("/activity/{}/", def.slug)
    };
    Ok(views::verification(Some(&identity.id), &def.name, &base).into_response())
}

/// GET /verify/{slug}?v=... — the page opened from the Verify link.
pub async fn link_page(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> Result<Response, AppError> {
    let (def, _) = bot(&state, &slug).await.ok_or(AppError::NotFound)?;
    let base = format!("/activity/{}/", def.slug);
    Ok(views::verification(None, &def.name, &base).into_response())
}

pub async fn script() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            // Versioned URL (see `views::verification`): safe to keep.
            (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
        ],
        include_str!("../assets/activity.js"),
    )
}

#[derive(Deserialize)]
pub struct SessionBody {
    code: String,
    guild_id: String,
}

/// POST /activity/{slug}/session — Activity: identifies the member with
/// Discord's OAuth2 and starts a captcha.
pub async fn session(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    Json(body): Json<SessionBody>,
) -> Result<Json<Value>, AppError> {
    let (def, token) = bot(&state, &slug).await.ok_or(AppError::NotFound)?;
    let secret = def.client_secret().ok_or(AppError::NotFound)?;
    let code_ok = (10..=100).contains(&body.code.len())
        && body.code.bytes().all(|b| b.is_ascii_alphanumeric());
    if !dc_core::is_snowflake(&body.guild_id) || !code_ok {
        return Err(AppError::BadRequest("Invalid request."));
    }
    let config = GuildConfig::load(&def.dir, &body.guild_id)?;
    if !uses(&config, "activity") {
        return Ok(failure("Verification is not enabled on this server."));
    }
    let identity = state
        .discord
        .bot_identity(Some(&token))
        .await
        .ok_or(AppError::NotFound)?;

    let exchanged = state
        .discord
        .exchange_activity_code(&identity.id, &secret, &body.code)
        .await;
    let access_token = match exchanged {
        Ok(access_token) => access_token,
        Err(error) => {
            tracing::warn!(bot = %def.slug, %error, "Activity code exchange failed");
            return Ok(failure(
                "Discord did not confirm your account. Close this and try again.",
            ));
        }
    };
    let user = state.discord.oauth_user(&access_token).await?;
    let style = &config.rules.captcha_style;
    let picture = captcha_picture(&state, &def.slug);
    let (challenge, description) = state.challenges.create(
        (&def.slug, &body.guild_id, &user.id),
        style,
        picture.as_deref(),
    );
    let name = user.global_name.unwrap_or(user.username);
    let mut answer = started(challenge, description, &name);
    answer.0["bot"] = json!(def.name);
    Ok(answer)
}

#[derive(Deserialize)]
pub struct LinkBody {
    link: String,
}

/// POST /activity/{slug}/link — Verify link: checks its signature and
/// starts a captcha.
pub async fn link_session(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    Json(body): Json<LinkBody>,
) -> Result<Json<Value>, AppError> {
    let (def, token) = bot(&state, &slug).await.ok_or(AppError::NotFound)?;
    let Some(claims) = verify_link::check(&token, &def.slug, &body.link, dc_core::now()) else {
        return Ok(restart(
            "This link has expired or is not valid. Click Verify on the server again.",
        ));
    };
    let config = GuildConfig::load(&def.dir, &claims.guild_id)?;
    if !uses(&config, "web") {
        return Ok(failure("Verification is not enabled on this server."));
    }
    let name = state
        .discord
        .user(&token, &claims.user_id)
        .await
        .map(|user| user.global_name.unwrap_or(user.username))
        .unwrap_or_else(|_| "there".into());
    let picture = captcha_picture(&state, &def.slug);
    let (challenge, description) = state.challenges.create(
        (&def.slug, &claims.guild_id, &claims.user_id),
        &config.rules.captcha_style,
        picture.as_deref(),
    );
    Ok(started(challenge, description, &name))
}

#[derive(Deserialize)]
pub struct TokenQuery {
    t: String,
}

async fn png(svg: String) -> Result<Response, AppError> {
    let png = tokio::task::spawn_blocking(move || render::png(&svg))
        .await
        .ok()
        .and_then(Result::ok)
        .ok_or(AppError::BadRequest("The image could not be created."))?;
    let headers = [
        (header::CONTENT_TYPE, "image/png"),
        (header::CACHE_CONTROL, "no-store"),
    ];
    Ok((headers, png).into_response())
}

/// GET /activity/{slug}/captcha.png?t=token — the code to type.
pub async fn captcha(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    Query(query): Query<TokenQuery>,
) -> Result<Response, AppError> {
    let slug = slug_of(&state, &slug).await.ok_or(AppError::NotFound)?;
    let code = state
        .challenges
        .with(&slug, &query.t, |challenge| match &challenge.puzzle {
            Puzzle::Text(code) => Some(code.clone()),
            Puzzle::Slide { .. } => None,
        })
        .flatten()
        .ok_or(AppError::NotFound)?;
    png(render::captcha(&code)).await
}

/// GET /activity/{slug}/slide.png and piece.png — the slide captcha.
pub async fn slide_image(
    State(state): State<AppState>,
    Path((slug, image)): Path<(String, String)>,
    Query(query): Query<TokenQuery>,
) -> Result<Response, AppError> {
    let slug = slug_of(&state, &slug).await.ok_or(AppError::NotFound)?;
    let svg = state
        .challenges
        .with(&slug, &query.t, |challenge| match &challenge.puzzle {
            Puzzle::Slide {
                background, piece, ..
            } => match image.as_str() {
                "slide.png" => Some(background.clone()),
                "piece.png" => Some(piece.clone()),
                _ => None,
            },
            Puzzle::Text(_) => None,
        })
        .flatten()
        .ok_or(AppError::NotFound)?;
    png(svg).await
}

#[derive(Deserialize)]
pub struct TokenBody {
    token: String,
    #[serde(default)]
    answer: String,
}

/// POST /activity/{slug}/refresh — a new captcha for the same member.
pub async fn refresh(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    Json(body): Json<TokenBody>,
) -> Json<Value> {
    let slug = slug_of(&state, &slug).await.unwrap_or_default();
    let picture = captcha_picture(&state, &slug);
    let renewed = state.challenges.with(&slug, &body.token, |challenge| {
        challenge.attempts += 1;
        challenge.puzzle = Puzzle::new(&challenge.style, picture.as_deref());
        (challenge.attempts <= MAX_ATTEMPTS).then(|| challenge.puzzle.describe())
    });
    match renewed.flatten() {
        Some(description) => started(body.token, description, ""),
        None => {
            state.challenges.remove(&body.token);
            restart("This verification expired. Click Verify on the server again.")
        }
    }
}

/// POST /activity/{slug}/answer — checks the answer and verifies the member.
pub async fn answer(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    Json(body): Json<TokenBody>,
) -> Result<Json<Value>, AppError> {
    let (def, _) = bot(&state, &slug).await.ok_or(AppError::NotFound)?;
    let slug = def.slug.clone();
    let picture = captcha_picture(&state, &slug);

    enum Outcome {
        Passed { guild_id: String, user_id: String },
        Wrong { left: u32, next: Value },
    }
    let outcome = state.challenges.with(&slug, &body.token, |challenge| {
        if challenge.puzzle.solved_by(&body.answer) {
            return Outcome::Passed {
                guild_id: challenge.guild_id.clone(),
                user_id: challenge.user_id.clone(),
            };
        }
        // A wrong answer burns the captcha, so it cannot be guessed by trying.
        challenge.attempts += 1;
        challenge.puzzle = Puzzle::new(&challenge.style, picture.as_deref());
        Outcome::Wrong {
            left: MAX_ATTEMPTS.saturating_sub(challenge.attempts),
            next: challenge.puzzle.describe(),
        }
    });

    match outcome {
        None => Ok(restart(
            "This verification expired. Click Verify on the server again.",
        )),
        Some(Outcome::Wrong { left: 0, .. }) => {
            state.challenges.remove(&body.token);
            Ok(restart(
                "Too many wrong answers. Click Verify on the server again.",
            ))
        }
        Some(Outcome::Wrong { left, next }) => {
            let mut answer = json!({
                "ok": false, "newImage": true,
                "error": format!("Not quite. {left} attempt(s) left."),
            });
            if let (Some(answer), Some(next)) = (answer.as_object_mut(), next.as_object()) {
                answer.extend(next.clone());
            }
            Ok(Json(answer))
        }
        Some(Outcome::Passed { guild_id, user_id }) => {
            state.challenges.remove(&body.token);
            let request = Request::Verify {
                guild_id: guild_id.clone(),
                user_id: user_id.clone(),
            };
            let reply = bridge::ask_bot(&def.dir, &request).await;
            if reply.ok {
                tracing::info!(bot = %def.slug, guild = %guild_id, user = %user_id, "member verified on the web");
                Ok(Json(json!({ "ok": true })))
            } else {
                Ok(restart(&reply.message))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GUILD: &str = "175928847299117063";

    #[test]
    fn challenges_are_scoped_and_replaced() {
        let challenges = Challenges::default();
        let (first, _) = challenges.create(("sang", GUILD, "42"), "text", None);
        let (second, _) = challenges.create(("sang", GUILD, "42"), "text", None);
        assert_ne!(first, second);
        // A new challenge for the same member replaces the previous one.
        assert!(challenges.with("sang", &first, |_| ()).is_none());
        assert!(challenges.with("sang", &second, |_| ()).is_some());
        // A token of one bot is useless on another.
        assert!(challenges.with("other", &second, |_| ()).is_none());

        challenges.remove(&second);
        assert!(challenges.with("sang", &second, |_| ()).is_none());
    }

    #[test]
    fn expired_challenges_are_ignored() {
        let challenges = Challenges::default();
        let (token, _) = challenges.create(("sang", GUILD, "42"), "text", None);
        challenges
            .0
            .lock()
            .unwrap()
            .get_mut(&token)
            .unwrap()
            .expires = Instant::now() - Duration::from_secs(1);
        assert!(challenges.with("sang", &token, |_| ()).is_none());
    }

    #[test]
    fn puzzles() {
        let text = Puzzle::Text("AB12C".into());
        assert!(text.solved_by(" ab12c "));
        assert!(!text.solved_by("AB12D"));
        assert_eq!(text.describe()["kind"], "text");

        let slide = Puzzle::new("slide", None);
        let Puzzle::Slide { x, y, .. } = &slide else {
            panic!("expected a slide puzzle");
        };
        assert!(slide.solved_by(&(x + 3).to_string()));
        assert!(!slide.solved_by(&(x + 40).to_string()));
        assert!(!slide.solved_by("left"));
        let description = slide.describe();
        assert_eq!(description["kind"], "slide");
        assert_eq!(description["top"], y - render::PIECE_RISE);
        // The answer is never sent to the page.
        assert!(description.get("x").is_none());
    }

    #[test]
    fn modes_need_the_full_setup() {
        let mut config = GuildConfig::default();
        config.rules.verify_enabled = Some(true);
        config.rules.verification = "web".into();
        assert!(!uses(&config, "web"));
        config.rules.verified_role = Some(GUILD.into());
        assert!(uses(&config, "web"));
        assert!(!uses(&config, "activity"));
    }
}
