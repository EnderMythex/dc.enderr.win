//! `/announce` and `/maintenance`: announcements and maintenance notices
//! with an image banner.
//!
//! `/announce` opens a form for the title and the (multi-line) text; the
//! channel, mention and picture are chosen in the command itself.

use std::time::{Duration, Instant};

use crate::{
    app::{App, format_duration, parse_duration, str_of},
    db::{self, Panel},
    interactions::{Ctx, Options, modal_value},
    logs,
    ui::{self, Field, Msg},
};
use dc_core::{cards::Tone, crypto::random_token};

/// How long the form of `/announce` may stay open.
const DRAFT_LIFETIME: Duration = Duration::from_secs(900);
/// Largest picture attached to an announcement.
const MAX_IMAGE_BYTES: usize = 8 * 1024 * 1024;
const MAX_TITLE: u32 = 80;
const MAX_TEXT: u32 = 3000;

/// Who an announcement or a maintenance notice mentions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mention {
    Nobody,
    Everyone,
    Here,
    Role(String),
}

/// Discord's "Mention @everyone, @here and All Roles" permission.
const MENTION_EVERYONE: u64 = 1 << 17;

impl Mention {
    /// Refuses mentions the member could not make themselves.
    fn check(&self, ctx: &Ctx) -> Result<(), &'static str> {
        match self {
            Self::Everyone | Self::Here if !ctx.has_permission(MENTION_EVERYONE) => {
                Err("You need Discord's \"Mention @everyone\" permission to mention everyone.")
            }
            _ => Ok(()),
        }
    }

    fn from_options(options: &Options) -> Self {
        match (options.str("mention"), options.str("role")) {
            (Some("everyone"), _) => Self::Everyone,
            (Some("here"), _) => Self::Here,
            (_, Some(role)) => Self::Role(role.to_owned()),
            _ => Self::Nobody,
        }
    }

    fn text(&self) -> Option<String> {
        match self {
            Self::Nobody => None,
            Self::Everyone => Some("@everyone".into()),
            Self::Here => Some("@here".into()),
            Self::Role(role) => Some(format!("<@&{role}>")),
        }
    }

    /// Lets the message actually ping who it mentions.
    fn allow(&self, mut msg: Msg) -> Msg {
        match self {
            Self::Nobody => {}
            Self::Everyone | Self::Here => msg.ping_everyone = true,
            Self::Role(role) => msg.ping_roles.push(role.clone()),
        }
        msg
    }
}

/// An announcement waiting for its form to be filled in.
#[derive(Debug, Clone)]
pub struct Draft {
    channel_id: String,
    mention: Mention,
    /// `(file name, url)` of the attached picture.
    image: Option<(String, String)>,
    author: String,
    expires: Instant,
}

// ---------------------------------------------------------------- announce

/// `/announce`: checks the options, then opens the form.
pub async fn announce(ctx: &Ctx, options: &Options) {
    if let Err(reason) = Mention::from_options(options).check(ctx) {
        return ctx.reply(Msg::denied_with(reason)).await;
    }
    let image = match options.attachment("image") {
        None => None,
        Some(file) => {
            let kind = str_of(file, "content_type");
            let size = file["size"].as_u64().unwrap_or(0) as usize;
            if !kind.starts_with("image/") {
                return ctx
                    .reply(Msg::error("The attachment must be an image."))
                    .await;
            }
            if size > MAX_IMAGE_BYTES {
                return ctx
                    .reply(Msg::error("The picture is too large (8 MB at most)."))
                    .await;
            }
            let extension = kind.trim_start_matches("image/").replace("jpeg", "jpg");
            Some((
                format!("announcement.{extension}"),
                str_of(file, "url").to_owned(),
            ))
        }
    };
    let id = random_token(8);
    {
        let mut drafts = ctx.app.drafts.lock().unwrap();
        drafts.retain(|_, draft| draft.expires > Instant::now());
        drafts.insert(
            id.clone(),
            Draft {
                channel_id: options.str("channel").unwrap_or(&ctx.channel_id).to_owned(),
                mention: Mention::from_options(options),
                image,
                author: ctx.user_id().to_owned(),
                expires: Instant::now() + DRAFT_LIFETIME,
            },
        );
    }
    let fields = [
        Field {
            id: "title",
            label: "Title",
            placeholder: "New event this weekend!",
            max: MAX_TITLE,
            paragraph: false,
            required: true,
        },
        Field {
            id: "text",
            label: "Message",
            placeholder: "Discord formatting works: **bold**, lists, links, mentions.",
            max: MAX_TEXT,
            paragraph: true,
            required: true,
        },
    ];
    ctx.modal(ui::form_modal(
        &format!("announce:{id}"),
        "New announcement",
        &fields,
    ))
    .await;
}

