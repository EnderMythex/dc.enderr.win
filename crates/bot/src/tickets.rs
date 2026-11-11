//! Ticket system: a panel to open private channels with the staff, with
//! claim, close and a transcript sent to the log channel.

use dc_core::guild::GuildConfig;
use serde_json::{Value, json};

use crate::{
    app::{roles_of, str_of},
    db::{self, Ticket},
    interactions::Ctx,
    ui::{self, Msg},
};

const VIEW_CHANNEL: u64 = 1 << 10;
const SEND_MESSAGES: u64 = 1 << 11;
const EMBED_LINKS: u64 = 1 << 14;
const ATTACH_FILES: u64 = 1 << 15;
const READ_HISTORY: u64 = 1 << 16;
const MANAGE_CHANNELS: u64 = 1 << 4;
const PARTICIPANT: u64 = VIEW_CHANNEL | SEND_MESSAGES | EMBED_LINKS | ATTACH_FILES | READ_HISTORY;

/// Messages kept in a transcript (10 pages of 100).
const TRANSCRIPT_PAGES: usize = 10;

/// The panel posted in the ticket channel.
pub fn panel(config: &GuildConfig) -> Msg {
    let tickets = &config.tickets;
    let opener = if tickets.topics.is_empty() {
        ui::button(ui::STYLE_PRIMARY, "Open a ticket", "ticket:open")
    } else {
        ui::select(
            "ticket:topic",
            "Choose a topic to open a ticket",
            &tickets.topics,
        )
    };
    Msg::card(
        ui::WHITE,
        vec![
            ui::text(format!("# {}", tickets.title)),
            ui::text(tickets.text.clone()),
            ui::separator(),
            ui::row(vec![opener]),
        ],
    )
}

fn is_staff(ctx: &Ctx, config: &GuildConfig) -> bool {
    let roles = roles_of(&ctx.member);
    config
        .tickets
        .staff_roles
        .iter()
        .any(|role| roles.contains(role))
        || ctx.is_admin()
}

/// Click on "Open a ticket", or a topic picked in the menu.
pub async fn open(ctx: &Ctx) {
    let config = ctx.config();
    let tickets = &config.tickets;
    if !tickets.enabled {
        let text = "The ticket system is turned off.";
        return ctx
            .reply(Msg::warning("Unavailable", text).ephemeral())
            .await;
    }
    let topic = ctx.data["values"][0]
        .as_str()
        .and_then(|index| index.parse::<usize>().ok())
        .and_then(|index| tickets.topics.get(index))
        .cloned()
        .unwrap_or_default();

    let db = &ctx.app.db;
    let user_id = ctx.user_id();
    let open = db::open_tickets(db, &ctx.guild_id, user_id)
        .await
        .unwrap_or_default();
    if open.len() >= tickets.max_open as usize {
        let list: Vec<String> = open.iter().map(|id| format!("<#{id}>")).collect();
        let text = format!("You already have an open ticket: {}", list.join(", "));
        return ctx
            .reply(Msg::warning("Ticket already open", &text).ephemeral())
            .await;
    }

    ctx.defer(true).await;
    let number = match db::next_ticket_number(db, &ctx.guild_id).await {
        Ok(number) => number,
        Err(error) => {
            tracing::error!(%error, "numbering a ticket failed");
            return ctx
                .reply(Msg::error("The ticket could not be created."))
                .await;
        }
    };

    // Private channel: hidden from everyone but the member, the staff and the bot.
    let allow = PARTICIPANT.to_string();
    let mut overwrites = vec![
        json!({ "id": ctx.guild_id, "type": 0, "allow": "0", "deny": VIEW_CHANNEL.to_string() }),
        json!({ "id": user_id, "type": 1, "allow": allow, "deny": "0" }),
        json!({ "id": ctx.app.user_id(), "type": 1, "allow": (PARTICIPANT | MANAGE_CHANNELS).to_string(), "deny": "0" }),
    ];
    for role in &tickets.staff_roles {
        overwrites.push(json!({ "id": role, "type": 0, "allow": allow, "deny": "0" }));
    }
    let mut body = json!({
        "name": format!("ticket-{number:04}"),
        "type": 0,
        "topic": format!("Ticket #{number:04} opened by {}", str_of(&ctx.user, "username")),
        "permission_overwrites": overwrites,
    });
    if let Some(category) = &tickets.category {
        body["parent_id"] = json!(category);
    }
    let channel = match ctx
        .app
        .api
        .post(&format!("/guilds/{}/channels", ctx.guild_id), &body)
        .await
    {
        Ok(channel) => channel,
        Err(error) => {
            tracing::warn!(guild = %ctx.guild_id, %error, "creating a ticket channel failed");
            return ctx.reply(Msg::error(&error.user_message())).await;
        }
    };
    let channel_id = str_of(&channel, "id").to_owned();

    let ticket = Ticket {
        channel_id: channel_id.clone(),
        user_id: user_id.to_owned(),
        number,
        topic: topic.clone(),
        claimed_by: None,
        opened_at: dc_core::now(),
    };
    if let Err(error) = db::insert_ticket(db, &ctx.guild_id, &ticket).await {
        tracing::error!(%error, "saving a ticket failed");
    }

    let heading = if topic.is_empty() {
        format!("### Ticket #{number:04}")
    } else {
        format!("### Ticket #{number:04} · {}", ui::escape(&topic))
    };
    let staff: Vec<String> = tickets
        .staff_roles
        .iter()
        .map(|role| format!("<@&{role}>"))
        .collect();
    let mut intro = Msg::card(
        ui::WHITE,
        vec![
            ui::text(heading),
            ui::text(format!(
                "Hello <@{user_id}>, describe your request here. The staff will answer as soon as possible."
            )),
            ui::separator(),
            ui::row(vec![
                ui::button(ui::STYLE_SECONDARY, "Claim", "ticket:claim"),
                ui::button(ui::STYLE_DANGER, "Close", "ticket:close"),
            ]),
        ],
    )
    .ping(user_id);
    if !staff.is_empty() {
        intro
            .components
            .push(ui::text(format!("-# {}", staff.join(" "))));
        intro.ping_roles = tickets.staff_roles.clone();
    }
    if let Err(error) = ctx.app.api.send_message(&channel_id, &intro).await {
        tracing::warn!(%error, "posting in a new ticket failed");
    }

    let text = format!("Your ticket is ready: <#{channel_id}>");
    ctx.reply(Msg::success("Ticket opened", &text)).await;
}

