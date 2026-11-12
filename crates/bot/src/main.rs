//! The Discord bot run in each bot container.
//!
//! Usage: `dc-bot [BOT_FOLDER]` (or the `BOT_CONFIG_DIR` variable). The folder
//! is `bots/<slug>`: it holds `bot.toml`, the token in `.env`, the settings
//! written by the dashboard and the bot's own data.

mod announce;
mod antiscam;
mod api;
mod app;
mod assistant;
mod db;
mod gemini;
mod giveaway;
mod interactions;
mod lang;
mod linkguard;
mod logs;
mod massrole;
mod members;
mod moderation;
mod panels;
mod reminders;
mod requests;
mod rolepanel;
mod scanline;
mod settings;
mod sol;
mod tickets;
mod twitch;
mod ui;
mod verify;
mod welcome;
mod xp;

use std::{path::PathBuf, sync::Arc};

use anyhow::{Context, bail};
use dc_core::registry;
use futures_util::StreamExt;
use serde_json::{Value, json};
use twilight_gateway::{Intents, Message, Shard, ShardId};

use crate::app::{App, CachedMessage, ChannelInfo, GuildInfo, Identity, bits_of, str_of};

const GUILDS: u64 = 1 << 0;
const GUILD_MEMBERS: u64 = 1 << 1;
const GUILD_MESSAGES: u64 = 1 << 9;
const MESSAGE_CONTENT: u64 = 1 << 15;

/// Close code sent by Discord when a privileged intent is not enabled.
const CLOSE_DISALLOWED_INTENTS: u16 = 4014;
const CLOSE_BAD_TOKEN: u16 = 4004;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().init();

    let dir: PathBuf = std::env::args()
        .nth(1)
        .or_else(|| std::env::var("BOT_CONFIG_DIR").ok())
        .unwrap_or_else(|| ".".into())
        .into();
    let slug = std::env::var("BOT_SLUG").ok().unwrap_or_else(|| {
        let name = dir
            .canonicalize()
            .ok()
            .and_then(|p| p.file_name()?.to_str().map(str::to_owned));
        name.unwrap_or_default()
    });
    // In a container, the definition comes with the image (see the README).
    if let Some(seed_dir) = std::env::var_os("BOTS_SEED_DIR")
        && let Some(bots_dir) = dir.parent()
    {
        registry::seed(seed_dir.as_ref(), bots_dir, Some(&slug))
            .context("copying the bot definition")?;
    }
    let def = registry::load_one(&dir, &slug)
        .map_err(|error| anyhow::anyhow!("{}: {error}", dir.display()))?;
    let token = std::env::var("DISCORD_TOKEN")
        .ok()
        .filter(|token| !token.trim().is_empty())
        .or_else(|| def.token())
        .with_context(|| {
            format!(
                "no DISCORD_TOKEN in the environment or in {}/.env",
                dir.display()
            )
        })?;

    let db = db::open(&dir.join("data").join("bot.db"))
        .await
        .context("opening the bot database")?;
    let dashboard_url = std::env::var("DASHBOARD_URL")
        .ok()
        .filter(|url| url.starts_with("http"));
    let app = Arc::new(App::new(
        api::Api::new(token.trim())?,
        db,
        dir,
        def.owners.clone(),
        (def.name.clone(), def.slug.clone(), def.packs.clone()),
        dashboard_url,
    ));
    ui::set_bot_name(&def.name);
    tracing::info!(bot = %def.slug, "starting {}", def.name);
    tokio::spawn(panels::watch(app.clone()));
    tokio::spawn(requests::watch(app.clone()));
    tokio::spawn(giveaway::watch(app.clone()));
    tokio::spawn(twitch::watch(app.clone()));
    tokio::spawn(reminders::watch(app.clone()));
    tokio::spawn(sol::watch(app.clone()));

    // Privileged intents must be enabled on the Developer Portal. Without
    // them Discord closes the connection: fall back to what is allowed.
    let mut privileged = true;
    loop {
        let bits = if privileged {
            GUILDS | GUILD_MESSAGES | GUILD_MEMBERS | MESSAGE_CONTENT
        } else {
            GUILDS | GUILD_MESSAGES
        };
        let mut shard = Shard::new(
            ShardId::ONE,
            token.trim().to_owned(),
            Intents::from_bits_truncate(bits),
        );
        *app.gateway.lock().unwrap() = Some(shard.sender());
        let mut disallowed = false;

        while let Some(item) = shard.next().await {
            match item {
                Ok(Message::Text(text)) => match serde_json::from_str::<Value>(&text) {
                    // Opcode 0 is a dispatched event; the library handles the rest.
                    Ok(mut payload) if payload["op"] == 0 => {
                        let kind = str_of(&payload, "t").to_owned();
                        dispatch(&app, &kind, payload["d"].take()).await;
                    }
                    Ok(_) => {}
                    Err(error) => tracing::warn!(%error, "unreadable gateway payload"),
                },
                Ok(Message::Close(frame)) => match frame.map(|frame| frame.code) {
                    Some(CLOSE_DISALLOWED_INTENTS) => disallowed = true,
                    Some(CLOSE_BAD_TOKEN) => bail!("Discord rejected the bot token"),
                    _ => {}
                },
                Err(error) => tracing::warn!(%error, "gateway error"),
            }
        }

        if disallowed && privileged {
            tracing::error!(
                "privileged intents refused: enable SERVER MEMBERS INTENT and MESSAGE CONTENT INTENT \
                 on the Developer Portal (Bot > Privileged Gateway Intents). Until then welcome \
                 messages, XP, link lock and message logs are unavailable."
            );
            privileged = false;
            continue;
        }
        bail!("connection to Discord closed for good");
    }
}