/// Downloads the attached picture: Discord's links to attachments expire,
/// so the announcement carries its own copy.
async fn download(app: &App, url: &str) -> Option<Vec<u8>> {
    let response = app
        .web
        .get(url)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let bytes = response.bytes().await.ok()?;
    (bytes.len() <= MAX_IMAGE_BYTES).then(|| bytes.to_vec())
}

/// The announcement message.
pub fn announcement(
    title: &str,
    text: &str,
    author: &str,
    mention: &Mention,
    image: Option<&str>,
) -> Msg {
    let mut children = Vec::new();
    if let Some(mention) = mention.text() {
        children.push(ui::text(mention));
    }
    children.push(ui::text(text));
    if let Some(file) = image {
        children.push(ui::gallery(&format!("attachment://{file}")));
    }
    children.push(ui::separator());
    children.push(ui::text(format!(
        "-# Posted by <@{author}> · <t:{}:f>",
        dc_core::now()
    )));
    mention.allow(Msg::banner(Tone::Announce, title, "", children))
}

/// The form of `/announce` was sent.
pub async fn submitted(ctx: &Ctx) {
    let id = ctx.custom_id().trim_start_matches("announce:");
    let draft = ctx.app.drafts.lock().unwrap().remove(id);
    let Some(draft) = draft.filter(|d| d.expires > Instant::now() && d.author == ctx.user_id())
    else {
        let text = "This form has expired. Run /announce again.";
        return ctx.reply(Msg::warning("Expired", text).ephemeral()).await;
    };
    if !ctx.is_admin() {
        return ctx.reply(Msg::denied()).await;
    }
    let title = modal_value(&ctx.data, "title").trim().to_owned();
    let text = modal_value(&ctx.data, "text").trim().to_owned();
    ctx.defer(true).await;

    let mut picture = None;
    if let Some((file, url)) = &draft.image {
        match download(&ctx.app, url).await {
            Some(bytes) => picture = Some((file.clone(), bytes)),
            None => {
                return ctx
                    .reply(Msg::error(
                        "The picture could not be downloaded. Try again.",
                    ))
                    .await;
            }
        }
    }
    let file = picture.as_ref().map(|(name, _)| name.as_str());
    let mut msg = announcement(&title, &text, ctx.user_id(), &draft.mention, file);
    if let Some((name, bytes)) = picture {
        msg = msg.file(&name, bytes);
    }

    match ctx.app.api.send_message(&draft.channel_id, &msg).await {
        Ok(sent) => {
            let link = format!(
                "https://discord.com/channels/{}/{}/{}",
                ctx.guild_id,
                draft.channel_id,
                str_of(&sent, "id")
            );
            let done = format!("Posted in <#{}>. [See it]({link})", draft.channel_id);
            ctx.reply(Msg::success("Announcement posted", &done)).await;
            let entry = format!(
                "**Title:** {}\n**Channel:** <#{}>\n**By:** <@{}>",
                ui::escape(&title),
                draft.channel_id,
                ctx.user_id()
            );
            logs::moderation(&ctx.app, &ctx.guild_id, "Announcement posted", &entry).await;
        }
        Err(error) => ctx.reply(Msg::error(&error.user_message())).await,
    }
}

// ---------------------------------------------------------------- maintenance

/// The maintenance notice is remembered like the panels, under this kind.
const MAINTENANCE: &str = "maintenance";

fn maintenance_message(
    reason: &str,
    started: i64,
    until: Option<i64>,
    author: &str,
    mention: &Mention,
) -> Msg {
    let mut children = Vec::new();
    if let Some(mention) = mention.text() {
        children.push(ui::text(mention));
    }
    let mut details = format!("**Started:** <t:{started}:R>");
    if let Some(until) = until {
        details.push_str(&format!(
            "\n**Expected end:** <t:{until}:R> · <t:{until}:t>"
        ));
    }
    children.push(ui::text(details));
    children.push(ui::separator());
    children.push(ui::text(format!(
        "-# Announced by <@{author}>. This message is updated when it is over."
    )));
    mention.allow(Msg::banner(
        Tone::Maintenance,
        "Maintenance in progress",
        reason,
        children,
    ))
}

fn finished_message(text: &str, started: i64, author: &str, mention: &Mention) -> Msg {
    let mut children = Vec::new();
    if let Some(mention) = mention.text() {
        children.push(ui::text(mention));
    }
    let lasted = format_duration((dc_core::now() - started).max(0));
    children.push(ui::text(format!(
        "**Duration:** {lasted}\n**Ended:** <t:{}:R>",
        dc_core::now()
    )));
    children.push(ui::separator());
    children.push(ui::text(format!("-# Announced by <@{author}>")));
    mention.allow(Msg::banner(
        Tone::Success,
        "Maintenance finished",
        text,
        children,
    ))
}

