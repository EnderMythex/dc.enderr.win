//! Keeps the rules panel and the ticket panel in sync with the
//! configuration: when the dashboard changes a text or a channel, the bot
//! edits or re-posts its message by itself.

use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, SystemTime},
};

use dc_core::{crypto::sha256_hex, guild, settings::BotSettings};
use serde_json::json;

use crate::{
    app::{App, str_of},
    db::{self, Panel},
    rolepanel, tickets,
    ui::Msg,
    verify,
};

const POLL_INTERVAL: Duration = Duration::from_secs(4);

/// Makes one panel of a server match what the configuration asks for.
pub async fn sync_one(app: &App, guild_id: &str, kind: &str, wanted: Option<(String, Msg)>) {
    let existing = match db::get_panel(&app.db, guild_id, kind).await {
        Ok(existing) => existing,
        Err(error) => return tracing::error!(%error, "reading a panel failed"),
    };
    let Some((channel, msg)) = wanted else {
        // Module turned off: take the panel down.
        if let Some(panel) = existing {
            let _ = app
                .api
                .delete_message(&panel.channel_id, &panel.message_id)
                .await;
            let _ = db::delete_panel(&app.db, guild_id, kind).await;
        }
        return;
    };
    let hash = sha256_hex(&msg.fingerprint());

    if let Some(panel) = &existing {
        if panel.channel_id == channel {
            if panel.hash == hash {
                return;
            }
            match app
                .api
                .edit_message(&channel, &panel.message_id, &msg)
                .await
            {
                Ok(_) => {
                    let updated = Panel {
                        hash,
                        ..panel.clone()
                    };
                    let _ = db::set_panel(&app.db, guild_id, kind, &updated).await;
                    return;
                }
                // The message was deleted by hand: post it again below.
                Err(error) if error.is_not_found() => {}
                Err(error) => {
                    return tracing::warn!(guild = guild_id, kind, %error, "editing a panel failed");
                }
            }
        } else {
            let _ = app
                .api
                .delete_message(&panel.channel_id, &panel.message_id)
                .await;
        }
    }

    match app.api.send_message(&channel, &msg).await {
        Ok(sent) => {
            let panel = Panel {
                channel_id: channel,
                message_id: str_of(&sent, "id").to_owned(),
                hash,
            };
            if let Err(error) = db::set_panel(&app.db, guild_id, kind, &panel).await {
                tracing::error!(%error, "saving a panel failed");
            }
        }
        Err(error) => tracing::warn!(guild = guild_id, kind, %error, "posting a panel failed"),
    }
}

pub async fn sync(app: &App, guild_id: &str) {
    let config = app.config(guild_id);
    let rules = config
        .rules
        .channel
        .clone()
        .filter(|_| config.rules.enabled)
        .map(|channel| (channel, verify::panel(&config)));
    sync_one(app, guild_id, "rules", rules).await;

    let verification = config
        .rules
        .verify_message_channel()
        .map(|channel| (channel.to_owned(), verify::verify_panel(&config)));
    sync_one(app, guild_id, "verification", verification).await;

    let ticket_panel = config
        .tickets
        .panel_channel
        .clone()
        .filter(|_| config.tickets.enabled)
        .map(|channel| (channel, tickets::panel(&config)));
    sync_one(app, guild_id, "tickets", ticket_panel).await;

    let role_panel = config
        .role_panel
        .channel
        .clone()
        .filter(|_| config.role_panel.enabled && !config.role_panel.buttons.is_empty())
        .map(|channel| (channel, rolepanel::panel(&config)));
    sync_one(app, guild_id, "roles", role_panel).await;

    // The status and Minecraft panels (checked over the network).
    crate::uptime::refresh(app, guild_id, false).await;
}

/// Deletes a panel and posts it again at the bottom of its channel.
pub async fn repost(app: &App, guild_id: &str, kind: &str) {
    if let Ok(Some(panel)) = db::get_panel(&app.db, guild_id, kind).await {
        let _ = app
            .api
            .delete_message(&panel.channel_id, &panel.message_id)
            .await;
        let _ = db::delete_panel(&app.db, guild_id, kind).await;
    }
    sync(app, guild_id).await;
}

/// Gateway payload (opcode 3) for the presence chosen on the dashboard.
/// `sol_price`: the last SOL price, shown as "Watching SOL: $x";
/// `stats`: "1,234 players · 0.3.7" for `project_stats`.
pub fn presence_payload(
    settings: &BotSettings,
    sol_price: Option<f64>,
    stats: Option<&str>,
) -> String {
    let live = match settings.activity_type.as_str() {
        "sol_price" => Some(sol_price.map(crate::sol::label)),
        "project_stats" => Some(stats.map(str::to_owned)),
        _ => None,
    };
    if let Some(label) = live {
        let activities = match label {
            Some(name) => vec![json!({ "type": 3, "name": name })],
            None => Vec::new(),
        };
        return json!({
            "op": 3,
            "d": { "since": null, "activities": activities, "status": settings.presence_status, "afk": false }
        })
        .to_string();
    }
    let activity_type = match settings.activity_type.as_str() {
        "playing" => Some(0),
        "listening" => Some(2),
        "watching" => Some(3),
        "custom" => Some(4),
        "competing" => Some(5),
        _ => None,
    };
    let activities = match activity_type {
        Some(kind) if !settings.activity_text.is_empty() => {
            // A custom status shows `state`; the other types show `name`.
            vec![
                json!({ "type": kind, "name": settings.activity_text, "state": settings.activity_text }),
            ]
        }
        _ => Vec::new(),
    };
    json!({
        "op": 3,
        "d": { "since": null, "activities": activities, "status": settings.presence_status, "afk": false }
    })
    .to_string()
}

