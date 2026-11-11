//! Link lock: removes links members may not post, optionally times them
//! out, and lets them send the link to the staff for review (the staff
//! accepts — the bot posts it — or refuses). Review flow ported from the
//! Scanline bot.

use std::time::{Duration, Instant};

use dc_core::{cards::Tone, crypto::random_token};
use serde_json::{Value, json};

use crate::{
    antiscam,
    app::{App, roles_of, str_of},
    db,
    interactions::Ctx,
    logs,
    moderation::blocked_link,
    ui::{self, Msg},
};

pub const SUBMIT_PREFIX: &str = "link-submit:";
pub const REVIEW_PREFIX: &str = "link-review:";
/// How long the member may send a removed link to the staff.
const SUBMIT_LIFETIME: Duration = Duration::from_secs(3600);
/// When their DMs are closed, the notice stays this long in the channel.
const CHANNEL_NOTICE_LIFETIME: Duration = Duration::from_secs(60);
const X_DOMAINS: [&str; 2] = ["x.com", "twitter.com"];

/// A removed link its author may still send to the staff.
#[derive(Debug, Clone)]
pub struct Pending {
    guild_id: String,
    user_id: String,
    channel_id: String,
    content: String,
    expires: Instant,
}

/// Opens (or reuses) the private channel with a user.
pub async fn dm_channel(app: &App, user: &str) -> Option<String> {
    let channel = app
        .api
        .post("/users/@me/channels", &json!({ "recipient_id": user }))
        .await
        .ok()?;
    Some(str_of(&channel, "id").to_owned()).filter(|id| !id.is_empty())
}

/// Is this link check skipped in this channel?
fn watched(config: &dc_core::guild::LockUrl, channel: &str) -> bool {
    match config.scope.as_str() {
        "listed" => config.channels.iter().any(|id| id == channel),
        _ => !config.exempt_channels.iter().any(|id| id == channel),
    }
}

/// Deletes the message if it has a link its author may not post. `true`
/// when the message was removed.
pub async fn filter_links(app: &App, guild: &str, message: &Value) -> bool {
    let config = app.config(guild);
    let filter = &config.lock_url;
    let channel = str_of(message, "channel_id");
    if !filter.enabled || !watched(filter, channel) {
        return false;
    }
    let mut allowed = filter.allowed_domains.clone();
    if filter.x_blocked_channels.iter().any(|id| id == channel) {
        allowed.retain(|domain| !X_DOMAINS.contains(&domain.as_str()));
    }
    let content = str_of(message, "content");
    let Some(domain) = blocked_link(content, &allowed) else {
        return false;
    };
    let author = str_of(&message["author"], "id");
    let roles = roles_of(&message["member"]);
    if filter.exempt_roles.iter().any(|role| roles.contains(role))
        || app.is_staff(guild, author, &roles)
    {
        return false;
    }

    let message_id = str_of(message, "id");
    if let Err(error) = app.api.delete_message(channel, message_id).await {
        tracing::warn!(guild, %error, "deleting a link failed");
        return false;
    }
    if filter.timeout_secs > 0 {
        let reason = "Link posted where links are not allowed";
        antiscam::timeout(app, guild, author, filter.timeout_secs.into(), reason).await;
    }
    let entry =
        format!("**Author:** <@{author}>\n**Channel:** <#{channel}>\n**Domain:** `{domain}`");
    logs::moderation(app, guild, "Link removed", &entry).await;

    let rule = "Ce type de lien n'est pas autorisé ici. / This kind of link is not allowed here.";
    if filter.review_channel.is_none() {
        let text = format!("<@{author}>, your message was removed because it contained a link.");
        let notice = Msg::banner(
            Tone::Warning,
            "Links are not allowed here",
            &text,
            Vec::new(),
        )
        .ping(author);
        notice_in_channel(app, channel, notice).await;
        return true;
    }

    {
        let mut pending = app.pending_links.lock().unwrap();
        pending.retain(|_, p| p.expires > Instant::now());
        pending.insert(
            message_id.to_owned(),
            Pending {
                guild_id: guild.to_owned(),
                user_id: author.to_owned(),
                channel_id: channel.to_owned(),
                content: content.to_owned(),
                expires: Instant::now() + SUBMIT_LIFETIME,
            },
        );
    }
    let button = ui::row(vec![ui::emoji_button(
        ui::STYLE_PRIMARY,
        "📨",
        "Envoyer au staff / Send to staff",
        &format!("{SUBMIT_PREFIX}{message_id}"),
    )]);
    let help = "Tu peux demander au staff de vérifier ton lien. / You can ask the staff to review your link.";

    // 1) A private message; 2) when DMs are closed, a short notice in the channel.
    let dm_text = format!(
        "Ton message dans <#{channel}> a été supprimé. / Your message in <#{channel}> was removed.\n-# {help}"
    );
    let dm = Msg::banner(
        Tone::Warning,
        rule,
        "",
        vec![ui::text(dm_text), button.clone()],
    );
    if let Some(dm_channel) = dm_channel(app, author).await
        && app.api.send_message(&dm_channel, &dm).await.is_ok()
    {
        return true;
    }
    let text = format!("<@{author}>\n-# {help}");
    let notice = Msg::banner(Tone::Warning, rule, "", vec![ui::text(text), button]).ping(author);
    notice_in_channel(app, channel, notice).await;
    true
}

