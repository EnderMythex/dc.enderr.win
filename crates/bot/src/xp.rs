//! Levels: XP for chatting, level-up announcements, role rewards, the rank
//! card and the leaderboard.

use std::time::{Duration, Instant};

use dc_core::{
    cards::{self, LeaderboardRow, Tone},
    crypto::random_range,
    render,
};
use serde_json::Value;

use crate::{
    app::{App, display_name, str_of},
    db,
    interactions::{Ctx, Options},
    ui::{self, Msg},
};

const LEADERBOARD_SIZE: i64 = 10;
const MAX_ADJUSTMENT: i64 = 1_000_000;

/// XP needed to go from `level` to the next one.
pub fn xp_for_level(level: u64) -> u64 {
    5 * level * level + 50 * level + 100
}

/// Splits a total into `(level, xp inside that level, xp the level needs)`.
pub fn progress(total: u64) -> (u64, u64, u64) {
    let mut level = 0;
    let mut remaining = total;
    loop {
        let needed = xp_for_level(level);
        if remaining < needed {
            return (level, remaining, needed);
        }
        remaining -= needed;
        level += 1;
    }
}

/// Called for every message of a member: maybe grants XP.
pub async fn on_message(app: &App, guild_id: &str, message: &Value) {
    let config = app.config(guild_id);
    let xp = &config.xp;
    let channel_id = str_of(message, "channel_id");
    if !xp.enabled || xp.ignored_channels.iter().any(|id| id == channel_id) {
        return;
    }
    let author = &message["author"];
    let user_id = str_of(author, "id");

    // One reward per cooldown, so spamming does not pay.
    {
        let mut cooldowns = app.xp_cooldowns.lock().unwrap();
        let key = (guild_id.to_owned(), user_id.to_owned());
        let cooldown = Duration::from_secs(xp.cooldown_secs.into());
        if cooldowns
            .get(&key)
            .is_some_and(|last| last.elapsed() < cooldown)
        {
            return;
        }
        cooldowns.insert(key, Instant::now());
        if cooldowns.len() > 50_000 {
            cooldowns.retain(|_, last| last.elapsed() < cooldown);
        }
    }

    let amount = i64::from(random_range(xp.min_xp, xp.max_xp));
    let name = display_name(author, &message["member"]);
    let (before, after) = match db::add_message_xp(&app.db, guild_id, user_id, &name, amount).await
    {
        Ok(totals) => totals,
        Err(error) => return tracing::error!(%error, "saving XP failed"),
    };
    let old_level = progress(before.max(0) as u64).0;
    let new_level = progress(after.max(0) as u64).0;
    if new_level <= old_level {
        return;
    }

    let mut earned_roles = Vec::new();
    for reward in &xp.rewards {
        let level = u64::from(reward.level);
        if level > old_level && level <= new_level {
            match app
                .api
                .add_role(guild_id, user_id, &reward.role, "Level reward")
                .await
            {
                Ok(_) => earned_roles.push(format!("<@&{}>", reward.role)),
                Err(error) => {
                    tracing::warn!(guild = guild_id, %error, "giving a level reward failed")
                }
            }
        }
    }

    let target = match xp.announce.as_str() {
        "same" => Some(channel_id),
        "channel" => xp.announce_channel.as_deref(),
        _ => None,
    };
    let Some(target) = target else { return };
    let msg = level_up(author, new_level, &earned_roles);
    if let Err(error) = app.api.send_message(target, &msg).await {
        tracing::warn!(guild = guild_id, %error, "announcing a level-up failed");
    }
}

/// The level-up announcement. `rewards` are role mentions.
pub fn level_up(user: &Value, level: u64, rewards: &[String]) -> Msg {
    let user_id = str_of(user, "id");
    let mut text = format!("### Level up!\n<@{user_id}> reached **level {level}**.");
    if !rewards.is_empty() {
        text.push_str(&format!("\n**Reward:** {}", rewards.join(", ")));
    }
    let avatar = crate::app::avatar_url(user, 128);
    Msg::card(ui::WHITE, vec![ui::section_thumb(text, &avatar)]).ping(user_id)
}

fn disabled() -> Msg {
    Msg::banner(
        Tone::Warning,
        "Levels are off",
        "The XP system is not enabled on this server.",
        Vec::new(),
    )
    .ephemeral()
}

/// `/rank`: the rank card of a member, as an image.
pub async fn rank(ctx: &Ctx, options: &Options) {
    if !ctx.config().xp.enabled {
        return ctx.reply(disabled()).await;
    }
    let (user, member) = match options.user("user") {
        Some(user) => (user.clone(), options.member("user").clone()),
        None => (ctx.user.clone(), ctx.member.clone()),
    };
    if user["bot"] == true {
        return ctx.reply(Msg::error("Bots do not earn XP.")).await;
    }
    ctx.defer(false).await;

    let user_id = str_of(&user, "id");
    let total = match db::get_xp(&ctx.app.db, &ctx.guild_id, user_id).await {
        Ok(row) => row.map_or(0, |row| row.xp.max(0)),
        Err(error) => {
            tracing::error!(%error, "reading XP failed");
            return ctx.reply(Msg::error("The rank could not be loaded.")).await;
        }
    };
    let position = db::rank_of(&ctx.app.db, &ctx.guild_id, total)
        .await
        .unwrap_or(1);
    let (level, in_level, for_level) = progress(total as u64);
    let name = display_name(&user, &member);
    let picture = ctx.app.avatar_png(&user).await;

    let image = tokio::task::spawn_blocking(move || {
        render::png(&render::rank(&render::RankCard {
            name: &name,
            rank: position,
            level,
            xp_in_level: in_level,
            xp_for_level: for_level,
            total_xp: total as u64,
            picture: picture.as_deref(),
        }))
    })
    .await;

    match image {
        Ok(Ok(png)) => {
            let msg = Msg::card(ui::WHITE, vec![ui::gallery("attachment://rank.png")])
                .file("rank.png", png);
            ctx.reply(msg).await;
        }
        other => {
            tracing::error!(?other, "rendering the rank card failed");
            // Still answer, without the picture.
            let text = format!(
                "<@{user_id}> · **Rank #{position}** · Level {level}\n{} / {} XP",
                ui::number(in_level),
                ui::number(for_level)
            );
            ctx.reply(Msg::notice(ui::WHITE, "Rank", &text)).await;
        }
    }
}