pub fn apply_presence(app: &App) {
    let settings = match BotSettings::load(&app.dir) {
        Ok(settings) => settings,
        Err(error) => return tracing::warn!(%error, "unreadable settings.json"),
    };
    if let Some(sender) = app.gateway.lock().unwrap().as_ref()
        && let Err(error) = sender.send(presence_payload(
            &settings,
            *app.sol_price.lock().unwrap(),
            app.stats_label.lock().unwrap().as_deref(),
        ))
    {
        tracing::warn!(%error, "updating the presence failed");
    }
}

fn modified(path: &std::path::Path) -> Option<SystemTime> {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
}

/// Loads the picture drawn behind the bot's cards (changed on the panel).
pub fn apply_background(app: &App) {
    let dim = BotSettings::load(&app.dir)
        .map(|settings| settings.background_dim)
        .unwrap_or(dc_core::settings::BACKGROUND_DIM);
    let background = dc_core::render::Background::load(&app.dir, dim);
    tracing::info!(enabled = background.is_some(), "card background loaded");
    dc_core::render::set_background(background);
}

/// Background task: notices configuration files changed by the dashboard and
/// applies them (panels, presence, card background) within a few seconds.
pub async fn watch(app: Arc<App>) {
    let settings_path = app.dir.join(dc_core::settings::FILE_NAME);
    let mut settings_seen = modified(&settings_path);
    let background_path = app.dir.join(dc_core::render::BACKGROUND_FILE);
    let mut background_seen = modified(&background_path);
    apply_background(&app);
    let mut seen: HashMap<String, SystemTime> = HashMap::new();
    let mut first_pass = true;

    loop {
        tokio::time::sleep(POLL_INTERVAL).await;

        let settings_now = modified(&settings_path);
        let background_now = modified(&background_path);
        let settings_changed = settings_now != settings_seen;
        if settings_changed {
            settings_seen = settings_now;
            apply_presence(&app);
        }
        // The darkening lives in settings.json, the picture in its own file.
        if settings_changed || background_now != background_seen {
            background_seen = background_now;
            apply_background(&app);
        }

        let Ok(entries) = std::fs::read_dir(app.dir.join(guild::DIR_NAME)) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(guild_id) = path
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.strip_suffix(".json"))
                .filter(|id| dc_core::is_snowflake(id))
                .map(str::to_owned)
            else {
                continue;
            };
            let Some(changed_at) = modified(&path) else {
                continue;
            };
            let is_new = seen.insert(guild_id.clone(), changed_at) != Some(changed_at);
            // Servers are already synced when the bot connects.
            if is_new && !first_pass && app.guilds.lock().unwrap().contains_key(&guild_id) {
                sync(&app, &guild_id).await;
            }
        }
        first_pass = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presence() {
        let mut settings = BotSettings::default();
        let idle: serde_json::Value =
            serde_json::from_str(&presence_payload(&settings, None, None)).unwrap();
        assert_eq!(idle["op"], 3);
        assert_eq!(idle["d"]["status"], "online");
        assert_eq!(idle["d"]["activities"], json!([]));

        settings.presence_status = "dnd".into();
        settings.activity_type = "watching".into();
        settings.activity_text = "the server".into();
        let busy: serde_json::Value =
            serde_json::from_str(&presence_payload(&settings, None, None)).unwrap();
        assert_eq!(busy["d"]["status"], "dnd");
        assert_eq!(busy["d"]["activities"][0]["type"], 3);
        assert_eq!(busy["d"]["activities"][0]["name"], "the server");

        settings.activity_type = "sol_price".into();
        let sol: serde_json::Value =
            serde_json::from_str(&presence_payload(&settings, Some(150.5), None)).unwrap();
        assert_eq!(sol["d"]["activities"][0]["name"], "SOL: $150.50");
        assert_eq!(sol["d"]["activities"][0]["type"], 3);

        settings.activity_type = "project_stats".into();
        let stats: serde_json::Value = serde_json::from_str(&presence_payload(
            &settings,
            None,
            Some("42 players · 0.3.7"),
        ))
        .unwrap();
        assert_eq!(stats["d"]["activities"][0]["name"], "42 players · 0.3.7");
        let waiting: serde_json::Value =
            serde_json::from_str(&presence_payload(&settings, None, None)).unwrap();
        assert_eq!(waiting["d"]["activities"], json!([]));
    }
}
