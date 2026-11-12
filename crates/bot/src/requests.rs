//! Requests coming from the dashboard (see `dc_core::requests`): test
//! messages, verifications passed in the Activity, and XP resets.

use std::{sync::Arc, time::Duration};

use dc_core::requests::{self, Request};
use serde_json::Value;

use crate::{
    app::App,
    db, logs, panels,
    ui::{self, Msg},
    verify, welcome, xp,
};

const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// Background task: answers the dashboard's requests as they arrive.
pub async fn watch(app: Arc<App>) {
    loop {
        tokio::time::sleep(POLL_INTERVAL).await;
        for (id, request) in requests::take_pending(&app.dir) {
            let app = app.clone();
            tokio::spawn(async move {
                let result = handle(&app, &request).await;
                if let Err(reason) = &result {
                    tracing::info!(?request, reason, "dashboard request refused");
                }
                if let Err(error) = requests::answer(&app.dir, &id, result) {
                    tracing::warn!(%error, "answering a dashboard request failed");
                }
            });
        }
    }
}

/// `Ok` and `Err` both carry a sentence shown on the dashboard.
async fn handle(app: &App, request: &Request) -> Result<String, String> {
    let guild_id = match request {
        Request::Test { guild_id, .. }
        | Request::Verify { guild_id, .. }
        | Request::ResetXp { guild_id } => guild_id,
    };
    if !app.guilds.lock().unwrap().contains_key(guild_id) {
        return Err("The bot is not connected to this server right now.".into());
    }
    match request {
        Request::Test {
            module, user_id, ..
        } => test(app, guild_id, module, user_id).await,
        Request::Verify { user_id, .. } => {
            let rules = app.config(guild_id).rules.clone();
            let mode = rules.verification.as_str();
            if !rules.verifying() || (mode != "activity" && mode != "web") {
                return Err("This server no longer uses this verification.".into());
            }
            match verify::grant_roles(app, guild_id, user_id).await {
                Ok(()) => Ok("Verified.".into()),
                Err(verify::GrantError::NotSetUp) => {
                    Err("Verification is not set up on this server.".into())
                }
                Err(verify::GrantError::Discord(error)) => Err(error.user_message()),
            }
        }
        Request::ResetXp { .. } => match db::reset_xp(&app.db, guild_id).await {
            Ok(members) => {
                let body = format!("The XP of {members} member(s) was erased from the dashboard.");
                logs::moderation(app, guild_id, "XP reset", &body).await;
                Ok(format!("XP erased for {members} member(s)."))
            }
            Err(error) => {
                tracing::error!(%error, "erasing XP failed");
                Err("The XP could not be erased.".into())
            }
        },
    }
}

fn channel_label(app: &App, guild_id: &str, channel_id: &str) -> String {
    match app.guild(guild_id).channels.get(channel_id) {
        Some(channel) => format!("#{}", channel.name),
        None => "the channel".into(),
    }
}

/// The dashboard user as `(user, member)`; the member is null when they are
/// not on the server (a panel admin, for instance).
async fn sample_member(app: &App, guild_id: &str, user_id: &str) -> Result<(Value, Value), String> {
    if let Ok(mut member) = app
        .api
        .get(&format!("/guilds/{guild_id}/members/{user_id}"))
        .await
    {
        return Ok((member["user"].take(), member));
    }
    match app.api.get(&format!("/users/{user_id}")).await {
        Ok(user) => Ok((user, Value::Null)),
        Err(error) => Err(error.user_message()),
    }
}

fn marked(msg: Msg) -> Msg {
    msg.push(ui::text("-# Test sent from the dashboard."))
}

