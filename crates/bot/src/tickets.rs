//! Ticket system: a panel to open private channels with the staff, with
//! claim, close and a transcript sent to the log channel.

use dc_core::guild::GuildConfig;
use serde_json::{Value, json};

use crate::{
    app::{roles_of, str_of},
    db::{self, Ticket},
    interactions::Ctx,
    lang::Lang,
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
    ui::panel_card(
        &tickets.look,
        &tickets.title,
        vec![
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

pub const FORM_PREFIX: &str = "ticket:form:";

/// May this member open a ticket? Answers them when not.
async fn may_open(ctx: &Ctx, config: &GuildConfig) -> bool {
    let tickets = &config.tickets;
    if !tickets.enabled {
        let text = "The ticket system is turned off.";
        ctx.reply(Msg::warning("Unavailable", text).ephemeral())
            .await;
        return false;
    }
    let open = db::open_tickets(&ctx.app.db, &ctx.guild_id, ctx.user_id())
        .await
        .unwrap_or_default();
    if open.len() >= tickets.max_open as usize {
        let list: Vec<String> = open.iter().map(|id| format!("<#{id}>")).collect();
        let text = format!("You already have an open ticket: {}", list.join(", "));
        ctx.reply(Msg::warning("Ticket already open", &text).ephemeral())
            .await;
        return false;
    }
    true
}

/// Click on "Open a ticket", or a topic picked in the menu.
pub async fn open(ctx: &Ctx) {
    let config = ctx.config();
    let index = ctx.data["values"][0]
        .as_str()
        .and_then(|index| index.parse::<usize>().ok());
    let topic = index
        .and_then(|index| config.tickets.topics.get(index))
        .cloned()
        .unwrap_or_default();
    if !may_open(ctx, &config).await {
        return;
    }
    if config.tickets.questions {
        let lang = form_lang(&config, &topic);
        let questions = questions(lang, &topic);
        let fields: Vec<ui::Field> = questions
            .iter()
            .map(|q| ui::Field {
                id: q.id,
                label: q.label,
                placeholder: q.placeholder,
                max: q.max,
                paragraph: q.paragraph,
                required: q.required,
            })
            .collect();
        let title = if topic.is_empty() {
            lang.pick("Ouvrir un ticket", "Open a ticket").to_owned()
        } else {
            ui::truncate(&topic, 45)
        };
        let id = format!(
            "{FORM_PREFIX}{}",
            index.map_or("x".to_owned(), |i| i.to_string())
        );
        return ctx.modal(ui::form_modal(&id, &title, &fields)).await;
    }
    create(ctx, &config, &topic, None).await;
}

/// The form of a ticket was sent.
pub async fn submitted(ctx: &Ctx) {
    let config = ctx.config();
    let index = ctx
        .custom_id()
        .trim_start_matches(FORM_PREFIX)
        .parse::<usize>()
        .ok();
    let topic = index
        .and_then(|index| config.tickets.topics.get(index))
        .cloned()
        .unwrap_or_default();
    if !may_open(ctx, &config).await {
        return;
    }
    let lang = form_lang(&config, &topic);
    let questions = questions(lang, &topic);
    let answers = answers_text(lang, &questions, |id| {
        crate::interactions::modal_value(&ctx.data, id).to_owned()
    });
    create(ctx, &config, &topic, Some(answers)).await;
}

// ---------------------------------------------------------------- questions

/// A question of the ticket form (five at most).
pub struct Question {
    pub id: &'static str,
    pub label: &'static str,
    pub placeholder: &'static str,
    pub paragraph: bool,
    pub required: bool,
    pub max: u32,
}

/// French for a text with accents or common French words; English
/// otherwise (topic names are too short for the AI's language detection).
fn looks_french(text: &str) -> bool {
    const WORDS: &[&str] = &[
        "le",
        "la",
        "les",
        "un",
        "une",
        "des",
        "de",
        "du",
        "avec",
        "pour",
        "sur",
        "aide",
        "autre",
        "signaler",
        "proposer",
        "partenariat",
        "demande",
        "ton",
        "ta",
        "tes",
        "besoin",
        "ouvre",
        "salon",
    ];
    let text = text.to_lowercase();
    text.contains(['é', 'è', 'ê', 'à', 'ç', 'ù', 'ô', 'î', 'û'])
        || text
            .split(|c: char| !c.is_alphanumeric())
            .any(|word| WORDS.contains(&word))
}

/// Language of the form: the one of the topic (or of the panel's text).
fn form_lang(config: &GuildConfig, topic: &str) -> Lang {
    let text = if topic.is_empty() {
        &config.tickets.text
    } else {
        topic
    };
    if looks_french(text) {
        Lang::Fr
    } else {
        Lang::En
    }
}

fn is_idea(topic: &str) -> bool {
    [
        "feature",
        "fonctionnalit",
        "suggest",
        "idée",
        "idea",
        "propos",
    ]
    .iter()
    .any(|word| topic.contains(word))
}

/// Questions asked for a topic (ported from the Misty bot): a bug, an
/// idea, or a single general question.
pub fn questions(lang: Lang, topic: &str) -> Vec<Question> {
    let q = |id, label, placeholder, paragraph, required, max| Question {
        id,
        label,
        placeholder,
        paragraph,
        required,
        max,
    };
    let topic = topic.to_lowercase();
    if topic.contains("bug") {
        vec![
            q(
                "did",
                lang.pick("Ce que tu as fait", "What you did"),
                lang.pick("1. J'ouvre…\n2. Je clique sur…", "1. I open…\n2. I click…"),
                true,
                true,
                1000,
            ),
            q(
                "happened",
                lang.pick("Ce qui s'est passé", "What happened"),
                lang.pick(
                    "Tu pourras ajouter des captures dans le ticket.",
                    "You can add screenshots in the ticket.",
                ),
                true,
                true,
                1000,
            ),
            q(
                "expected",
                lang.pick("Ce que tu attendais", "What you expected"),
                "",
                true,
                true,
                500,
            ),
            q(
                "always",
                lang.pick("Ça arrive à chaque fois ?", "Does it happen every time?"),
                lang.pick("Oui / Non / Parfois", "Yes / No / Sometimes"),
                false,
                true,
                100,
            ),
            q(
                "crash",
                lang.pick("Code de crash (facultatif)", "Crash code (optional)"),
                "MR-XXXXXXX",
                false,
                false,
                40,
            ),
        ]
    } else if is_idea(&topic) {
        vec![
            q(
                "idea",
                lang.pick("L'idée", "The idea"),
                lang.pick("Pouvoir…", "Being able to…"),
                true,
                true,
                1000,
            ),
            q(
                "why",
                lang.pick("Pourquoi : ce que ça améliore", "Why: what it makes better"),
                "",
                true,
                true,
                1000,
            ),
            q(
                "how",
                lang.pick("Comment ça pourrait marcher", "How it could work"),
                lang.pick("Facultatif.", "Optional."),
                true,
                false,
                1000,
            ),
        ]
    } else {
        vec![q(
            "text",
            lang.pick("Explique ta demande", "Explain your request"),
            lang.pick("Que se passe-t-il ?", "What's going on?"),
            true,
            true,
            1500,
        )]
    }
}

/// The answers as the text posted in the ticket (empty ones left out).
pub fn answers_text(lang: Lang, questions: &[Question], answer: impl Fn(&str) -> String) -> String {
    let mut out = Vec::new();
    for question in questions {
        let text = answer(question.id);
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        let text = ui::truncate(text, 1000);
        if questions.len() == 1 {
            out.push(text);
        } else if question.id == "crash" {
            out.push(format!(
                "**{}** `{}`",
                lang.pick("Code de crash", "Crash code"),
                text.replace('`', "")
            ));
        } else {
            out.push(format!("**{}**\n{text}", question.label));
        }
    }
    out.join("\n\n")
}

/// Creates the ticket channel; `answers`: the form, posted in it.
async fn create(ctx: &Ctx, config: &GuildConfig, topic: &str, answers: Option<String>) {
    let tickets = &config.tickets;
    let topic = topic.to_owned();
    let db = &ctx.app.db;
    let user_id = ctx.user_id();
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
    let welcome = if answers.is_some() {
        format!("Hello <@{user_id}>, the staff will answer here as soon as possible.")
    } else {
        format!(
            "Hello <@{user_id}>, describe your request here. The staff will answer as soon as possible."
        )
    };
    let mut blocks = vec![ui::text(heading), ui::text(welcome)];
    if let Some(answers) = answers.filter(|a| !a.is_empty()) {
        blocks.push(ui::separator());
        blocks.push(ui::text(ui::truncate(&answers, 3000)));
    }
    blocks.push(ui::separator());
    blocks.push(ui::row(vec![
        ui::button(ui::STYLE_SECONDARY, "Claim", "ticket:claim"),
        ui::button(ui::STYLE_DANGER, "Close", "ticket:close"),
    ]));
    let mut intro = Msg::card(ui::WHITE, blocks).ping(user_id);
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
    fn ticket_forms() {
        let bug = questions(Lang::Fr, "Signaler un bug");
        assert_eq!(bug.len(), 5);
        assert!(!bug[4].required);
        assert_eq!(questions(Lang::En, "Suggest a feature").len(), 3);
        assert_eq!(questions(Lang::En, "Other").len(), 1);
        for (topic, french) in [
            ("Signaler un bug", true),
            ("Aide avec le launcher", true),
            ("Report a bug", false),
            ("Launcher help", false),
            ("Suggest a feature", false),
        ] {
            assert_eq!(looks_french(topic), french, "{topic}");
        }
        // Discord limits: 45 characters per label, 100 per placeholder.
        for (lang, topic) in [
            (Lang::Fr, "bug"),
            (Lang::En, "bug"),
            (Lang::Fr, "idée"),
            (Lang::En, "idea"),
            (Lang::En, "x"),
        ] {
            for q in questions(lang, topic) {
                assert!(q.label.chars().count() <= 45 && q.placeholder.chars().count() <= 100);
            }
        }
        let text = answers_text(Lang::Fr, &bug, |id| match id {
            "did" => "Je lance".into(),
            "crash" => "MR-AB12CD3".into(),
            _ => String::new(),
        });
        assert_eq!(
            text,
            "**Ce que tu as fait**
Je lance

**Code de crash** `MR-AB12CD3`"
        );
        let single = questions(Lang::En, "Other");
        assert_eq!(answers_text(Lang::En, &single, |_| " Help ".into()), "Help");
    }

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
