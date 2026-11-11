//! Giveaways: `/giveaway start|end|reroll|cancel|list`, an Enter button on
//! the giveaway message, automatic draw when the time is up, and rerolls.

use std::{collections::HashSet, sync::Arc, time::Duration};

use dc_core::{cards::Tone, crypto::random_range};

use crate::{
    app::{App, parse_duration, roles_of, str_of},
    db::{self, Giveaway},
    interactions::{Ctx, Options},
    logs,
    ui::{self, Msg},
};

const MIN_SECS: i64 = 10;
const MAX_SECS: i64 = 30 * 86_400;
pub const MAX_WINNERS: i64 = 20;
const MAX_PRIZE: usize = 200;
/// How often ended giveaways are drawn and entry counts refreshed.
const TICK: Duration = Duration::from_secs(5);

// ---------------------------------------------------------------- messages

fn plural(n: i64, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

fn mentions(ids: &[String]) -> String {
    ids.iter()
        .map(|id| format!("<@{id}>"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The giveaway message, running or ended.
pub fn message(giveaway: &Giveaway, entries: i64) -> Msg {
    let footer = ui::text(format!("-# Giveaway #{}", giveaway.number));
    if giveaway.ended {
        let winners = giveaway.winner_list();
        let result = if winners.is_empty() {
            "**Winners:** nobody, there were no valid entries".to_owned()
        } else {
            format!("**Winners:** {}", mentions(&winners))
        };
        let details = format!(
            "{result}\n**Entries:** {entries}\n**Hosted by:** <@{}>\n**Ended:** <t:{}:R>",
            giveaway.host_id, giveaway.ends_at
        );
        let button = ui::disabled(ui::emoji_button(
            ui::STYLE_SECONDARY,
            "🎉",
            &format!("Ended · {entries}"),
            &format!("giveaway:enter:{}", giveaway.number),
        ));
        return Msg::banner(
            Tone::Success,
            &giveaway.prize,
            "This giveaway has ended.",
            vec![
                ui::text(details),
                ui::separator(),
                ui::row(vec![button]),
                footer,
            ],
        );
    }

    let mut details = format!(
        "**Ends:** <t:{0}:R> · <t:{0}:f>\n**Hosted by:** <@{1}>",
        giveaway.ends_at, giveaway.host_id
    );
    if let Some(role) = &giveaway.required_role {
        details.push_str(&format!("\n**Required role:** <@&{role}>"));
    }
    let button = ui::emoji_button(
        ui::STYLE_PRIMARY,
        "🎉",
        &format!("Enter · {entries}"),
        &format!("giveaway:enter:{}", giveaway.number),
    );
    Msg::banner(
        Tone::Gift,
        &giveaway.prize,
        &format!(
            "{}. Click Enter below to take part.",
            plural(giveaway.winners, "winner", "winners")
        ),
        vec![
            ui::text(details),
            ui::separator(),
            ui::row(vec![button]),
            footer,
        ],
    )
}

/// Picks up to `count` different entries at random, leaving out `exclude`.
pub fn draw(entries: &[String], exclude: &[String], count: usize) -> Vec<String> {
    let mut pool: Vec<&String> = entries.iter().filter(|id| !exclude.contains(id)).collect();
    let count = count.min(pool.len());
    // Partial Fisher-Yates: the first `count` places end up random.
    for index in 0..count {
        let pick = random_range(index as u32, pool.len() as u32 - 1) as usize;
        pool.swap(index, pick);
    }
    pool.into_iter().take(count).cloned().collect()
}

fn jump(giveaway: &Giveaway) -> String {
    format!(
        "https://discord.com/channels/{}/{}/{}",
        giveaway.guild_id, giveaway.channel_id, giveaway.message_id
    )
}

async fn refresh(app: &App, giveaway: &Giveaway) {
    let entries = db::count_entries(&app.db, &giveaway.guild_id, giveaway.number)
        .await
        .unwrap_or(0);
    if let Err(error) = app
        .api
        .edit_message(
            &giveaway.channel_id,
            &giveaway.message_id,
            &message(giveaway, entries),
        )
        .await
    {
        tracing::warn!(guild = %giveaway.guild_id, %error, "updating a giveaway failed");
    }
}

/// Posts the winners under the giveaway.
async fn announce(app: &App, giveaway: &Giveaway, winners: &[String], reroll: bool) {
    let msg = if winners.is_empty() {
        let text = format!(
            "Nobody could win **{}**: there were no valid entries.\n[Giveaway #{}]({})",
            ui::escape(&giveaway.prize),
            giveaway.number,
            jump(giveaway)
        );
        Msg::banner(Tone::Warning, "No winner", &text, Vec::new())
    } else {
        let title = if reroll {
            "New winner"
        } else {
            "We have a winner"
        };
        let text = format!(
            "Congratulations {}! You won **{}**.\n[Giveaway #{}]({}) · hosted by <@{}>",
            mentions(winners),
            ui::escape(&giveaway.prize),
            giveaway.number,
            jump(giveaway),
            giveaway.host_id
        );
        let mut msg = Msg::banner(Tone::Gift, title, &text, Vec::new());
        msg.ping_users = winners.to_vec();
        msg
    };
    if let Err(error) = app.api.send_message(&giveaway.channel_id, &msg).await {
        tracing::warn!(guild = %giveaway.guild_id, %error, "announcing giveaway winners failed");
    }
}

/// Draws the winners of a running giveaway. Safe to call twice: only the
/// first call draws.
async fn finish(app: &App, giveaway: &Giveaway) {
    let entries = match db::giveaway_entries(&app.db, &giveaway.guild_id, giveaway.number).await {
        Ok(entries) => entries,
        Err(error) => return tracing::error!(%error, "reading giveaway entries failed"),
    };
    let winners = draw(&entries, &[], giveaway.winners.max(1) as usize);
    match db::finish_giveaway(
        &app.db,
        &giveaway.guild_id,
        giveaway.number,
        &winners,
        false,
    )
    .await
    {
        Ok(true) => {}
        Ok(false) => return,
        Err(error) => return tracing::error!(%error, "ending a giveaway failed"),
    }
    let ended = Giveaway {
        ended: true,
        winner_ids: winners.join(","),
        ..giveaway.clone()
    };
    refresh(app, &ended).await;
    announce(app, &ended, &winners, false).await;
    let body = format!(
        "**Prize:** {}\n**Winners:** {}\n**Entries:** {}",
        ui::escape(&giveaway.prize),
        if winners.is_empty() {
            "none".to_owned()
        } else {
            mentions(&winners)
        },
        entries.len()
    );
    logs::moderation(app, &giveaway.guild_id, "Giveaway ended", &body).await;
}

/// Background task: draws giveaways whose time is up and keeps the entry
/// counts on the buttons up to date.
pub async fn watch(app: Arc<App>) {
    loop {
        tokio::time::sleep(TICK).await;
        match db::due_giveaways(&app.db, dc_core::now()).await {
            Ok(due) => {
                for giveaway in due {
                    finish(&app, &giveaway).await;
                }
            }
            Err(error) => tracing::error!(%error, "reading due giveaways failed"),
        }
        let changed: HashSet<(String, i64)> =
            std::mem::take(&mut *app.giveaways_changed.lock().unwrap());
        for (guild, number) in changed {
            if let Ok(Some(giveaway)) = db::get_giveaway(&app.db, &guild, number).await
                && !giveaway.ended
            {
                refresh(&app, &giveaway).await;
            }
        }
    }
}

// ---------------------------------------------------------------- buttons

/// Enter and Leave buttons, open to every member.
pub async fn component(ctx: &Ctx) {
    let mut parts = ctx.custom_id().split(':').skip(1);
    let action = parts.next().unwrap_or("");
    let Some(number) = parts.next().and_then(|n| n.parse::<i64>().ok()) else {
        return ctx
            .reply(Msg::error("This button is no longer active."))
            .await;
    };
    let (db, guild, user) = (&ctx.app.db, ctx.guild_id.as_str(), ctx.user_id());
    let giveaway = match db::get_giveaway(db, guild, number).await {
        Ok(Some(giveaway)) => giveaway,
        Ok(None) => {
            return ctx
                .reply(Msg::error("This giveaway no longer exists."))
                .await;
        }
        Err(error) => {
            tracing::error!(%error, "reading a giveaway failed");
            return ctx.reply(Msg::error("Try again in a moment.")).await;
        }
    };
    if giveaway.ended {
        let msg = Msg::banner(
            Tone::Warning,
            "Too late",
            "This giveaway has ended.",
            Vec::new(),
        );
        return ctx.reply(msg.ephemeral()).await;
    }

    match action {
        "enter" => {
            if let Some(role) = &giveaway.required_role
                && !roles_of(&ctx.member).contains(role)
            {
                let text = format!("You need the <@&{role}> role to enter this giveaway.");
                let msg = Msg::banner(Tone::Denied, "Role required", &text, Vec::new());
                return ctx.reply(msg.ephemeral()).await;
            }
            match db::enter_giveaway(db, guild, number, user).await {
                Ok(true) => {
                    mark_changed(ctx, number);
                    let text = "Good luck! The winners are drawn when the giveaway ends.";
                    let msg = Msg::banner(Tone::Success, "You are in", text, Vec::new());
                    ctx.reply(msg.ephemeral()).await;
                }
                Ok(false) => {
                    let leave = ui::button(
                        ui::STYLE_DANGER,
                        "Leave the giveaway",
                        &format!("giveaway:leave:{number}"),
                    );
                    let msg = Msg::banner(
                        Tone::Info,
                        "Already entered",
                        "You are already taking part in this giveaway.",
                        vec![ui::row(vec![leave])],
                    );
                    ctx.reply(msg.ephemeral()).await;
                }
                Err(error) => {
                    tracing::error!(%error, "entering a giveaway failed");
                    ctx.reply(Msg::error("Try again in a moment.")).await;
                }
            }
        }
        "leave" => {
            if let Err(error) = db::leave_giveaway(db, guild, number, user).await {
                tracing::error!(%error, "leaving a giveaway failed");
                return ctx.reply(Msg::error("Try again in a moment.")).await;
            }
            mark_changed(ctx, number);
            let text = "You can enter again any time before it ends.";
            ctx.update(Msg::banner(
                Tone::Info,
                "You left the giveaway",
                text,
                Vec::new(),
            ))
            .await;
        }
        _ => {
            ctx.reply(Msg::error("This button is no longer active."))
                .await
        }
    }
}

fn mark_changed(ctx: &Ctx, number: i64) {
    ctx.app
        .giveaways_changed
        .lock()
        .unwrap()
        .insert((ctx.guild_id.clone(), number));
}

// ---------------------------------------------------------------- commands

/// `/giveaway ...` (bot admins).
pub async fn command(ctx: &Ctx, options: &Options) {
    match options.sub.as_deref() {
        Some("start") => start(ctx, options).await,
        Some("end") => end(ctx, options).await,
        Some("reroll") => reroll(ctx, options).await,
        Some("cancel") => cancel(ctx, options).await,
        _ => list(ctx).await,
    }
}

async fn start(ctx: &Ctx, options: &Options) {
    let prize = options.str("prize").unwrap_or("").trim();
    if prize.is_empty() {
        return ctx.reply(Msg::error("Describe the prize.")).await;
    }
    let Some(secs) = options.str("duration").and_then(parse_duration) else {
        return ctx
            .reply(Msg::error("Use a duration such as `30m`, `2h` or `3d`."))
            .await;
    };
    if !(MIN_SECS..=MAX_SECS).contains(&secs) {
        return ctx
            .reply(Msg::error(
                "A giveaway lasts between 10 seconds and 30 days.",
            ))
            .await;
    }
    let winners = options.int("winners").unwrap_or(1).clamp(1, MAX_WINNERS);
    let channel = options.str("channel").unwrap_or(&ctx.channel_id).to_owned();
    let required_role = options
        .str("required_role")
        .filter(|role| *role != ctx.guild_id)
        .map(str::to_owned);

    ctx.defer(true).await;
    let mut giveaway = Giveaway {
        guild_id: ctx.guild_id.clone(),
        number: 0,
        channel_id: channel.clone(),
        message_id: String::new(),
        prize: ui::truncate(prize, MAX_PRIZE),
        winners,
        ends_at: dc_core::now() + secs,
        host_id: ctx.user_id().to_owned(),
        required_role,
        ended: false,
        winner_ids: String::new(),
    };
    let db = &ctx.app.db;
    giveaway.number = match db::create_giveaway(db, &giveaway).await {
        Ok(number) => number,
        Err(error) => {
            tracing::error!(%error, "creating a giveaway failed");
            return ctx
                .reply(Msg::error("The giveaway could not be created."))
                .await;
        }
    };
    match ctx
        .app
        .api
        .send_message(&channel, &message(&giveaway, 0))
        .await
    {
        Ok(sent) => {
            let message_id = str_of(&sent, "id");
            if let Err(error) =
                db::set_giveaway_message(db, &ctx.guild_id, giveaway.number, message_id).await
            {
                tracing::error!(%error, "saving a giveaway failed");
            }
            let text = format!(
                "Giveaway #{} for **{}** is running in <#{channel}>. It ends <t:{}:R>.",
                giveaway.number,
                ui::escape(&giveaway.prize),
                giveaway.ends_at
            );
            ctx.reply(Msg::success("Giveaway started", &text)).await;
            let body = format!(
                "**Prize:** {}\n**Channel:** <#{channel}>\n**By:** <@{}>",
                ui::escape(&giveaway.prize),
                ctx.user_id()
            );
            logs::moderation(&ctx.app, &ctx.guild_id, "Giveaway started", &body).await;
        }
        Err(error) => {
            let _ = db::delete_giveaway(db, &ctx.guild_id, giveaway.number).await;
            ctx.reply(Msg::error(&error.user_message())).await;
        }
    }
}

/// The giveaway named by the `id` option, or an answer saying why not.
async fn find(ctx: &Ctx, options: &Options) -> Option<Giveaway> {
    let number = options.int("id").unwrap_or(0);
    match db::get_giveaway(&ctx.app.db, &ctx.guild_id, number).await {
        Ok(Some(giveaway)) if !giveaway.message_id.is_empty() => Some(giveaway),
        Ok(_) => {
            let text = format!("There is no giveaway #{number}. `/giveaway list` shows them.");
            ctx.reply(Msg::error(&text)).await;
            None
        }
        Err(error) => {
            tracing::error!(%error, "reading a giveaway failed");
            ctx.reply(Msg::error("Try again in a moment.")).await;
            None
        }
    }
}

async fn end(ctx: &Ctx, options: &Options) {
    let Some(giveaway) = find(ctx, options).await else {
        return;
    };
    if giveaway.ended {
        let text = "It has already ended. Use `/giveaway reroll` to draw someone else.";
        return ctx
            .reply(Msg::warning("Already ended", text).ephemeral())
            .await;
    }
    ctx.defer(true).await;
    finish(&ctx.app, &giveaway).await;
    let text = format!(
        "Giveaway #{} has ended and its winners are announced.",
        giveaway.number
    );
    ctx.reply(Msg::success("Giveaway ended", &text)).await;
}

async fn reroll(ctx: &Ctx, options: &Options) {
    let Some(giveaway) = find(ctx, options).await else {
        return;
    };
    if !giveaway.ended {
        let text = "This giveaway is still running. End it first with `/giveaway end`.";
        return ctx
            .reply(Msg::warning("Still running", text).ephemeral())
            .await;
    }
    ctx.defer(true).await;
    let entries = db::giveaway_entries(&ctx.app.db, &ctx.guild_id, giveaway.number)
        .await
        .unwrap_or_default();
    let count = options.int("winners").unwrap_or(1).clamp(1, MAX_WINNERS) as usize;
    // Everyone who already won is left out.
    let winners = draw(&entries, &giveaway.winner_list(), count);
    if winners.is_empty() {
        let text = "Every entrant has already won. There is nobody left to draw.";
        return ctx.reply(Msg::warning("Nobody left", text)).await;
    }
    if let Err(error) =
        db::finish_giveaway(&ctx.app.db, &ctx.guild_id, giveaway.number, &winners, true).await
    {
        tracing::error!(%error, "saving a reroll failed");
        return ctx
            .reply(Msg::error("The reroll could not be saved."))
            .await;
    }
    if let Ok(Some(updated)) = db::get_giveaway(&ctx.app.db, &ctx.guild_id, giveaway.number).await {
        refresh(&ctx.app, &updated).await;
    }
    announce(&ctx.app, &giveaway, &winners, true).await;
    let text = format!(
        "New winner(s) of giveaway #{}: {}",
        giveaway.number,
        mentions(&winners)
    );
    ctx.reply(Msg::success("Rerolled", &text)).await;
    let body = format!(
        "**Prize:** {}\n**New winner(s):** {}\n**By:** <@{}>",
        ui::escape(&giveaway.prize),
        mentions(&winners),
        ctx.user_id()
    );
    logs::moderation(&ctx.app, &ctx.guild_id, "Giveaway rerolled", &body).await;
}

async fn cancel(ctx: &Ctx, options: &Options) {
    let Some(giveaway) = find(ctx, options).await else {
        return;
    };
    ctx.defer(true).await;
    if let Err(error) = db::delete_giveaway(&ctx.app.db, &ctx.guild_id, giveaway.number).await {
        tracing::error!(%error, "deleting a giveaway failed");
        return ctx
            .reply(Msg::error("The giveaway could not be cancelled."))
            .await;
    }
    let notice = Msg::banner(
        Tone::Warning,
        &giveaway.prize,
        "This giveaway was cancelled.",
        vec![ui::text(format!("-# Giveaway #{}", giveaway.number))],
    );
    let _ = ctx
        .app
        .api
        .edit_message(&giveaway.channel_id, &giveaway.message_id, &notice)
        .await;
    let text = format!("Giveaway #{} was cancelled. Nobody wins.", giveaway.number);
    ctx.reply(Msg::success("Giveaway cancelled", &text)).await;
}

async fn list(ctx: &Ctx) {
    let running = db::running_giveaways(&ctx.app.db, &ctx.guild_id)
        .await
        .unwrap_or_default();
    if running.is_empty() {
        let text = "No giveaway is running. Start one with /giveaway start.";
        return ctx
            .reply(Msg::banner(Tone::Info, "Giveaways", text, Vec::new()).ephemeral())
            .await;
    }
    let mut lines = Vec::new();
    for giveaway in running.iter().filter(|g| !g.message_id.is_empty()) {
        let entries = db::count_entries(&ctx.app.db, &ctx.guild_id, giveaway.number)
            .await
            .unwrap_or(0);
        lines.push(format!(
            "**#{}** · [{}]({}) · ends <t:{}:R> · {}",
            giveaway.number,
            ui::escape(&giveaway.prize),
            jump(giveaway),
            giveaway.ends_at,
            plural(entries, "entry", "entries")
        ));
    }
    let msg = Msg::banner(
        Tone::Gift,
        "Running giveaways",
        &lines.join("\n"),
        Vec::new(),
    );
    ctx.reply(msg.ephemeral()).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(ended: bool) -> Giveaway {
        Giveaway {
            guild_id: "1".into(),
            number: 3,
            channel_id: "2".into(),
            message_id: "4".into(),
            prize: "Nitro".into(),
            winners: 2,
            ends_at: 1_800_000_000,
            host_id: "5".into(),
            required_role: Some("6".into()),
            ended,
            winner_ids: if ended { "7,8".into() } else { String::new() },
        }
    }

    #[test]
    fn draws_are_distinct_and_exclude_previous_winners() {
        let entries: Vec<String> = (0..10).map(|i| i.to_string()).collect();
        for _ in 0..50 {
            let winners = draw(&entries, &["3".into()], 4);
            assert_eq!(winners.len(), 4);
            assert!(!winners.contains(&"3".to_owned()));
            let unique: HashSet<_> = winners.iter().collect();
            assert_eq!(unique.len(), 4);
        }
        assert_eq!(draw(&entries, &[], 50).len(), 10);
        assert!(draw(&[], &[], 3).is_empty());
        assert!(draw(&["1".into()], &["1".into()], 1).is_empty());
    }

    #[test]
    fn messages() {
        let running = message(&sample(false), 12).payload().to_string();
        assert!(running.contains("Enter · 12"));
        assert!(running.contains("giveaway:enter:3"));
        assert!(running.contains("<@&6>"));
        let ended = message(&sample(true), 12).payload().to_string();
        assert!(ended.contains("<@7>, <@8>"));
        assert!(ended.contains("\"disabled\":true"));
        // A giveaway message never pings anyone by itself.
        assert!(ended.contains("\"users\":[]"));
    }

    #[test]
    fn winner_lists() {
        assert_eq!(sample(true).winner_list(), vec!["7", "8"]);
        assert!(sample(false).winner_list().is_empty());
    }
}