/// Posts a sample of what a section sends, using the saved settings.
async fn test(app: &App, guild_id: &str, module: &str, user_id: &str) -> Result<String, String> {
    let config = app.config(guild_id);
    let missing = |what: &str| Err(format!("Choose {what} and save first."));

    let (channel, msg) = match module {
        "welcome" => {
            let Some(channel) = config.welcome.channel.clone() else {
                return missing("a welcome channel");
            };
            let (user, member) = sample_member(app, guild_id, user_id).await?;
            let msg = welcome::message(app, guild_id, &config, &user, &member).await;
            (channel, marked(msg))
        }
        "levels" => {
            let Some(channel) = config.xp.announce_channel.clone() else {
                return missing("an announcement channel");
            };
            let (user, _) = sample_member(app, guild_id, user_id).await?;
            let reward: Vec<String> = config
                .xp
                .rewards
                .first()
                .map(|reward| format!("<@&{}>", reward.role))
                .into_iter()
                .collect();
            let level = config.xp.rewards.first().map_or(5, |r| u64::from(r.level));
            (channel, marked(xp::level_up(&user, level, &reward)))
        }
        "logs" => {
            let Some(channel) = config.logs.channel.clone() else {
                return missing("a log channel");
            };
            (channel, logs::sample())
        }
        "twitch" => {
            let twitch = &config.twitch;
            let Some(channel) = twitch.channel.clone() else {
                return missing("an alert channel");
            };
            let login = twitch.logins.first().map_or("streamer", String::as_str);
            let stream = serde_json::json!({
                "user_name": login,
                "title": "Test alert from the dashboard",
                "game_name": "Just Chatting",
            });
            (
                channel,
                crate::twitch::alert(login, &stream, twitch.role.as_deref()),
            )
        }
        // Panels are permanent messages: a test posts them again.
        "reminders" => {
            let reminders = &config.reminders;
            let targets = [
                (&reminders.channel_fr, crate::lang::Lang::Fr),
                (&reminders.channel_en, crate::lang::Lang::En),
            ];
            let mut posted = Vec::new();
            for (channel, lang) in targets {
                let Some(channel) = channel else { continue };
                app.api
                    .send_message(channel, &crate::reminders::reminder(lang))
                    .await
                    .map_err(|error| error.user_message())?;
                posted.push(channel_label(app, guild_id, channel));
            }
            return if posted.is_empty() {
                missing("a French or English channel")
            } else {
                Ok(format!("Reminder sent to {}.", posted.join(" and ")))
            };
        }
        "rules" | "tickets" | "roles" | "verification" => {
            let (enabled, channel) = match module {
                "rules" => (config.rules.enabled, config.rules.channel.clone()),
                "verification" => {
                    let Some(channel) = config.rules.verify_message_channel() else {
                        return Err(if config.rules.verify_under_rules() {
                            "The Verify button is under the rules: post the rules panel again from the Rules section."
                                .into()
                        } else {
                            "Turn the verification on, choose its channel and save first.".into()
                        });
                    };
                    (true, Some(channel.to_owned()))
                }
                "roles" => (
                    config.role_panel.enabled && !config.role_panel.buttons.is_empty(),
                    config.role_panel.channel.clone(),
                ),
                _ => (config.tickets.enabled, config.tickets.panel_channel.clone()),
            };
            let Some(channel) = channel.filter(|_| enabled) else {
                return Err("Enable the panel, choose its channel and save first.".into());
            };
            panels::repost(app, guild_id, module).await;
            return match db::get_panel(&app.db, guild_id, module).await {
                Ok(Some(_)) => Ok(format!(
                    "Panel posted again in {}.",
                    channel_label(app, guild_id, &channel)
                )),
                _ => Err(format!(
                    "The panel could not be posted in {}. Check that the bot can see the \
                     channel and send messages there.",
                    channel_label(app, guild_id, &channel)
                )),
            };
        }
        _ => return Err("There is nothing to test in this section.".into()),
    };

    match app.api.send_message(&channel, &msg).await {
        Ok(_) => Ok(format!(
            "Test sent to {}.",
            channel_label(app, guild_id, &channel)
        )),
        Err(error) => {
            tracing::warn!(guild = guild_id, module, %error, "sending a test failed");
            Err(error.user_message())
        }
    }
}