/// Loads the ticket of the current channel, or answers that there is none.
async fn current(ctx: &Ctx) -> Option<Ticket> {
    match db::get_ticket(&ctx.app.db, &ctx.channel_id).await {
        Ok(Some(ticket)) => Some(ticket),
        _ => {
            ctx.reply(Msg::error("This channel is not an open ticket."))
                .await;
            None
        }
    }
}

pub async fn claim(ctx: &Ctx) {
    let Some(ticket) = current(ctx).await else {
        return;
    };
    if !is_staff(ctx, &ctx.config()) {
        let text = "Only the staff can claim a ticket.";
        return ctx
            .reply(Msg::notice(ui::RED, "Not allowed", text).ephemeral())
            .await;
    }
    match db::claim_ticket(&ctx.app.db, &ticket.channel_id, ctx.user_id()).await {
        Ok(true) => {
            let text = format!("<@{}> is taking care of this ticket.", ctx.user_id());
            ctx.reply(Msg::success("Ticket claimed", &text)).await;
        }
        _ => {
            let by = ticket
                .claimed_by
                .map_or("someone".to_owned(), |id| format!("<@{id}>"));
            let text = format!("This ticket is already handled by {by}.");
            ctx.reply(Msg::warning("Already claimed", &text).ephemeral())
                .await;
        }
    }
}

fn may_close(ctx: &Ctx, ticket: &Ticket) -> bool {
    ticket.user_id == ctx.user_id() || is_staff(ctx, &ctx.config())
}

/// Click on "Close": asks for confirmation.
pub async fn ask_close(ctx: &Ctx) {
    let Some(ticket) = current(ctx).await else {
        return;
    };
    if !may_close(ctx, &ticket) {
        return ctx.reply(Msg::denied()).await;
    }
    let msg = Msg::banner(
        dc_core::cards::Tone::Confirm,
        "Close this ticket?",
        "The channel will be deleted. A transcript is kept for the staff.",
        vec![ui::row(vec![
            ui::button(ui::STYLE_DANGER, "Close the ticket", "ticket:close:yes"),
            ui::button(ui::STYLE_SECONDARY, "Cancel", "ticket:close:no"),
        ])],
    );
    ctx.reply(msg).await;
}

pub async fn cancel_close(ctx: &Ctx) {
    ctx.update(Msg::notice(ui::GREY, "Cancelled", "The ticket stays open."))
        .await;
}

