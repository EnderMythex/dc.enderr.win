//! Discord login (OAuth2) and sessions.
//!
//! The session token only exists in clear text in the browser cookie: the
//! database keeps its SHA-256 hash.

use axum::{
    Form,
    extract::{FromRequestParts, Query, State},
    http::request::Parts,
    response::{IntoResponse, Redirect, Response},
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use dc_core::{crypto, now};
use serde::Deserialize;
use subtle::ConstantTimeEq;

use crate::{AppState, discord::Discord, discord::DiscordError, error::AppError};

const SESSION_COOKIE: &str = "dc_session";
const STATE_COOKIE: &str = "dc_oauth_state";
const SESSION_TTL_SECS: i64 = 30 * 24 * 3600;
const STATE_TTL_SECS: i64 = 600;

#[derive(Debug, Clone)]
pub struct User {
    pub id: String,
    pub username: String,
    pub global_name: Option<String>,
    pub avatar: Option<String>,
}

impl User {
    pub fn display_name(&self) -> &str {
        self.global_name.as_deref().unwrap_or(&self.username)
    }
}

/// Logged-in user. As an extractor, redirects to the login page when the
/// session is missing or expired.
pub struct Session {
    pub user: User,
    pub csrf: String,
    pub is_admin: bool,
    id_hash: String,
}

impl Session {
    /// Identifies this session server-side (never sent to the browser).
    pub fn key(&self) -> &str {
        &self.id_hash
    }

    /// Must be called first by every handler that changes something.
    pub fn check_csrf(&self, provided: &str) -> Result<(), AppError> {
        if bool::from(self.csrf.as_bytes().ct_eq(provided.as_bytes())) {
            Ok(())
        } else {
            Err(AppError::Csrf)
        }
    }
}

impl FromRequestParts<AppState> for Session {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Response> {
        let jar = CookieJar::from_headers(&parts.headers);
        match load_session(state, &jar).await {
            Ok(Some(session)) => Ok(session),
            Ok(None) => Err(Redirect::to("/").into_response()),
            Err(error) => Err(error.into_response()),
        }
    }
}

pub async fn load_session(state: &AppState, jar: &CookieJar) -> Result<Option<Session>, AppError> {
    let Some(cookie) = jar.get(SESSION_COOKIE) else {
        return Ok(None);
    };
    let id_hash = crypto::sha256_hex(cookie.value());
    type Row = (String, String, String, Option<String>, Option<String>);
    let row: Option<Row> = sqlx::query_as(
        "SELECT s.csrf_token, u.id, u.username, u.global_name, u.avatar
         FROM sessions s JOIN users u ON u.id = s.user_id
         WHERE s.id_hash = ? AND s.expires_at > ?",
    )
    .bind(&id_hash)
    .bind(now())
    .fetch_optional(&state.db)
    .await?;

    Ok(
        row.map(|(csrf, id, username, global_name, avatar)| Session {
            is_admin: state.cfg.is_admin(&id),
            user: User {
                id,
                username,
                global_name,
                avatar,
            },
            csrf,
            id_hash,
        }),
    )
}

fn cookie(state: &AppState, name: &'static str, value: String, ttl_secs: i64) -> Cookie<'static> {
    Cookie::build((name, value))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Lax)
        .secure(state.cfg.is_https())
        .max_age(time::Duration::seconds(ttl_secs))
        .build()
}

fn removal(name: &'static str) -> Cookie<'static> {
    Cookie::build((name, "")).path("/").build()
}

/// GET /auth/login — sends the user to Discord.
pub async fn login(State(state): State<AppState>, jar: CookieJar) -> (CookieJar, Redirect) {
    let oauth_state = crypto::random_token(24);
    let url = Discord::authorize_url(&state.cfg, &oauth_state);
    let jar = jar.add(cookie(&state, STATE_COOKIE, oauth_state, STATE_TTL_SECS));
    (jar, Redirect::to(&url))
}

#[derive(Deserialize)]
pub struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
}

/// GET /auth/callback — Discord sends the user back here.
pub async fn callback(
    State(state): State<AppState>,
    jar: CookieJar,
    Query(query): Query<CallbackQuery>,
) -> Result<(CookieJar, Redirect), AppError> {
    let expected_state = jar.get(STATE_COOKIE).map(|c| c.value().to_owned());
    let jar = jar.remove(removal(STATE_COOKIE));

    // The user declined, or the URL was opened directly.
    let (Some(code), Some(returned_state)) = (query.code, query.state) else {
        return Ok((jar, Redirect::to("/?e=denied")));
    };
    // `state` proves this callback belongs to a login started in this browser.
    let state_matches = expected_state
        .is_some_and(|expected| bool::from(expected.as_bytes().ct_eq(returned_state.as_bytes())));
    if !state_matches {
        return Ok((jar, Redirect::to("/?e=expired")));
    }

    let access_token = match state.discord.exchange_code(&state.cfg, &code).await {
        Ok(token) => token,
        Err(DiscordError::Unauthorized) => return Ok((jar, Redirect::to("/?e=expired"))),
        Err(error) => return Err(error.into()),
    };
    let user = state.discord.oauth_user(&access_token).await?;

    let now = now();
    sqlx::query(
        "INSERT INTO users (id, username, global_name, avatar, created_at, last_login_at)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT(id) DO UPDATE SET
             username = excluded.username,
             global_name = excluded.global_name,
             avatar = excluded.avatar,
             last_login_at = excluded.last_login_at",
    )
    .bind(&user.id)
    .bind(&user.username)
    .bind(&user.global_name)
    .bind(&user.avatar)
    .bind(now)
    .bind(now)
    .execute(&state.db)
    .await?;

    sqlx::query("DELETE FROM sessions WHERE expires_at <= ?")
        .bind(now)
        .execute(&state.db)
        .await?;

    let session_token = crypto::random_token(32);
    sqlx::query(
        "INSERT INTO sessions (id_hash, user_id, csrf_token, created_at, expires_at)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(crypto::sha256_hex(&session_token))
    .bind(&user.id)
    .bind(crypto::random_token(32))
    .bind(now)
    .bind(now + SESSION_TTL_SECS)
    .execute(&state.db)
    .await?;

    tracing::info!(user = %user.id, "login");
    let jar = jar.add(cookie(
        &state,
        SESSION_COOKIE,
        session_token,
        SESSION_TTL_SECS,
    ));
    Ok((jar, Redirect::to("/dashboard")))
}

#[derive(Deserialize)]
pub struct CsrfForm {
    pub csrf: String,
}

/// POST /auth/logout
pub async fn logout(
    State(state): State<AppState>,
    session: Session,
    jar: CookieJar,
    Form(form): Form<CsrfForm>,
) -> Result<(CookieJar, Redirect), AppError> {
    session.check_csrf(&form.csrf)?;
    sqlx::query("DELETE FROM sessions WHERE id_hash = ?")
        .bind(&session.id_hash)
        .execute(&state.db)
        .await?;
    Ok((jar.remove(removal(SESSION_COOKIE)), Redirect::to("/")))
}
