//! Member verification inside a Discord Activity.
//!
//! The bot's "Verify" button opens the bot application's Activity, whose URL
//! mapping points at `/activity/<slug>` here. The page proves who the member
//! is with Discord's OAuth2 (through the Activity's own RPC channel), shows a
//! captcha, and on success asks the bot to hand out the roles.
//!
//! These routes are public: they are loaded by Discord's proxy, without the
//! panel's session cookie. A challenge is bound to the Discord account that
//! authorized it, and only ever results in "this member passed the captcha".

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axum::{
    Json,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use dc_core::{
    crypto::random_token,
    guild::GuildConfig,
    registry::{self, BotDef},
    render,
    requests::Request,
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{AppState, bridge, error::AppError, views};

const LIFETIME: Duration = Duration::from_secs(600);
const MAX_ATTEMPTS: u32 = 5;
const MAX_CHALLENGES: usize = 10_000;

struct Challenge {
    slug: String,
    guild_id: String,
    user_id: String,
    code: String,
    attempts: u32,
    expires: Instant,
}

/// Captchas waiting for an answer, by their random token.
#[derive(Clone, Default)]
pub struct Challenges(Arc<Mutex<HashMap<String, Challenge>>>);

impl Challenges {
    fn create(&self, slug: &str, guild_id: &str, user_id: &str) -> String {
        let mut all = self.0.lock().unwrap();
        all.retain(|_, challenge| challenge.expires > Instant::now());
        // One challenge per member and server, and a hard cap on memory.
        all.retain(|_, c| !(c.slug == slug && c.guild_id == guild_id && c.user_id == user_id));
        if all.len() >= MAX_CHALLENGES {
            all.clear();
        }
        let token = random_token(24);
        all.insert(
            token.clone(),
            Challenge {
                slug: slug.to_owned(),
                guild_id: guild_id.to_owned(),
                user_id: user_id.to_owned(),
                code: render::captcha_code(),
                attempts: 0,
                expires: Instant::now() + LIFETIME,
            },
        );
        token
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

/// The bot, if it exists and can run the Activity: `(definition, bot token,
/// client secret)`.
fn activity_bot(state: &AppState, slug: &str) -> Option<(BotDef, String, String)> {
    let def = registry::find(&state.cfg.bots_dir, slug).ok().flatten()?;
    let token = def.token()?;
    let secret = def.client_secret()?;
    Some((def, token, secret))
}

fn uses_activity(config: &GuildConfig) -> bool {
    let rules = &config.rules;
    rules.enabled && rules.verification == "activity" && rules.verified_role.is_some()
}

fn failure(message: &str) -> Json<Value> {
    Json(json!({ "ok": false, "error": message }))
}

/// "Start over": the page reloads and asks Discord for a new authorization.
fn restart(message: &str) -> Json<Value> {
    Json(json!({ "ok": false, "error": message, "restart": true }))
}

#[derive(Deserialize)]
pub struct PageQuery {
    /// Set by Discord when the page runs inside its client.
    frame_id: Option<String>,
}

/// GET /activity/{slug} — the page shown inside Discord.
pub async fn page(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    Query(query): Query<PageQuery>,
) -> Result<Response, AppError> {
    let (def, token, _) = activity_bot(&state, &slug).ok_or(AppError::NotFound)?;
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
    Ok(views::activity(&identity.id, &identity.username, &base).into_response())
}

pub async fn script() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=300"),
        ],
        include_str!("../assets/activity.js"),
    )
}

#[derive(Deserialize)]
pub struct SessionBody {
    code: String,
    guild_id: String,
}

/// POST /activity/{slug}/session — identifies the member and starts a captcha.
pub async fn session(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    Json(body): Json<SessionBody>,
) -> Result<Json<Value>, AppError> {
    let (def, token, secret) = activity_bot(&state, &slug).ok_or(AppError::NotFound)?;
    let code_ok = (10..=100).contains(&body.code.len())
        && body.code.bytes().all(|b| b.is_ascii_alphanumeric());
    if !dc_core::is_snowflake(&body.guild_id) || !code_ok {
        return Err(AppError::BadRequest("Invalid request."));
    }
    let config = GuildConfig::load(&def.dir, &body.guild_id)?;
    if !uses_activity(&config) {
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
    let challenge = state.challenges.create(&def.slug, &body.guild_id, &user.id);
    let name = user.global_name.unwrap_or(user.username);
    Ok(Json(
        json!({ "ok": true, "token": challenge, "name": name }),
    ))
}

#[derive(Deserialize)]
pub struct TokenQuery {
    t: String,
}

/// GET /activity/{slug}/captcha.png?t=token
pub async fn captcha(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    Query(query): Query<TokenQuery>,
) -> Result<Response, AppError> {
    let code = state
        .challenges
        .with(&slug, &query.t, |challenge| challenge.code.clone())
        .ok_or(AppError::NotFound)?;
    let png = tokio::task::spawn_blocking(move || render::png(&render::captcha(&code)))
        .await
        .ok()
        .and_then(Result::ok)
        .ok_or(AppError::BadRequest("The image could not be created."))?;
    Ok(([(header::CONTENT_TYPE, "image/png")], png).into_response())
}

#[derive(Deserialize)]
pub struct TokenBody {
    token: String,
    #[serde(default)]
    answer: String,
}

/// POST /activity/{slug}/refresh — a new image for the same member.
pub async fn refresh(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    Json(body): Json<TokenBody>,
) -> (StatusCode, Json<Value>) {
    let renewed = state.challenges.with(&slug, &body.token, |challenge| {
        challenge.attempts += 1;
        challenge.code = render::captcha_code();
        challenge.attempts <= MAX_ATTEMPTS
    });
    match renewed {
        Some(true) => (StatusCode::OK, Json(json!({ "ok": true }))),
        Some(false) | None => {
            state.challenges.remove(&body.token);
            (StatusCode::OK, restart("This verification expired."))
        }
    }
}

/// POST /activity/{slug}/answer — checks the code and verifies the member.
pub async fn answer(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    Json(body): Json<TokenBody>,
) -> Result<Json<Value>, AppError> {
    let (def, _, _) = activity_bot(&state, &slug).ok_or(AppError::NotFound)?;
    let typed = body.answer.trim().to_uppercase();

    enum Outcome {
        Passed { guild_id: String, user_id: String },
        Wrong { left: u32 },
    }
    let outcome = state.challenges.with(&slug, &body.token, |challenge| {
        if challenge.code == typed {
            return Outcome::Passed {
                guild_id: challenge.guild_id.clone(),
                user_id: challenge.user_id.clone(),
            };
        }
        // A wrong answer burns the code, so it cannot be guessed by trying.
        challenge.attempts += 1;
        challenge.code = render::captcha_code();
        Outcome::Wrong {
            left: MAX_ATTEMPTS.saturating_sub(challenge.attempts),
        }
    });

    match outcome {
        None => Ok(restart("This verification expired.")),
        Some(Outcome::Wrong { left: 0 }) => {
            state.challenges.remove(&body.token);
            Ok(restart("Too many wrong answers."))
        }
        Some(Outcome::Wrong { left }) => Ok(Json(json!({
            "ok": false, "newImage": true,
            "error": format!("That is not the code in the image. {left} attempt(s) left."),
        }))),
        Some(Outcome::Passed { guild_id, user_id }) => {
            state.challenges.remove(&body.token);
            let request = Request::Verify {
                guild_id: guild_id.clone(),
                user_id: user_id.clone(),
            };
            let reply = bridge::ask_bot(&def.dir, &request).await;
            if reply.ok {
                tracing::info!(bot = %def.slug, guild = %guild_id, user = %user_id, "verified in the Activity");
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
        let first = challenges.create("sang", GUILD, "42");
        let second = challenges.create("sang", GUILD, "42");
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
        let token = challenges.create("sang", GUILD, "42");
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
    fn activity_needs_the_full_setup() {
        let mut config = GuildConfig::default();
        config.rules.enabled = true;
        config.rules.verification = "activity".into();
        assert!(!uses_activity(&config));
        config.rules.verified_role = Some(GUILD.into());
        assert!(uses_activity(&config));
        config.rules.verification = "captcha".into();
        assert!(!uses_activity(&config));
    }
}
