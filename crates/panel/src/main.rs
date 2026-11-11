mod activity;
mod auth;
mod bridge;
mod config;
mod discord;
mod docker;
mod error;
mod guilds;
mod routes;
mod views;

use std::{sync::Arc, time::Instant};

use anyhow::Context;
use axum::{
    Router,
    extract::{DefaultBodyLimit, Request, State},
    http::{HeaderValue, header},
    middleware::{self, Next},
    response::Response,
    routing::{get, post},
};
use dc_core::{db, registry};

use crate::{config::Config, discord::Discord, docker::DockerClient};

#[derive(Clone)]
pub struct AppState {
    pub cfg: Arc<Config>,
    pub db: db::Db,
    pub discord: Discord,
    pub docker: DockerClient,
    /// One-time messages shown after a redirect.
    pub notices: bridge::Notices,
    /// Captchas of the verification Activity.
    pub challenges: activity::Challenges,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt().init();

    let cfg = Config::from_env()?;
    if let Some(seed_dir) = &cfg.seed_dir {
        let copied = registry::seed(seed_dir, &cfg.bots_dir, None)
            .with_context(|| format!("copying the bot definitions from {}", seed_dir.display()))?;
        tracing::info!("bot definitions installed: {}", copied.join(", "));
    }
    let (bots, errors) = registry::load(&cfg.bots_dir);
    for error in &errors {
        tracing::warn!("invalid bot ignored: {error}");
    }
    tracing::info!(
        "{} bot(s) loaded from {}",
        bots.len(),
        cfg.bots_dir.display()
    );

    let state = AppState {
        db: db::connect(&cfg.database_url)
            .await
            .context("opening the database")?,
        discord: Discord::new()?,
        docker: DockerClient::connect(),
        notices: Default::default(),
        challenges: Default::default(),
        cfg: Arc::new(cfg),
    };

    let listener = tokio::net::TcpListener::bind(&state.cfg.bind_addr)
        .await
        .with_context(|| format!("listening on {}", state.cfg.bind_addr))?;
    tracing::info!(
        "panel listening on http://{} (public URL: {})",
        listener.local_addr()?,
        state.cfg.public_url
    );

    axum::serve(listener, app(state))
        .with_graceful_shutdown(async {
            tokio::signal::ctrl_c().await.ok();
        })
        .await?;
    Ok(())
}

fn app(state: AppState) -> Router {
    Router::new()
        .route("/", get(routes::index))
        .route("/healthz", get(|| async { "ok" }))
        .route("/assets/app.css", get(routes::stylesheet))
        .route("/assets/app.js", get(routes::script))
        .route("/sw.js", get(routes::service_worker))
        .route("/offline", get(routes::offline))
        .route("/assets/favicon.svg", get(routes::favicon))
        .route("/auth/login", get(auth::login))
        .route("/auth/callback", get(auth::callback))
        .route("/auth/logout", post(auth::logout))
        .route("/dashboard", get(routes::dashboard))
        .route("/bots/{slug}", get(routes::bot_overview))
        .route("/bots/{slug}/servers", get(routes::bot_servers))
        .route("/bots/{slug}/servers/{guild_id}", get(guilds::home))
        .route(
            "/bots/{slug}/servers/{guild_id}/{module}",
            get(guilds::module).post(guilds::save),
        )
        .route(
            "/bots/{slug}/servers/{guild_id}/{module}/test",
            post(guilds::test),
        )
        .route("/bots/{slug}/servers/{guild_id}/reset", post(guilds::reset))
        .route(
            "/bots/{slug}/servers/{guild_id}/preview/{image}",
            get(guilds::preview),
        )
        .route("/bots/{slug}/logs", get(routes::bot_logs))
        .route(
            "/bots/{slug}/settings",
            get(routes::bot_settings).post(routes::save_settings),
        )
        .route(
            "/bots/{slug}/profile",
            // Room for a 2 MB picture, a 4 MB banner and the rest of the form.
            post(routes::save_profile).layer(DefaultBodyLimit::max(7 * 1024 * 1024)),
        )
        .route("/bots/{slug}/power", post(routes::power))
        .route("/activity/{slug}", get(activity::page))
        .route("/activity/{slug}/", get(activity::page))
        .route("/activity/{slug}/app.css", get(routes::stylesheet))
        .route("/activity/{slug}/app.js", get(activity::script))
        .route("/activity/{slug}/session", post(activity::session))
        .route("/activity/{slug}/captcha.png", get(activity::captcha))
        .route("/activity/{slug}/refresh", post(activity::refresh))
        .route("/activity/{slug}/answer", post(activity::answer))
        .fallback(routes::not_found)
        .layer(middleware::from_fn_with_state(
            state.clone(),
            harden_and_log,
        ))
        .with_state(state)
}

/// Scripts and styles from the panel only, none inline. The pages are
/// server-rendered and work without the script (`assets/app.js`).
const CSP: &str = "default-src 'none'; style-src 'self'; script-src 'self'; \
    connect-src 'self'; worker-src 'self'; manifest-src 'self'; \
    img-src 'self' https://cdn.discordapp.com; form-action 'self'; \
    base-uri 'none'; frame-ancestors 'none'";

/// The verification Activity is the one page with a script, and it is
/// displayed inside Discord's own frame, so it cannot forbid framing. It
/// holds no session: there is nothing to hijack by framing it elsewhere.
const ACTIVITY_CSP: &str = "default-src 'none'; style-src 'self'; script-src 'self'; \
    img-src 'self'; connect-src 'self'; form-action 'none'; base-uri 'none'";

/// Adds security headers and logs the request. Only the path is logged, never
/// the query string, which holds the OAuth code on the Discord callback.
async fn harden_and_log(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    let started = Instant::now();

    let mut response = next.run(request).await;

    let in_activity = path.starts_with("/activity/");
    let headers = response.headers_mut();
    // Tells the browser script and service worker that this answer comes
    // from the panel itself, not from a proxy error page.
    headers.insert("x-dc-panel", HeaderValue::from_static("1"));
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(if in_activity { ACTIVITY_CSP } else { CSP }),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    if !in_activity {
        headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    }
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("same-origin"),
    );
    headers
        .entry(header::CACHE_CONTROL)
        .or_insert(HeaderValue::from_static("no-store"));
    if state.cfg.is_https() {
        headers.insert(
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static("max-age=31536000"),
        );
    }

    tracing::info!(
        "{method} {path} -> {} ({} ms)",
        response.status().as_u16(),
        started.elapsed().as_millis()
    );
    response
}