/// Posts a notice in the channel and deletes it after a while.
async fn notice_in_channel(app: &App, channel: &str, notice: Msg) {
    if let Ok(sent) = app.api.send_message(channel, &notice).await {
        let (api, channel, id) = (
            app.api.clone(),
            channel.to_owned(),
            str_of(&sent, "id").to_owned(),
        );
        tokio::spawn(async move {
            tokio::time::sleep(CHANNEL_NOTICE_LIFETIME).await;
            let _ = api.delete_message(&channel, &id).await;
        });
    }
}

fn review_card(review: &db::LinkReview, decision: Option<&str>) -> Msg {
    let quoted: String = review
        .content
        .lines()
        .map(|line| format!("> {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut children = vec![ui::text(format!(
        "**Member:** <@{}>\n**Channel:** <#{}>\n{}",
        review.user_id,
        review.channel_id,
        ui::truncate(&quoted, 1500)
    ))];
    match decision {
        Some(decision) => children.push(ui::text(decision)),
        None => children.push(ui::row(vec![
            ui::emoji_button(
                ui::STYLE_SUCCESS,
                "✅",
                "Accept",
                &format!("{REVIEW_PREFIX}accept:{}", review.id),
            ),
            ui::emoji_button(
                ui::STYLE_DANGER,
                "❌",
                "Refuse",
                &format!("{REVIEW_PREFIX}refuse:{}", review.id),
            ),
        ])),
    }
    let tone = match decision {
        None => Tone::Confirm,
        Some(text) if text.contains("Accepted") => Tone::Success,
        Some(_) => Tone::Error,
    };
    Msg::banner(tone, "Link to review", "", children)
}

/// The member clicked "Send to staff" (in their DMs or in the channel).
pub async fn submit(ctx: &Ctx) {
    let message_id = ctx.custom_id().trim_start_matches(SUBMIT_PREFIX).to_owned();
    // Taken out first, so a double click sends one request only.
    let (pending, not_theirs) = {
        let mut all = ctx.app.pending_links.lock().unwrap();
        match all.get(&message_id) {
            Some(p) if p.user_id != ctx.user_id() => (None, true),
            _ => (
                all.remove(&message_id)
                    .filter(|p| p.expires > Instant::now()),
                false,
            ),
        }
    };
    if not_theirs {
        let text = "Ce bouton n'est pas pour toi. / This button isn't for you.";
        return ctx.reply(Msg::denied_with(text)).await;
    }
    let Some(pending) = pending else {
        let text = "Cette demande a expiré. / This request expired.";
        return ctx
            .reply(Msg::banner(Tone::Warning, "Expired", text, Vec::new()).ephemeral())
            .await;
    };
    let config = ctx.app.config(&pending.guild_id);
    let Some(review_channel) = config.lock_url.review_channel.clone() else {
        return ctx
            .reply(Msg::error("Link reviews are turned off on this server."))
            .await;
    };
    let review = db::LinkReview {
        id: random_token(8),
        guild_id: pending.guild_id.clone(),
        user_id: pending.user_id.clone(),
        channel_id: pending.channel_id.clone(),
        content: pending.content.chars().take(1800).collect(),
    };
    let sent = ctx
        .app
        .api
        .send_message(&review_channel, &review_card(&review, None))
        .await;
    if sent.is_err() || db::save_link_review(&ctx.app.db, &review).await.is_err() {
        ctx.app
            .pending_links
            .lock()
            .unwrap()
            .insert(message_id, pending);
        return ctx
            .reply(Msg::error("The request could not be sent. Try again."))
            .await;
    }
    let text = "Ton lien a été envoyé au staff. / Your link was sent to the staff for review.";
    ctx.update(Msg::banner(Tone::Success, "Sent", text, Vec::new()))
        .await;
}

