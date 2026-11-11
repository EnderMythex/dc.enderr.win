//! `/stats` (arrivals of members) and `/missing-role` (members without a
//! role), ported from the Scanline bot. Discord keeps no history of members
//! who left: the numbers are based on the members still on the server.

use dc_core::cards::Tone;
use serde_json::Value;

use crate::{
    app::{roles_of, str_of},
    interactions::{Ctx, Options},
    massrole::list_members,
    ui::{self, Msg},
};

/// Inline list limit; longer lists are sent as a file.
const MAX_INLINE: usize = 40;

/// Days since 1970-01-01 of a civil date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// `2024-03-01T12:34:56.789+00:00` -> Unix seconds (the time of day matters
/// for "today"; the offset is always UTC from Discord).
pub fn parse_timestamp(text: &str) -> Option<i64> {
    let date = text.get(..10)?;
    let mut parts = date.split('-');
    let (y, m, d): (i64, i64, i64) = (
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    );
    let time = text.get(11..19).unwrap_or("00:00:00");
    let mut clock = time.split(':').map(|v| v.parse::<i64>().unwrap_or(0));
    let (h, min, s) = (clock.next()?, clock.next()?, clock.next()?);
    Some(days_from_civil(y, m, d) * 86_400 + h * 3600 + min * 60 + s)
}

pub fn ymd(secs: i64) -> String {
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    format!("{y:04}-{m:02}-{d:02}")
}

#[derive(Debug, PartialEq, Eq)]
pub struct Arrivals {
    pub total: usize,
    pub today: usize,
    pub yesterday: usize,
    pub this_month: usize,
    pub last_month: usize,
}

pub fn arrivals(joined: &[Option<i64>], now: i64) -> Arrivals {
    const DAY: i64 = 86_400;
    let today = now.div_euclid(DAY);
    let start_today = today * DAY;
    let (y, m, _) = civil_from_days(today);
    let start_month = days_from_civil(y, m, 1) * DAY;
    let (ly, lm) = if m == 1 { (y - 1, 12) } else { (y, m - 1) };
    let start_last_month = days_from_civil(ly, lm, 1) * DAY;
    let count = |from: i64, to: i64| {
        joined
            .iter()
            .flatten()
            .filter(|&&t| t >= from && t < to)
            .count()
    };
    Arrivals {
        total: joined.len(),
        today: count(start_today, start_today + DAY),
        yesterday: count(start_today - DAY, start_today),
        this_month: count(start_month, now + 1),
        last_month: count(start_last_month, start_month),
    }
}

pub fn change(current: usize, previous: usize) -> String {
    if previous == 0 {
        return if current == 0 {
            "±0%".into()
        } else {
            "+∞%".into()
        };
    }
    let pct = (current as f64 - previous as f64) / previous as f64 * 100.0;
    format!("{}{pct:.0}%", if pct > 0.0 { "+" } else { "" })
}

fn humans(members: &[Value]) -> impl Iterator<Item = &Value> {
    members.iter().filter(|m| m["user"]["bot"] != true)
}

/// `/stats` (bot admins).
pub async fn stats(ctx: &Ctx) {
    ctx.defer(true).await;
    let members = match list_members(&ctx.app, &ctx.guild_id).await {
        Ok(members) => members,
        Err(error) => return ctx.reply(Msg::error(&error.user_message())).await,
    };
    let joined: Vec<Option<i64>> = humans(&members)
        .map(|m| parse_timestamp(str_of(m, "joined_at")))
        .collect();
    let a = arrivals(&joined, dc_core::now());
    let text = format!(
        "**Members:** {}\n**Today:** {} (yesterday {}) · {}\n**This month:** {} (last month {}) · {}\n-# Based on the members still here: arrivals of people who left are not counted.",
        ui::number(a.total as u64),
        a.today,
        a.yesterday,
        change(a.today, a.yesterday),
        a.this_month,
        a.last_month,
        change(a.this_month, a.last_month),
    );
    ctx.reply(Msg::banner(
        Tone::Info,
        "Server arrivals",
        "",
        vec![ui::text(text)],
    ))
    .await;
}

/// `/missing-role` (bot admins): members without a role.
pub async fn missing_role(ctx: &Ctx, options: &Options) {
    let Some(role) = options.str("role").map(str::to_owned) else {
        return ctx.reply(Msg::error("Pick a role.")).await;
    };
    ctx.defer(true).await;
    let members = match list_members(&ctx.app, &ctx.guild_id).await {
        Ok(members) => members,
        Err(error) => return ctx.reply(Msg::error(&error.user_message())).await,
    };
    let mut missing: Vec<&Value> = humans(&members)
        .filter(|m| !roles_of(m).contains(&role))
        .collect();
    missing.sort_by_key(|m| parse_timestamp(str_of(m, "joined_at")).unwrap_or(0));
    if missing.is_empty() {
        let text = format!("Every member has <@&{role}>.");
        return ctx.reply(Msg::success("Nobody is missing it", &text)).await;
    }
    let header = format!("**{}** member(s) without <@&{role}>:", missing.len());
    if missing.len() <= MAX_INLINE {
        let lines: Vec<String> = missing
            .iter()
            .map(|m| {
                format!(
                    "- <@{}> · `{}`",
                    str_of(&m["user"], "id"),
                    str_of(&m["user"], "username")
                )
            })
            .collect();
        let text = format!("{header}\n{}", lines.join("\n"));
        return ctx
            .reply(Msg::banner(
                Tone::Info,
                "Members without the role",
                "",
                vec![ui::text(text)],
            ))
            .await;
    }
    let file: String = missing
        .iter()
        .map(|m| {
            let joined =
                parse_timestamp(str_of(m, "joined_at")).map_or_else(|| "?".to_owned(), ymd);
            format!(
                "{}\t{}\tjoined {joined}\n",
                str_of(&m["user"], "username"),
                str_of(&m["user"], "id")
            )
        })
        .collect();
    let blocks = vec![
        ui::text(format!(
            "{header}\n-# Full list in the file: name, id, arrival date."
        )),
        ui::file("attachment://members-without-role.txt"),
    ];
    let msg = Msg::banner(Tone::Info, "Members without the role", "", blocks)
        .file("members-without-role.txt", file.into_bytes());
    ctx.reply(msg).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(days_from_civil(2026, 11, 2), 20_759);
        assert_eq!(ymd(1_793_620_800), "2026-11-02");
        assert_eq!(
            parse_timestamp("1970-01-02T01:00:05.123000+00:00"),
            Some(86_400 + 3605)
        );
        assert_eq!(parse_timestamp("bad"), None);
    }

    #[test]
    fn counts_arrivals() {
        let now = days_from_civil(2026, 3, 15) * 86_400 + 3600;
        let day = |y, m, d| Some(days_from_civil(y, m, d) * 86_400 + 60);
        let joined = [
            day(2026, 3, 15),
            day(2026, 3, 14),
            day(2026, 3, 1),
            day(2026, 2, 10),
            None,
        ];
        assert_eq!(
            arrivals(&joined, now),
            Arrivals {
                total: 5,
                today: 1,
                yesterday: 1,
                this_month: 3,
                last_month: 1
            }
        );
        assert_eq!(change(3, 1), "+200%");
        assert_eq!(change(0, 0), "±0%");
    }
}
