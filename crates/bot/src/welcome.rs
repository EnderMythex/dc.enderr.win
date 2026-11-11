//! Welcome of new members: roles given on arrival and a message with an
//! image (avatar, name and member number).

use dc_core::{cards::Tone, guild::GuildConfig, render};
use serde_json::Value;

use crate::{
    app::{App, display_name, str_of},
    interactions::Ctx,
    lang::Lang,
    linkguard, reminders,
    ui::{self, Msg},
};

pub const LANG_PREFIX: &str = "welcome-lang:";

/// The private message sent to new members, in French (the main text) or
/// English (the optional second text).
pub fn dm(config: &GuildConfig, guild_id: &str, server: &str, user_id: &str, lang: Lang) -> Msg {
    let welcome = &config.welcome;
    let english = !welcome.dm_text_en.is_empty();
    let text = match lang {
        Lang::En if english => &welcome.dm_text_en,
        _ => &welcome.dm_text,
    };
    let mut children = vec![ui::text(fill(text, user_id, "", server, 0))];
    let tips_lang = if english { lang } else { Lang::En };
    if welcome.dm_safety {
        children.push(ui::separator());
        children.extend(reminders::tips_blocks(tips_lang));
    }
    let mut buttons: Vec<Value> = welcome
        .dm_links
        .iter()
        .map(|link| ui::link_button(&link.label, &link.url))
        .collect();
    if english {
        let (other, label) = match lang {
            Lang::Fr => ("en", "🇬🇧 English"),
            Lang::En => ("fr", "🇫🇷 Français"),
        };
        buttons.push(ui::button(
            ui::STYLE_SECONDARY,
            label,
            &format!("{LANG_PREFIX}{other}:{guild_id}"),
        ));
    }
    if !buttons.is_empty() {
        children.push(ui::separator());
        children.push(ui::row(buttons));
    }
    let title = match lang {
        Lang::Fr if english => format!("Bienvenue sur {server}"),
        _ => format!("Welcome to {server}"),
    };
    let mut msg = Msg::banner(Tone::Info, &title, "", children);
    if welcome.dm_safety {
        msg = msg.file(reminders::CLOSE_DMS_FILE, reminders::CLOSE_DMS_PNG.to_vec());
    }
    msg
}

/// The language button of the welcome DM.
pub async fn switch_language(ctx: &Ctx) {
    let rest = ctx.custom_id().trim_start_matches(LANG_PREFIX);
    let (lang, guild_id) = rest.split_once(':').unwrap_or(("en", ""));
    if !dc_core::is_snowflake(guild_id) {
        return ctx
            .reply(Msg::error("This button is no longer active."))
            .await;
    }
    let lang = if lang == "fr" { Lang::Fr } else { Lang::En };
    let config = ctx.app.config(guild_id);
    let server = ctx.app.guild(guild_id).name;
    ctx.update(dm(&config, guild_id, &server, ctx.user_id(), lang))
        .await;
}

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
    if welcome.dm_enabled {
        let server = app.guild(guild_id).name;
        let msg = dm(&config, guild_id, &server, user_id, Lang::Fr);
        let sent = match linkguard::dm_channel(app, user_id).await {
            Some(channel) => app.api.send_message(&channel, &msg).await.is_ok(),
            None => false,
        };
        if !sent {
            // Common and harmless: the member does not accept DMs.
            tracing::info!(guild = guild_id, "the welcome DM could not be sent");
        }
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
    fn welcome_dms() {
        let mut config = GuildConfig::default();
        config.welcome.dm_text = "Bienvenue {user} !".into();
        config.welcome.dm_links = vec![dc_core::guild::LinkButton {
            label: "Axiom".into(),
            url: "https://axiom.trade/@x".into(),
        }];
        let french_only = dm(&config, "1", "Scanline", "42", Lang::Fr)
            .payload()
            .to_string();
        assert!(french_only.contains("Bienvenue <@42> !"));
        assert!(french_only.contains("https://axiom.trade/@x"));
        assert!(!french_only.contains("welcome-lang:"));
        assert!(french_only.contains("attachment://close-dms.png"));

        config.welcome.dm_text_en = "Welcome {user}!".into();
        config.welcome.dm_safety = false;
        let english = dm(&config, "1", "Scanline", "42", Lang::En)
            .payload()
            .to_string();
        assert!(english.contains("Welcome <@42>!"));
        assert!(english.contains("welcome-lang:fr:1"));
        assert!(!english.contains("close-dms.png"));
    }

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