/// The staff clicked Accept or Refuse.
pub async fn review(ctx: &Ctx) {
    if !ctx
        .app
        .is_staff(&ctx.guild_id, ctx.user_id(), &roles_of(&ctx.member))
    {
        return ctx.reply(Msg::denied()).await;
    }
    let mut parts = ctx.custom_id().trim_start_matches(REVIEW_PREFIX).split(':');
    let (action, id) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    let review = match db::take_link_review(&ctx.app.db, id).await {
        Ok(Some(review)) if review.guild_id == ctx.guild_id => review,
        _ => {
            let text = "Already handled, or no longer available.";
            return ctx
                .reply(Msg::banner(Tone::Info, "Nothing to do", text, Vec::new()).ephemeral())
                .await;
        }
    };
    let accepted = action == "accept";
    if accepted {
        // The original message was deleted: the bot posts it for the member.
        let mut post = Msg::default();
        post.components = vec![ui::text(format!(
            "<@{}> a partagé (lien validé par le staff) / shared (link approved by the staff):\n{}",
            review.user_id,
            ui::truncate(&review.content, 1800)
        ))];
        if let Err(error) = ctx.app.api.send_message(&review.channel_id, &post).await {
            let _ = db::save_link_review(&ctx.app.db, &review).await;
            return ctx.reply(Msg::error(&error.user_message())).await;
        }
    }
    let decision = if accepted {
        format!("✅ **Accepted** by <@{}>", ctx.user_id())
    } else {
        format!("❌ **Refused** by <@{}>", ctx.user_id())
    };
    ctx.update(review_card(&review, Some(&decision))).await;
    // Best effort: the member may have closed their DMs.
    if let Some(dm) = dm_channel(&ctx.app, &review.user_id).await {
        let text = if accepted {
            format!(
                "✅ Ton lien a été validé et publié dans <#{0}>. / Your link was approved and posted in <#{0}>.",
                review.channel_id
            )
        } else {
            format!(
                "❌ Ton lien a été refusé pour <#{0}>. / Your link was refused for <#{0}>.",
                review.channel_id
            )
        };
        let mut msg = Msg::default();
        msg.components = vec![ui::text(text)];
        let _ = ctx.app.api.send_message(&dm, &msg).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dc_core::guild::LockUrl;

    #[test]
    fn scopes() {
        let mut config = LockUrl {
            scope: "all".into(),
            exempt_channels: vec!["1".into()],
            channels: vec!["2".into()],
            ..LockUrl::default()
        };
        assert!(!watched(&config, "1"));
        assert!(watched(&config, "3"));
        config.scope = "listed".into();
        assert!(watched(&config, "2"));
        assert!(!watched(&config, "3"));
    }

    #[test]
    fn review_cards() {
        let review = db::LinkReview {
            id: "abc".into(),
            guild_id: "1".into(),
            user_id: "2".into(),
            channel_id: "3".into(),
            content: "look https://site.example\nline two".into(),
        };
        let open = review_card(&review, None).payload().to_string();
        assert!(open.contains("link-review:accept:abc") && open.contains("> line two"));
        let closed = review_card(&review, Some("✅ **Accepted** by <@9>"))
            .payload()
            .to_string();
        assert!(!closed.contains("link-review:"));
        // Nobody is pinged by a review card.
        assert!(closed.contains("\"users\":[]"));
    }
}
