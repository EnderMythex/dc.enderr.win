//! Server logs: deleted and edited messages, channel changes, member
//! arrivals and departures, and moderation actions.

use dc_core::guild::Logs;
use serde_json::Value;

use crate::{
    app::{App, CachedMessage, ChannelInfo, avatar_url, str_of},
    ui::{self, Msg},
};

#[derive(Clone, Copy)]
pub enum Kind {
    MessageDelete,
    MessageEdit,
    Channels,
    Members,
    Moderation,
}

impl Kind {
    fn is_on(self, logs: &Logs) -> bool {
        match self {
            Self::MessageDelete => logs.message_delete,
            Self::MessageEdit => logs.message_edit,
            Self::Channels => logs.channels,
            Self::Members => logs.members,
            Self::Moderation => logs.moderation,
        }
    }
}

/// Posts an entry in the log channel, if that kind of log is enabled.
pub async fn send(app: &App, guild_id: &str, kind: Kind, color: u32, title: &str, body: &str) {
    send_with_thumb(app, guild_id, kind, color, title, body, None).await;
}

async fn send_with_thumb(
    app: &App,
    guild_id: &str,
    kind: Kind,
    color: u32,
    title: &str,
    body: &str,
    thumbnail: Option<&str>,
) {
    let config = app.config(guild_id);
    let Some(channel) = &config.logs.channel else {
        return;
    };
    if !config.logs.enabled || !kind.is_on(&config.logs) {
        return;
    }
    let content = format!("### {title}\n{body}");
    let block = match thumbnail {
        Some(url) => ui::section_thumb(content, url),
        None => ui::text(content),
    };
    let footer = ui::text(format!("-# <t:{}:f>", dc_core::now()));
    let msg = Msg::card(color, vec![block, ui::separator(), footer]);
    if let Err(error) = app.api.send_message(channel, &msg).await {
        tracing::warn!(guild = guild_id, %error, "writing to the log channel failed");
    }
}

/// A sample entry, whatever kinds of logs are switched on.
pub fn sample() -> Msg {
    let content = "### Test entry\nThis is what a log entry looks like. Deleted and edited \
                   messages, channel changes, members and moderation actions are reported \
                   here, depending on what is enabled.";
    let footer = ui::text(format!("-# <t:{}:f>", dc_core::now()));
    Msg::card(ui::WHITE, vec![ui::text(content), ui::separator(), footer])
}

fn quote(content: &str) -> String {
    if content.trim().is_empty() {
        return "> *(no text)*".into();
    }
    ui::truncate(content, 1200)
        .lines()
        .map(|line| format!("> {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

pub async fn message_deleted(
    app: &App,
    guild_id: &str,
    channel_id: &str,
    cached: Option<CachedMessage>,
) {
    let body = match cached {
        Some(message) => {
            let attachments = match message.attachments {
                0 => String::new(),
                n => format!("\n-# {n} attachment(s)"),
            };
            format!(
                "**Author:** <@{}>\n**Channel:** <#{channel_id}>\n{}{attachments}",
                message.author_id,
                quote(&message.content)
            )
        }
        // Sent before the bot started: Discord only gives the message id.
        None => format!(
            "**Channel:** <#{channel_id}>\n-# The content of this older message is unknown."
        ),
    };
    send(
        app,
        guild_id,
        Kind::MessageDelete,
        ui::RED,
        "Message deleted",
        &body,
    )
    .await;
}

pub async fn message_edited(
    app: &App,
    guild_id: &str,
    message: &Value,
    before: &CachedMessage,
    after: &str,
) {
    let link = format!(
        "https://discord.com/channels/{guild_id}/{}/{}",
        before.channel_id,
        str_of(message, "id")
    );
    let body = format!(
        "**Author:** <@{}>\n**Channel:** <#{}> · [Jump to message]({link})\n**Before**\n{}\n**After**\n{}",
        before.author_id,
        before.channel_id,
        quote(&before.content),
        quote(after)
    );
    send(
        app,
        guild_id,
        Kind::MessageEdit,
        ui::AMBER,
        "Message edited",
        &body,
    )
    .await;
}

pub async fn channel_created(app: &App, guild_id: &str, channel: &Value) {
    let body = format!(
        "<#{}> · `{}`",
        str_of(channel, "id"),
        str_of(channel, "name")
    );
    send(
        app,
        guild_id,
        Kind::Channels,
        ui::GREEN,
        "Channel created",
        &body,
    )
    .await;
}

pub async fn channel_deleted(app: &App, guild_id: &str, channel: &Value) {
    let body = format!("`#{}`", str_of(channel, "name"));
    send(
        app,
        guild_id,
        Kind::Channels,
        ui::RED,
        "Channel deleted",
        &body,
    )
    .await;
}

/// Logs a rename or topic change. Other updates (position, permissions) are
/// frequent and noisy, so they are ignored.
pub async fn channel_updated(
    app: &App,
    guild_id: &str,
    channel_id: &str,
    before: &ChannelInfo,
    after: &ChannelInfo,
) {
    let mut changes = Vec::new();
    if before.name != after.name {
        changes.push(format!("**Name:** `{}` → `{}`", before.name, after.name));
    }
    if before.topic != after.topic {
        let show = |topic: &str| {
            if topic.is_empty() {
                "*(none)*".to_owned()
            } else {
                ui::truncate(&ui::escape(topic), 300)
            }
        };
        changes.push(format!(
            "**Topic:** {} → {}",
            show(&before.topic),
            show(&after.topic)
        ));
    }
    if changes.is_empty() {
        return;
    }
    let body = format!("<#{channel_id}>\n{}", changes.join("\n"));
    send(
        app,
        guild_id,
        Kind::Channels,
        ui::AMBER,
        "Channel edited",
        &body,
    )
    .await;
}

pub async fn member_joined(app: &App, guild_id: &str, user: &Value) {
    let id = str_of(user, "id");
    // The creation date of an account is encoded in its id.
    let created = (id.parse::<u64>().unwrap_or(0) >> 22) / 1000 + 1_420_070_400;
    let body = format!(
        "<@{id}> · `{}`\n**Account created:** <t:{created}:R>",
        str_of(user, "username")
    );
    let avatar = avatar_url(user, 128);
    send_with_thumb(
        app,
        guild_id,
        Kind::Members,
        ui::GREEN,
        "Member joined",
        &body,
        Some(&avatar),
    )
    .await;
}

pub async fn member_left(app: &App, guild_id: &str, user: &Value) {
    let body = format!("<@{}> · `{}`", str_of(user, "id"), str_of(user, "username"));
    let avatar = avatar_url(user, 128);
    send_with_thumb(
        app,
        guild_id,
        Kind::Members,
        ui::GREY,
        "Member left",
        &body,
        Some(&avatar),
    )
    .await;
}

pub async fn moderation(app: &App, guild_id: &str, title: &str, body: &str) {
    send(app, guild_id, Kind::Moderation, ui::WHITE, title, body).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes() {
        assert_eq!(quote("a\nb"), "> a\n> b");
        assert_eq!(quote("  "), "> *(no text)*");
    }
}
