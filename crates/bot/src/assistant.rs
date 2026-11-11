//! The AI assistant: answers members who mention the bot, with the server's
//! context (or the bot's default one). Ported from the Scanline bot.

use std::{
    sync::LazyLock,
    time::{Duration, Instant},
};

use regex::Regex;
use serde_json::{Value, json};

use crate::{
    app::{App, str_of},
    gemini::Intent,
    lang, scanline,
    ui::{self, Msg},
};

static ANY_MENTION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<@!?\d+>").unwrap());

/// A plain-text answer, as a reply to the member's message.
fn answer(text: &str, reply_to: &str) -> Msg {
    let mut msg = Msg::default();
    msg.components = vec![ui::text(ui::truncate(text, 1900))];
    msg.reply_to(reply_to)
}

/// Seconds left before this member may ask again, if any.
fn cooldown_left(app: &App, guild: &str, user: &str, cooldown: Duration) -> Option<u64> {
    let mut last = app.assistant_cooldowns.lock().unwrap();
    let key = (guild.to_owned(), user.to_owned());
    if let Some(at) = last.get(&key)
        && at.elapsed() < cooldown
    {
        let left = cooldown - at.elapsed();
        return Some(left.as_secs() + u64::from(left.subsec_nanos() > 0));
    }
    if last.len() > 10_000 {
        last.retain(|_, at| at.elapsed() < cooldown);
    }
    last.insert(key, Instant::now());
    None
}

/// Called for messages of members (after the moderation checks).
pub async fn on_message(app: &App, guild: &str, message: &Value) {
    let config = app.config(guild);
    let settings = &config.assistant;
    let content = str_of(message, "content");
    let me = app.user_id();
    // Only an explicit @mention typed in the message, not a reply to the bot.
    let mentioned = !me.is_empty()
        && (content.contains(&format!("<@{me}>")) || content.contains(&format!("<@!{me}>")));
    if !settings.enabled || !mentioned || !app.gemini.enabled() {
        return;
    }
    let channel = str_of(message, "channel_id");
    if !settings.channels.is_empty() && !settings.channels.iter().any(|id| id == channel) {
        return;
    }
    let question = ANY_MENTION.replace_all(content, "").trim().to_owned();
    if question.is_empty() {
        return;
    }
    let message_id = str_of(message, "id");
    let lang = lang::detect(&question);
    let send = async |msg: Msg| {
        if let Err(error) = app.api.send_message(channel, &msg).await {
            tracing::warn!(guild, %error, "answering a mention failed");
        }
    };

    let cooldown = Duration::from_secs(settings.cooldown_secs.into());
    if let Some(left) = cooldown_left(app, guild, str_of(&message["author"], "id"), cooldown) {
        let text = lang.pick(
            format!("Doucement 😅 réessaie dans {left}s."),
            format!("Slow down 😅 try again in {left}s."),
        );
        return send(answer(&text, message_id)).await;
    }
    let length = question.chars().count();
    if length > settings.max_length as usize {
        let text = lang.pick(
            format!(
                "Ton message est trop long ({length} caractères), essaie de le raccourcir stp 🙏"
            ),
            format!("Your message is too long ({length} characters), please try to shorten it 🙏"),
        );
        return send(answer(&text, message_id)).await;
    }

    // One answer at a time, in order: the moderation never waits for this.
    let _turn = app.assistant_queue.lock().await;
    let has_pack = app.packs.iter().any(|pack| pack == scanline::PACK);
    if has_pack && scanline::may_want_files(&question) {
        let files = match app.gemini.intent(&question).await {
            Intent::Filters => Some(scanline::filters_prompt(lang)),
            Intent::WalletTracker => Some(scanline::wallet_tracker(app, lang)),
            Intent::None => None,
        };
        if let Some(msg) = files {
            return send(msg.reply_to(message_id)).await;
        }
    }

    let _ = app
        .api
        .post(&format!("/channels/{channel}/typing"), &json!({}))
        .await;
    let context = if settings.context.is_empty() {
        scanline::default_context(app)
    } else {
        settings.context.clone()
    };
    let text = match app.gemini.ask(&context, &question).await {
        Ok(text) => text,
        Err(error) => {
            tracing::warn!(guild, "AI answer failed: {error:#}");
            lang.pick(
                "Désolé, je n'ai pas pu répondre pour le moment.",
                "Sorry, I couldn't answer right now.",
            )
            .to_owned()
        }
    };
    send(answer(&text, message_id)).await;
}