/// Confirmed: saves the transcript, then deletes the channel.
pub async fn close(ctx: &Ctx) {
    let Some(ticket) = current(ctx).await else {
        return;
    };
    if !may_close(ctx, &ticket) {
        return ctx.reply(Msg::denied()).await;
    }
    ctx.update(Msg::notice(ui::GREY, "Closing…", "Saving the transcript."))
        .await;

    let config = ctx.config();
    let api = &ctx.app.api;
    if let Some(log_channel) = &config.tickets.log_channel {
        let (transcript, count) = transcript(ctx, &ticket).await;
        let mut lines = vec![
            format!("**Opened by:** <@{}>", ticket.user_id),
            format!("**Closed by:** <@{}>", ctx.user_id()),
            format!("**Opened:** <t:{}:f>", ticket.opened_at),
            format!("**Messages:** {count}"),
        ];
        if !ticket.topic.is_empty() {
            lines.insert(0, format!("**Topic:** {}", ui::escape(&ticket.topic)));
        }
        if let Some(staff) = &ticket.claimed_by {
            lines.push(format!("**Handled by:** <@{staff}>"));
        }
        let file_name = format!("ticket-{:04}.txt", ticket.number);
        let msg = Msg::card(
            ui::WHITE,
            vec![
                ui::text(format!("### Ticket #{:04} closed", ticket.number)),
                ui::text(lines.join("\n")),
                ui::separator(),
                json!({ "type": 13, "file": { "url": format!("attachment://{file_name}") } }),
            ],
        )
        .file(&file_name, transcript.into_bytes());
        if let Err(error) = api.send_message(log_channel, &msg).await {
            tracing::warn!(guild = %ctx.guild_id, %error, "sending a transcript failed");
        }
    }

    let reason = format!("Ticket closed by {}", str_of(&ctx.user, "username"));
    match api
        .delete(&format!("/channels/{}", ticket.channel_id), Some(&reason))
        .await
    {
        Ok(_) => {
            let _ = db::delete_ticket(&ctx.app.db, &ticket.channel_id).await;
        }
        Err(error) => {
            tracing::warn!(guild = %ctx.guild_id, %error, "deleting a ticket channel failed");
            let msg = Msg::error(&error.user_message());
            let _ = api.send_message(&ticket.channel_id, &msg).await;
        }
    }
}

/// Text transcript of the ticket, oldest message first, and its length.
async fn transcript(ctx: &Ctx, ticket: &Ticket) -> (String, usize) {
    let mut messages: Vec<Value> = Vec::new();
    let mut before = String::new();
    for _ in 0..TRANSCRIPT_PAGES {
        let mut path = format!("/channels/{}/messages?limit=100", ticket.channel_id);
        if !before.is_empty() {
            path.push_str(&format!("&before={before}"));
        }
        let Ok(Value::Array(page)) = ctx.app.api.get(&path).await else {
            break;
        };
        let Some(last) = page.last() else { break };
        before = str_of(last, "id").to_owned();
        let full = page.len() == 100;
        messages.extend(page);
        if !full {
            break;
        }
    }
    messages.reverse();

    let mut out = format!("Ticket #{:04}\n", ticket.number);
    if !ticket.topic.is_empty() {
        out.push_str(&format!("Topic: {}\n", ticket.topic));
    }
    out.push('\n');
    let mut count = 0;
    for message in &messages {
        let line = transcript_line(message);
        if !line.is_empty() {
            out.push_str(&line);
            out.push('\n');
            count += 1;
        }
    }
    (out, count)
}

/// One message as `[date time] author: text`, with attachment links.
fn transcript_line(message: &Value) -> String {
    let content = str_of(message, "content");
    let attachments: Vec<&str> = message["attachments"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|attachment| attachment["url"].as_str())
        .collect();
    // The bot's own cards have no plain text: leave them out.
    if content.is_empty() && attachments.is_empty() {
        return String::new();
    }
    let time = str_of(message, "timestamp")
        .get(..19)
        .unwrap_or("")
        .replace('T', " ");
    let mut line = format!(
        "[{time}] {}: {content}",
        str_of(&message["author"], "username")
    );
    for url in attachments {
        line.push_str(&format!("\n    [attachment] {url}"));
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_offers_button_or_topics() {
        let mut config = GuildConfig::default();
        assert!(panel(&config).payload().to_string().contains("ticket:open"));
        config.tickets.topics = vec!["Billing".into(), "Report".into()];
        let payload = panel(&config).payload().to_string();
        assert!(payload.contains("ticket:topic") && payload.contains("Billing"));
    }

    #[test]
    fn transcript_lines() {
        let message = json!({
            "content": "hello",
            "timestamp": "2026-11-11T10:12:33.123000+00:00",
            "author": { "username": "ender" },
            "attachments": [{ "url": "https://cdn.example/a.png" }]
        });
        assert_eq!(
            transcript_line(&message),
            "[2026-11-11 10:12:33] ender: hello\n    [attachment] https://cdn.example/a.png"
        );
        let card = json!({ "content": "", "author": { "username": "bot" }, "attachments": [] });
        assert_eq!(transcript_line(&card), "");
    }
}