/// `/leaderboard`: the ten most active members.
pub async fn leaderboard(ctx: &Ctx) {
    if !ctx.config().xp.enabled {
        return ctx.reply(disabled()).await;
    }
    let db = &ctx.app.db;
    let rows = match db::top_xp(db, &ctx.guild_id, LEADERBOARD_SIZE).await {
        Ok(rows) => rows,
        Err(error) => {
            tracing::error!(%error, "reading the leaderboard failed");
            return ctx
                .reply(Msg::error("The leaderboard could not be loaded."))
                .await;
        }
    };
    if rows.is_empty() {
        let msg = Msg::banner(
            Tone::Info,
            "Leaderboard",
            "Nobody has earned XP yet. Start chatting!",
            Vec::new(),
        );
        return ctx.reply(msg).await;
    }

    let guild = ctx.app.guild(&ctx.guild_id);
    let image_rows: Vec<LeaderboardRow> = rows
        .iter()
        .map(|row| {
            let total = row.xp.max(0) as u64;
            LeaderboardRow {
                name: if row.username.is_empty() {
                    format!(
                        "Member {}",
                        &row.user_id[row.user_id.len().saturating_sub(4)..]
                    )
                } else {
                    row.username.clone()
                },
                level: progress(total).0,
                xp: total,
            }
        })
        .collect();
    let svg = cards::leaderboard(&guild.name, &image_rows);

    let lines: Vec<String> = rows
        .iter()
        .enumerate()
        .map(|(index, row)| {
            let medal = match index {
                0 => "🥇".to_owned(),
                1 => "🥈".to_owned(),
                2 => "🥉".to_owned(),
                n => format!("`#{}`", n + 1),
            };
            let total = row.xp.max(0) as u64;
            format!(
                "{medal} <@{}> · Level **{}** · {} XP",
                row.user_id,
                progress(total).0,
                ui::number(total)
            )
        })
        .collect();

    let own = db::get_xp(db, &ctx.guild_id, ctx.user_id())
        .await
        .ok()
        .flatten();
    let footer = match own {
        Some(row) if row.xp > 0 => {
            let position = db::rank_of(db, &ctx.guild_id, row.xp).await.unwrap_or(0);
            format!(
                "-# You are #{position} with {} XP.",
                ui::number(row.xp as u64)
            )
        }
        _ => "-# You have no XP yet.".to_owned(),
    };
    // The image shows the names; the mentions below are clickable.
    let fallback = vec![ui::text(format!(
        "### Leaderboard · {}\n{}",
        ui::escape(&guild.name),
        lines.join("\n")
    ))];
    let msg = Msg::with_image(
        ui::WHITE,
        "leaderboard.png",
        svg,
        vec![ui::separator(), ui::text(footer)],
        fallback,
    );
    ctx.defer(false).await;
    ctx.reply(msg).await;
}

/// `/xp add`, `/xp remove`, `/xp reset` (admins).
pub async fn manage(ctx: &Ctx, options: &Options) {
    let Some(user) = options.user("user") else {
        return ctx.reply(Msg::error("Unknown user.")).await;
    };
    if user["bot"] == true {
        return ctx.reply(Msg::error("Bots do not earn XP.")).await;
    }
    let user_id = str_of(user, "id");
    let db = &ctx.app.db;
    let current = match db::get_xp(db, &ctx.guild_id, user_id).await {
        Ok(row) => row.map_or(0, |row| row.xp),
        Err(error) => {
            tracing::error!(%error, "reading XP failed");
            return ctx.reply(Msg::error("The XP could not be loaded.")).await;
        }
    };
    let amount = options.int("amount").unwrap_or(0).clamp(0, MAX_ADJUSTMENT);
    let new_total = match options.sub.as_deref() {
        Some("add") => current.saturating_add(amount),
        Some("remove") => (current - amount).max(0),
        _ => 0,
    };
    if let Err(error) = db::set_xp(db, &ctx.guild_id, user_id, new_total).await {
        tracing::error!(%error, "saving XP failed");
        return ctx.reply(Msg::error("The XP could not be saved.")).await;
    }
    let text = format!(
        "<@{user_id}> now has **{} XP** (level {}).",
        ui::number(new_total as u64),
        progress(new_total as u64).0
    );
    ctx.reply(Msg::success("XP updated", &text).ephemeral())
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels() {
        assert_eq!(xp_for_level(0), 100);
        assert_eq!(progress(0), (0, 0, 100));
        assert_eq!(progress(99), (0, 99, 100));
        assert_eq!(progress(100), (1, 0, 155));
        assert_eq!(progress(255), (2, 0, 220));
        // Level boundaries are consistent whatever the total.
        let total = 123_456;
        let (level, inside, needed) = progress(total);
        let below: u64 = (0..level).map(xp_for_level).sum();
        assert_eq!(below + inside, total);
        assert!(inside < needed);
    }
}