/// `/maintenance start` and `/maintenance end`.
pub async fn maintenance(ctx: &Ctx, options: &Options) {
    let db = &ctx.app.db;
    let current = db::get_panel(db, &ctx.guild_id, MAINTENANCE)
        .await
        .ok()
        .flatten();
    let mention = Mention::from_options(options);
    if let Err(reason) = mention.check(ctx) {
        return ctx.reply(Msg::denied_with(reason)).await;
    }

    if options.sub.as_deref() == Some("end") {
        let Some(notice) = current else {
            let text = "No maintenance is in progress. Start one with /maintenance start.";
            return ctx
                .reply(Msg::warning("Nothing to end", text).ephemeral())
                .await;
        };
        ctx.defer(true).await;
        let started = notice.hash.parse().unwrap_or_else(|_| dc_core::now());
        let text = options
            .str("message")
            .unwrap_or("Everything is back to normal. Thank you for your patience.");
        // The old notice says it is over; a new message tells everyone.
        let quiet = finished_message(text, started, ctx.user_id(), &Mention::Nobody);
        let _ = ctx
            .app
            .api
            .edit_message(&notice.channel_id, &notice.message_id, &quiet)
            .await;
        let loud = finished_message(text, started, ctx.user_id(), &mention);
        let result = ctx.app.api.send_message(&notice.channel_id, &loud).await;
        let _ = db::delete_panel(db, &ctx.guild_id, MAINTENANCE).await;
        return match result {
            Ok(_) => {
                let done = format!(
                    "The end of the maintenance was announced in <#{}>.",
                    notice.channel_id
                );
                ctx.reply(Msg::success("Maintenance finished", &done)).await;
                let entry = format!("**By:** <@{}>", ctx.user_id());
                logs::moderation(&ctx.app, &ctx.guild_id, "Maintenance finished", &entry).await;
            }
            Err(error) => ctx.reply(Msg::error(&error.user_message())).await,
        };
    }

    // start
    if current.is_some() {
        let text = "A maintenance is already in progress. End it with /maintenance end.";
        return ctx
            .reply(Msg::warning("Already started", text).ephemeral())
            .await;
    }
    let until = match options.str("duration") {
        None => None,
        Some(duration) => match parse_duration(duration) {
            Some(secs) if secs <= 30 * 86_400 => Some(dc_core::now() + secs),
            _ => {
                return ctx
                    .reply(Msg::error("Use a duration such as `30m`, `2h` or `1d`."))
                    .await;
            }
        },
    };
    let reason = options
        .str("reason")
        .unwrap_or("We are working on the server. Some things may not work for a while.");
    let channel = options.str("channel").unwrap_or(&ctx.channel_id).to_owned();
    ctx.defer(true).await;
    let started = dc_core::now();
    let msg = maintenance_message(reason, started, until, ctx.user_id(), &mention);
    match ctx.app.api.send_message(&channel, &msg).await {
        Ok(sent) => {
            let notice = Panel {
                channel_id: channel.clone(),
                message_id: str_of(&sent, "id").to_owned(),
                hash: started.to_string(),
            };
            if let Err(error) = db::set_panel(db, &ctx.guild_id, MAINTENANCE, &notice).await {
                tracing::error!(%error, "saving the maintenance notice failed");
            }
            let done = format!(
                "The maintenance was announced in <#{channel}>. Use /maintenance end when it is over."
            );
            ctx.reply(Msg::success("Maintenance started", &done)).await;
            let entry = format!(
                "**Reason:** {}\n**By:** <@{}>",
                ui::escape(reason),
                ctx.user_id()
            );
            logs::moderation(&ctx.app, &ctx.guild_id, "Maintenance started", &entry).await;
        }
        Err(error) => ctx.reply(Msg::error(&error.user_message())).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn announcements() {
        let msg = announcement(
            "Event",
            "**Saturday**",
            "42",
            &Mention::Everyone,
            Some("a.png"),
        );
        let payload = msg.payload();
        let text = payload.to_string();
        assert!(text.contains("@everyone"));
        assert!(text.contains("attachment://a.png"));
        assert!(text.contains("Posted by <@42>"));
        assert_eq!(payload["allowed_mentions"]["parse"][0], "everyone");

        let quiet = announcement("Event", "Hi", "42", &Mention::Nobody, None);
        assert_eq!(
            quiet.payload()["allowed_mentions"]["parse"],
            serde_json::json!([])
        );
        let role = announcement("Event", "Hi", "42", &Mention::Role("7".into()), None);
        assert_eq!(role.payload()["allowed_mentions"]["roles"][0], "7");
    }

    #[test]
    fn maintenance_notices() {
        let running =
            maintenance_message("Database update", 100, Some(200), "42", &Mention::Here).payload();
        let text = running.to_string();
        assert!(text.contains("<t:200:R>") && text.contains("@here"));
        let done = finished_message("Back", 100, "42", &Mention::Nobody)
            .payload()
            .to_string();
        assert!(done.contains("Duration"));
        assert!(!done.contains("@here"));
    }
}
