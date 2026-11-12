//! The status panel (ported from the Misty bot): websites up or down with
//! their recent history, latest version, downloads, players and GitHub
//! stars, in one message edited in place every few minutes. The Minecraft
//! server panel is refreshed at the same pace.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use dc_core::{
    cards::{self, Beat, HISTORY_LEN, SiteRow},
    guild::GuildConfig,
};

use crate::{
    app::App,
    github, minecraft, panels,
    ui::{self, Msg},
};

const EVERY: Duration = Duration::from_secs(300);
/// A website checked less than this long ago is not checked again.
const CHECK_CACHE: Duration = Duration::from_secs(60);
pub const REFRESH: &str = "status:refresh";

/// Last answer of each website: `(when, HTTP status, milliseconds)`.
pub type Checks = Mutex<HashMap<String, (Instant, Option<u16>, u64)>>;

pub struct Snapshot {
    pub sites: Vec<SiteRow>,
    pub numbers: Vec<(String, String)>,
    pub up: usize,
}

async fn check(app: &App, url: &str) -> (Option<u16>, u64) {
    if let Some((at, status, ms)) = app.site_checks.lock().unwrap().get(url)
        && at.elapsed() < CHECK_CACHE
    {
        return (*status, *ms);
    }
    let start = Instant::now();
    let status = app
        .web
        .get(url)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .ok()
        .map(|r| r.status().as_u16());
    let ms = start.elapsed().as_millis() as u64;
    let mut checks = app.site_checks.lock().unwrap();
    if checks.len() > 200 {
        checks.clear();
    }
    checks.insert(url.to_owned(), (Instant::now(), status, ms));
    (status, ms)
}

/// Number of players, when the address answers `{"players": n}`.
async fn players(app: &App, url: &str) -> Option<u64> {
    if url.is_empty() {
        return None;
    }
    let answer: serde_json::Value = app
        .web
        .get(url)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;
    answer.get("players")?.as_u64()
}

/// Checks the websites and reads the numbers. `record`: the result goes in
/// the history (the regular checks, not the Refresh clicks).
pub async fn measure(app: &App, config: &GuildConfig, record: bool) -> Snapshot {
    let status = &config.status;
    let checks =
        futures_util::future::join_all(status.sites.iter().map(|site| check(app, &site.url))).await;
    if record {
        app.watch.update(|state| {
            for (site, (code, ms)) in status.sites.iter().zip(&checks) {
                let beat = match code {
                    Some(200..=399) if *ms > 1500 => Beat::Slow,
                    Some(200..=399) => Beat::Up,
                    _ => Beat::Down,
                };
                let history = state.history.entry(site.url.clone()).or_default();
                history.push_back(beat);
                while history.len() > HISTORY_LEN {
                    history.pop_front();
                }
            }
        });
    }
    let sites: Vec<SiteRow> = app.watch.read(|state| {
        status
            .sites
            .iter()
            .zip(&checks)
            .map(|(site, (code, ms))| SiteRow {
                name: site.name.clone(),
                detail: site
                    .url
                    .trim_start_matches("https://")
                    .trim_start_matches("http://")
                    .trim_end_matches('/')
                    .to_owned(),
                status: *code,
                latency_ms: *ms,
                history: state
                    .history
                    .get(&site.url)
                    .map(|h| h.iter().copied().collect())
                    .unwrap_or_default(),
            })
            .collect()
    });
    let up = sites
        .iter()
        .filter(|s| matches!(s.status, Some(200..=399)))
        .count();

    let mut numbers = Vec::new();
    let repo = status.repo_or(&config.releases);
    if !repo.is_empty() {
        let (releases, info) = tokio::join!(app.github.releases(repo), app.github.repo(repo));
        if let Ok(releases) = &releases {
            if let Some(latest) = github::latest(releases) {
                numbers.push(("Version".to_owned(), latest.tag_name.clone()));
            }
            let total: u64 = releases.iter().map(github::Release::downloads).sum();
            numbers.push(("Downloads".to_owned(), ui::number(total)));
        }
        if let Some(count) = players(app, &status.players_url).await {
            numbers.push(("Players".to_owned(), ui::number(count)));
        }
        if let Ok(info) = info {
            numbers.push(("Stars".to_owned(), ui::number(info.stargazers_count)));
        }
    } else if let Some(count) = players(app, &status.players_url).await {
        numbers.push(("Players".to_owned(), ui::number(count)));
    }
    numbers.truncate(4);
    Snapshot { sites, numbers, up }
}

pub fn card(app: &App, config: &GuildConfig, snapshot: &Snapshot) -> Msg {
    let svg = cards::status(&app.name, "Status", &snapshot.sites, &snapshot.numbers);
    let mut buttons = vec![ui::button(ui::STYLE_SECONDARY, "Refresh", REFRESH)];
    if !config.releases.website.is_empty() {
        buttons.push(ui::link_button("Website", &config.releases.website));
    }
    if !config.releases.download_url.is_empty() {
        buttons.push(ui::link_button("Download", &config.releases.download_url));
    }
    let children = vec![
        ui::text(format!(
            "-# Last check <t:{}:R> · refreshed automatically",
            dc_core::now()
        )),
        ui::row(buttons),
    ];
    let total = snapshot.sites.len();
    let color = if snapshot.up == total {
        ui::GREEN
    } else if snapshot.up == 0 {
        ui::RED
    } else {
        ui::AMBER
    };
    Msg::with_image(
        color,
        "status.png",
        svg,
        children,
        vec![ui::text("## Status")],
    )
}

/// The server's status panel, or `None` when it is not set up.
pub async fn status_panel(app: &App, guild_id: &str, record: bool) -> Option<(String, Msg)> {
    let config = app.config(guild_id);
    let channel = config
        .status
        .channel
        .clone()
        .filter(|_| config.status.enabled)?;
    let snapshot = measure(app, &config, record).await;
    Some((channel, card(app, &config, &snapshot)))
}

/// The Minecraft panel, or `None` when it is not set up.
pub async fn server_panel(app: &App, guild_id: &str) -> Option<(String, Msg)> {
    let config = app.config(guild_id);
    let settings = &config.minecraft;
    let channel = settings
        .channel
        .clone()
        .filter(|_| settings.enabled && !settings.address.is_empty())?;
    let status = app.pinger.status(&app.web, &settings.address).await;
    Some((channel, minecraft::card(settings, &status)))
}

/// Posts or updates both panels of a server.
pub async fn refresh(app: &App, guild_id: &str, record: bool) {
    let status = status_panel(app, guild_id, record).await;
    panels::sync_one(app, guild_id, "status", status).await;
    let server = server_panel(app, guild_id).await;
    panels::sync_one(app, guild_id, "minecraft", server).await;
}

pub async fn watch(app: Arc<App>) {
    loop {
        tokio::time::sleep(EVERY).await;
        let guilds: Vec<String> = app.guilds.lock().unwrap().keys().cloned().collect();
        for guild in guilds {
            let config = app.config(&guild);
            if config.status.enabled || config.minecraft.enabled {
                refresh(&app, &guild, true).await;
            }
        }
    }
}