/// Registers the slash commands, replacing the previous set. An application
/// with Activities enabled also has an Entry Point command (type 4) created
/// by Discord: it must be sent back, or Discord refuses the whole update.
async fn register_commands(app: &App) {
    let path = format!("/applications/{}/commands", app.app_id());
    let mut list = dc_core::commands::for_bot(&app.packs);
    match app.api.get(&path).await {
        Ok(existing) => {
            let entry_points = existing.as_array().into_iter().flatten();
            list.extend(
                entry_points
                    .filter(|command| command["type"] == 4)
                    .map(|command| {
                        json!({
                            "id": command["id"], "name": command["name"], "type": 4,
                            "handler": command["handler"], "description": command["description"],
                        })
                    }),
            );
        }
        Err(error) => tracing::warn!(%error, "listing the existing commands failed"),
    }
    match app.api.put(&path, &json!(list), None).await {
        Ok(_) => tracing::info!("slash commands registered"),
        Err(error) => tracing::error!(%error, "registering slash commands failed"),
    }
}

fn channel_info(channel: &Value) -> ChannelInfo {
    ChannelInfo {
        name: str_of(channel, "name").to_owned(),
        topic: str_of(channel, "topic").to_owned(),
    }
}

/// Handles one gateway event. Cache updates happen inline so they stay in
/// order; anything that talks to Discord runs in its own task.
async fn dispatch(app: &Arc<App>, kind: &str, d: Value) {
    let guild_id = str_of(&d, "guild_id").to_owned();
    match kind {
        "READY" => {
            let first = app
                .identity
                .set(Identity {
                    user_id: str_of(&d["user"], "id").to_owned(),
                    app_id: str_of(&d["application"], "id").to_owned(),
                })
                .is_ok();
            tracing::info!("connected as {}", str_of(&d["user"], "username"));
            panels::apply_presence(app);
            if first {
                register_commands(app).await;
            }
        }
        "GUILD_CREATE" => {
            let id = str_of(&d, "id").to_owned();
            let collect = |key: &str| d[key].as_array().cloned().unwrap_or_default();
            let info = GuildInfo {
                name: str_of(&d, "name").to_owned(),
                owner_id: str_of(&d, "owner_id").to_owned(),
                member_count: d["member_count"].as_u64().unwrap_or(0),
                roles: collect("roles")
                    .iter()
                    .map(|role| (str_of(role, "id").to_owned(), bits_of(&role["permissions"])))
                    .collect(),
                role_names: collect("roles")
                    .iter()
                    .map(|role| {
                        (
                            str_of(role, "id").to_owned(),
                            str_of(role, "name").to_owned(),
                        )
                    })
                    .collect(),
                role_positions: collect("roles")
                    .iter()
                    .map(|role| {
                        let position = role["position"].as_i64().unwrap_or(0);
                        (str_of(role, "id").to_owned(), position)
                    })
                    .collect(),
                channels: collect("channels")
                    .iter()
                    .map(|channel| (str_of(channel, "id").to_owned(), channel_info(channel)))
                    .collect(),
            };
            app.guilds.lock().unwrap().insert(id.clone(), info);
            let app = app.clone();
            tokio::spawn(async move { panels::sync(&app, &id).await });
        }
        "GUILD_UPDATE" => {
            if let Some(guild) = app.guilds.lock().unwrap().get_mut(str_of(&d, "id")) {
                guild.name = str_of(&d, "name").to_owned();
                guild.owner_id = str_of(&d, "owner_id").to_owned();
            }
        }
        "GUILD_DELETE" => {
            // `unavailable` means a Discord outage, not that the bot was removed.
            if d["unavailable"] != true {
                app.guilds.lock().unwrap().remove(str_of(&d, "id"));
            }
        }
        "GUILD_ROLE_CREATE" | "GUILD_ROLE_UPDATE" => {
            if let Some(guild) = app.guilds.lock().unwrap().get_mut(&guild_id) {
                let role = &d["role"];
                guild
                    .roles
                    .insert(str_of(role, "id").to_owned(), bits_of(&role["permissions"]));
                guild.role_names.insert(
                    str_of(role, "id").to_owned(),
                    str_of(role, "name").to_owned(),
                );
                guild.role_positions.insert(
                    str_of(role, "id").to_owned(),
                    role["position"].as_i64().unwrap_or(0),
                );
            }
        }
        "GUILD_ROLE_DELETE" => {
            if let Some(guild) = app.guilds.lock().unwrap().get_mut(&guild_id) {
                guild.roles.remove(str_of(&d, "role_id"));
                guild.role_names.remove(str_of(&d, "role_id"));
                guild.role_positions.remove(str_of(&d, "role_id"));
            }
        }
        "GUILD_MEMBER_ADD" => {
            if let Some(guild) = app.guilds.lock().unwrap().get_mut(&guild_id) {
                guild.member_count += 1;
            }
            let app = app.clone();
            tokio::spawn(async move {
                welcome::on_member_add(&app, &guild_id, &d).await;
                logs::member_joined(&app, &guild_id, &d["user"]).await;
            });
        }
        "GUILD_MEMBER_REMOVE" => {
            if let Some(guild) = app.guilds.lock().unwrap().get_mut(&guild_id) {
                guild.member_count = guild.member_count.saturating_sub(1);
            }
            let app = app.clone();
            tokio::spawn(async move { logs::member_left(&app, &guild_id, &d["user"]).await });
        }
        "CHANNEL_CREATE" => {
            if let Some(guild) = app.guilds.lock().unwrap().get_mut(&guild_id) {
                guild
                    .channels
                    .insert(str_of(&d, "id").to_owned(), channel_info(&d));
            }
            let app = app.clone();
            tokio::spawn(async move { logs::channel_created(&app, &guild_id, &d).await });
        }
        "CHANNEL_UPDATE" => {
            let id = str_of(&d, "id").to_owned();
            let after = channel_info(&d);
            let before = app
                .guilds
                .lock()
                .unwrap()
                .get_mut(&guild_id)
                .and_then(|guild| guild.channels.insert(id.clone(), after.clone()));
            if let Some(before) = before {
                let app = app.clone();
                tokio::spawn(async move {
                    logs::channel_updated(&app, &guild_id, &id, &before, &after).await;
                });
            }
        }
        "CHANNEL_DELETE" => {
            let id = str_of(&d, "id").to_owned();
            if let Some(guild) = app.guilds.lock().unwrap().get_mut(&guild_id) {
                guild.channels.remove(&id);
            }
            let app = app.clone();
            tokio::spawn(async move {
                // A ticket channel deleted by hand must not stay "open".
                let _ = db::delete_ticket(&app.db, &id).await;
                logs::channel_deleted(&app, &guild_id, &d).await;
            });
        }
        "MESSAGE_CREATE" if !guild_id.is_empty() => {
            let from_bot = d["author"]["bot"] == true || !d["webhook_id"].is_null();
            app.messages.lock().unwrap().insert(
                str_of(&d, "id").to_owned(),
                CachedMessage {
                    channel_id: str_of(&d, "channel_id").to_owned(),
                    author_id: str_of(&d["author"], "id").to_owned(),
                    content: str_of(&d, "content").to_owned(),
                    attachments: d["attachments"].as_array().map_or(0, Vec::len),
                    from_bot,
                },
            );
            if !from_bot {
                let app = app.clone();
                tokio::spawn(async move {
                    if antiscam::check(&app, &guild_id, &d, false).await
                        || linkguard::filter_links(&app, &guild_id, &d).await
                    {
                        return;
                    }
                    xp::on_message(&app, &guild_id, &d).await;
                    assistant::on_message(&app, &guild_id, &d).await;
                });
            }
        }
        "MESSAGE_UPDATE" if !guild_id.is_empty() => {
            // Some updates (embeds loading, pins) carry no new text.
            let Some(content) = d["content"].as_str().map(str::to_owned) else {
                return;
            };
            let before = {
                let mut messages = app.messages.lock().unwrap();
                let before = messages.get(str_of(&d, "id"));
                if let Some(cached) = &before {
                    let mut updated = cached.clone();
                    updated.content = content.clone();
                    messages.remove(str_of(&d, "id"));
                    messages.insert(str_of(&d, "id").to_owned(), updated);
                }
                before
            };
            let Some(before) =
                before.filter(|cached| !cached.from_bot && cached.content != content)
            else {
                return;
            };
            let app = app.clone();
            tokio::spawn(async move {
                // Otherwise a link could be added by editing a harmless message.
                if antiscam::check(&app, &guild_id, &d, true).await
                    || linkguard::filter_links(&app, &guild_id, &d).await
                {
                    return;
                }
                logs::message_edited(&app, &guild_id, &d, &before, &content).await;
            });
        }
        "MESSAGE_DELETE" if !guild_id.is_empty() => {
            let cached = app.messages.lock().unwrap().remove(str_of(&d, "id"));
            if cached.as_ref().is_some_and(|message| message.from_bot) {
                return;
            }
            let app = app.clone();
            tokio::spawn(async move {
                logs::message_deleted(&app, &guild_id, str_of(&d, "channel_id"), cached).await;
            });
        }
        "MESSAGE_DELETE_BULK" if !guild_id.is_empty() => {
            let ids = d["ids"].as_array().cloned().unwrap_or_default();
            {
                let mut messages = app.messages.lock().unwrap();
                for id in ids.iter().filter_map(Value::as_str) {
                    messages.remove(id);
                }
            }
            let app = app.clone();
            tokio::spawn(async move {
                let body = format!(
                    "**Channel:** <#{}>\n**Messages:** {}",
                    str_of(&d, "channel_id"),
                    ids.len()
                );
                logs::send(
                    &app,
                    &guild_id,
                    logs::Kind::MessageDelete,
                    ui::RED,
                    "Messages purged",
                    &body,
                )
                .await;
            });
        }
        "INTERACTION_CREATE" => {
            tokio::spawn(interactions::handle(app.clone(), d));
        }
        _ => {}
    }
}
