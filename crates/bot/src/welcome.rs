//! Welcome of new members: roles given on arrival and a message with an
//! image (avatar, name and member number).

use dc_core::{guild::GuildConfig, render};
use serde_json::Value;

use crate::{
    app::{App, display_name, str_of},
    ui::{self, Msg},
};

/// Fills the placeholders of the welcome text.
fn fill(template: &str, user_id: &str, name: &str, server: &str, count: u64) -> String {
    template
        .replace("{user}", &format!("<@{user_id}>"))
        .replace("{name}", &ui::escape(name))
        .replace("{server}", &ui::escape(server))
        .replace("{count}", &ui::number(count))
}

/// Builds the welcome message of a member.
pub async fn message(
    app: &App,
    guild_id: &str,
    config: &GuildConfig,
    user: &Value,
    member: &Value,
) -> Msg {
    let guild = app.guild(guild_id);
    let user_id = str_of(user, "id");
    let name = display_name(user, member);
    let text = fill(
        &config.welcome.message,
        user_id,
        &name,
        &guild.name,
        guild.member_count,
    );

    let image = if config.welcome.image {
        let picture = app.avatar_png(user).await;
        let subtitle = format!(
            "Member #{} · {}",
            ui::number(guild.member_count),
            guild.name
        );
        tokio::task::spawn_blocking(move || {
            render::png(&render::welcome(&name, &subtitle, picture.as_deref()))
        })
        .await
        .ok()
        .and_then(|result| {
            result
                .map_err(|error| tracing::error!(%error, "rendering the welcome card failed"))
                .ok()
        })
    } else {
        None
    };

    let mut children = vec![ui::text(text)];
    let mut msg = Msg::default();
    if let Some(png) = image {
        children.push(ui::gallery("attachment://welcome.png"));
        msg = msg.file("welcome.png", png);
    }
    msg.components = vec![ui::container(ui::WHITE, children)];
    msg.ping(user_id)
}

/// Mentions a new member in each chosen channel, then deletes the mentions:
/// the channels light up for them without leaving messages behind.
async fn ping_member(app: &App, guild_id: &str, channels: &[String], user_id: &str, seconds: u32) {
    let mut ping = Msg::default().ping(user_id);
    ping.components = vec![ui::text(format!("<@{user_id}>"))];
    let sends = channels.iter().map(|channel| async {
        match app.api.send_message(channel, &ping).await {
            Ok(sent) => Some((channel.clone(), str_of(&sent, "id").to_owned())),
            Err(error) => {
                tracing::warn!(guild = guild_id, %error, "join ping failed");
                None
            }
        }
    });
    let sent: Vec<(String, String)> = futures_util::future::join_all(sends)
        .await
        .into_iter()
        .flatten()
        .collect();
    tokio::time::sleep(std::time::Duration::from_secs(seconds.into())).await;
    for (channel, message) in sent {
        if let Err(error) = app.api.delete_message(&channel, &message).await {
            tracing::warn!(guild = guild_id, %error, "deleting a join ping failed");
        }
    }
}

pub async fn on_member_add(app: &App, guild_id: &str, member: &Value) {
    let config = app.config(guild_id);
    let welcome = &config.welcome;
    let user = &member["user"];
    if user["bot"] == true {
        return;
    }
    let user_id = str_of(user, "id");
    if !welcome.ping_channels.is_empty() {
        ping_member(
            app,
            guild_id,
            &welcome.ping_channels,
            user_id,
            welcome.ping_seconds,
        )
        .await;
    }
    if !welcome.enabled {
        return;
    }
    for role in &welcome.auto_roles {
        if let Err(error) = app
            .api
            .add_role(guild_id, user_id, role, "Role given on arrival")
            .await
        {
            tracing::warn!(guild = guild_id, %error, "giving the arrival role failed");
        }
    }
    if let Some(channel) = &welcome.channel {
        let msg = message(app, guild_id, &config, user, member).await;
        if let Err(error) = app.api.send_message(channel, &msg).await {
            tracing::warn!(guild = guild_id, %error, "sending the welcome message failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders() {
        let text = fill(
            "Hi {user} ({name}) on {server}, #{count}",
            "42",
            "A*b",
            "My_Server",
            1234,
        );
        assert_eq!(text, "Hi <@42> (A\\*b) on My\\_Server, #1,234");
    }
}
