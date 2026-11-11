//! Automatic anti-scam checks on every message of a non-staff member (ported
//! from the Scanline bot):
//!
//! 1. Discord invite links, even disguised (`di\nsco\nrd。gg/x`): message
//!    deleted, author kicked (1 h timeout if the kick fails).
//! 2. The recurring "join an insider, text me" pitch, compared loosely with
//!    the known text: deleted, author banned.
//! 3. Pictures of "I make you rich, DM me" recruiting ads, read by Gemini:
//!    deleted, author banned; the staff gets the picture with an Unban button.
//!
//! A ban falls back to a kick, then to the longest timeout (28 days). A
//! member banned in the last minute has their next messages deleted at once
//! (the same scam posted in several channels).

use std::{
    collections::{HashMap, HashSet},
    sync::{LazyLock, Mutex},
    time::{Duration, Instant},
};

use dc_core::cards::Tone;
use futures_util::{StreamExt as _, stream::FuturesUnordered};
use regex::Regex;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use unicode_normalization::UnicodeNormalization as _;

use crate::{
    app::{App, iso8601, str_of},
    interactions::Ctx,
    logs,
    ui::{self, Msg},
};

pub const UNBAN_PREFIX: &str = "scam-unban:";
const BAN_PURGE_SECONDS: u64 = 3600;
const MAX_TIMEOUT_SECS: i64 = 28 * 86_400;
const SCANNED_TYPES: [&str; 5] = [
    "image/png",
    "image/jpeg",
    "image/webp",
    "image/heic",
    "image/heif",
];
const MAX_IMAGE_BYTES: u64 = 10 * 1024 * 1024;
const MAX_SIDE: u64 = 1280;
const MAX_HASHES: usize = 10_000;
const RECENT_BAN: Duration = Duration::from_secs(60);

/// What the anti-scam remembers between messages.
#[derive(Default)]
pub struct Memory {
    recently_banned: Mutex<HashMap<(String, String), Instant>>,
    scam_images: Mutex<HashSet<[u8; 32]>>,
    safe_images: Mutex<HashSet<[u8; 32]>>,
}

impl Memory {
    /// `false` when the member was already being banned.
    fn mark_banned(&self, guild: &str, user: &str) -> bool {
        let mut banned = self.recently_banned.lock().unwrap();
        banned.retain(|_, at| at.elapsed() < RECENT_BAN);
        banned
            .insert((guild.to_owned(), user.to_owned()), Instant::now())
            .is_none()
    }

    fn recently_banned(&self, guild: &str, user: &str) -> bool {
        self.recently_banned
            .lock()
            .unwrap()
            .get(&(guild.to_owned(), user.to_owned()))
            .is_some_and(|at| at.elapsed() < RECENT_BAN)
    }
}

fn remember(set: &Mutex<HashSet<[u8; 32]>>, hash: [u8; 32]) {
    let mut set = set.lock().unwrap();
    if set.len() >= MAX_HASHES {
        set.clear();
    }
    set.insert(hash);
}

// ---------------------------------------------------------------- invites

static INVITE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:https?://)?(?:www\.)?(?:discord\.gg|discord(?:app)?\.com/invite)/[a-z0-9-]+",
    )
    .unwrap()
});

fn is_invisible(c: char) -> bool {
    matches!(
        c,
        '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{FEFF}' | '\u{00AD}'
    )
}

/// Undoes the usual disguises: invisible characters, line breaks and
/// formatting inside the word, look-alike dots and letters.
fn unmask(content: &str) -> String {
    content
        .nfkc()
        .filter(|c| !is_invisible(*c))
        .map(|c| match c {
            '。' | '．' | '｡' => '.',
            'ɛ' | 'Ɛ' => 'e',
            'ı' => 'i',
            other => other,
        })
        .filter(|c| !c.is_whitespace() && !matches!(c, '>' | '*' | '_' | '~' | '`' | '|'))
        .collect()
}

pub fn has_invite(content: &str) -> bool {
    !content.is_empty() && (INVITE.is_match(content) || INVITE.is_match(&unmask(content)))
}

// ---------------------------------------------------------------- insider pitch

const INSIDER_REFERENCE: &str = "Anyone new to crypto and don't know how to trade I will suggest you to join an insider \
were you get early coins to Ape in and profit text me Note Only if you are serious and \
wanna get good at trading meme coins and being profitable";
const INSIDER_ANCHORS: [&str; 7] = [
    "new to crypto",
    "don't know how to trade",
    "insider",
    "early coins",
    "ape in",
    "text me",
    "get good at trading",
];
const EXACT_SCAMS: [&str; 1] = [
    "perso je grind encore pas mal les airdrops sur solana, mais comme dit plus haut c'est fini la meta farm sybil avec 30% du supply lol",
];

fn normalize(text: &str) -> String {
    let mapped: String = text
        .nfkd()
        .filter(|c| !('\u{0300}'..='\u{036F}').contains(c))
        .flat_map(char::to_lowercase)
        .filter(|c| !is_invisible(*c))
        .map(|c| match c {
            '\'' | '’' | '‘' | '`' | '´' => '\'',
            c if c.is_ascii_lowercase() || c.is_ascii_digit() => c,
            _ => ' ',
        })
        .collect();
    mapped.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn words(normalized: &str) -> HashSet<String> {
    normalized
        .split(' ')
        .filter(|w| w.chars().count() > 2)
        .map(str::to_owned)
        .collect()
}

struct Reference {
    words: HashSet<String>,
    anchors: Vec<String>,
    exact: Vec<String>,
}

static REFERENCE: LazyLock<Reference> = LazyLock::new(|| Reference {
    words: words(&normalize(INSIDER_REFERENCE)),
    anchors: INSIDER_ANCHORS.iter().map(|p| normalize(p)).collect(),
    exact: EXACT_SCAMS.iter().map(|p| normalize(p)).collect(),
});

/// A loose comparison with the known pitch: scammers reword it a little,
/// while single keywords would catch real conversations.
pub fn is_insider_pitch(content: &str) -> bool {
    if content.chars().count() < 40 {
        return false;
    }
    let reference = &*REFERENCE;
    let text = normalize(content);
    if reference
        .exact
        .iter()
        .any(|phrase| text.contains(phrase.as_str()))
    {
        return true;
    }
    let anchors = reference
        .anchors
        .iter()
        .filter(|p| text.contains(p.as_str()))
        .count();
    if anchors < 4 {
        return false;
    }
    let mine = words(&text);
    let shared = mine.intersection(&reference.words).count();
    let union = mine.len() + reference.words.len() - shared;
    union > 0 && shared as f64 / union as f64 >= 0.45
}

// ---------------------------------------------------------------- sanctions

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sanction {
    Banned,
    Kicked,
    TimedOut,
    Failed,
}

impl Sanction {
    fn describe(self) -> &'static str {
        match self {
            Self::Banned => "banned",
            Self::Kicked => "kicked (the ban failed)",
            Self::TimedOut => "timed out for 28 days (ban and kick failed)",
            Self::Failed => "NOT sanctioned: check my permissions and role position",
        }
    }
}

pub async fn timeout(app: &App, guild: &str, user: &str, secs: i64, reason: &str) -> bool {
    let until = iso8601(dc_core::now() + secs.min(MAX_TIMEOUT_SECS));
    let body = json!({ "communication_disabled_until": until });
    app.api
        .patch(
            &format!("/guilds/{guild}/members/{user}"),
            &body,
            Some(reason),
        )
        .await
        .map_err(|error| tracing::warn!(guild, %error, "timeout failed"))
        .is_ok()
}

/// Ban, else kick, else the longest timeout.
async fn ban_with_fallback(app: &App, guild: &str, user: &str, reason: &str) -> Sanction {
    let body = json!({ "delete_message_seconds": BAN_PURGE_SECONDS });
    if app
        .api
        .put(&format!("/guilds/{guild}/bans/{user}"), &body, Some(reason))
        .await
        .is_ok()
    {
        return Sanction::Banned;
    }
    if app
        .api
        .delete(&format!("/guilds/{guild}/members/{user}"), Some(reason))
        .await
        .is_ok()
    {
        return Sanction::Kicked;
    }
    if timeout(app, guild, user, MAX_TIMEOUT_SECS, reason).await {
        Sanction::TimedOut
    } else {
        Sanction::Failed
    }
}

async fn delete(app: &App, message: &Value) {
    let _ = app
        .api
        .delete_message(str_of(message, "channel_id"), str_of(message, "id"))
        .await;
}

/// Deletes the message and bans its author at the same time. `None` when the
/// author was already being banned (same scam in several channels).
async fn delete_and_ban(app: &App, guild: &str, message: &Value, reason: &str) -> Option<Sanction> {
    let user = str_of(&message["author"], "id");
    if !app.anti_scam.mark_banned(guild, user) {
        delete(app, message).await;
        return None;
    }
    let (_, sanction) = tokio::join!(
        delete(app, message),
        ban_with_fallback(app, guild, user, reason)
    );
    Some(sanction)
}

/// Tells the staff channel what happened (and the moderation log).
async fn report(
    app: &App,
    guild: &str,
    message: &Value,
    what: &str,
    sanction: Sanction,
    picture: Option<(&str, &[u8], &str)>,
) {
    let author = &message["author"];
    let user = str_of(author, "id");
    let caption: String = str_of(message, "content")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(300)
        .collect();
    let mut text = format!(
        "**Member:** <@{user}> · `{}`\n**Channel:** <#{}>\n**Result:** {}",
        str_of(author, "username"),
        str_of(message, "channel_id"),
        sanction.describe()
    );
    if !caption.is_empty() {
        text.push_str(&format!("\n**Message:** {}", ui::escape(&caption)));
    }
    logs::moderation(app, guild, what, &text).await;

    let config = app.config(guild);
    let Some(channel) = &config.anti_scam.staff_channel else {
        return;
    };
    let tone = if sanction == Sanction::Banned {
        Tone::Success
    } else {
        Tone::Warning
    };
    let mut extra = vec![ui::text(text)];
    let mut files = Vec::new();
    if let Some((name, bytes, hash)) = picture {
        extra.push(ui::gallery(&format!("attachment://{name}")));
        files.push((name.to_owned(), bytes.to_vec()));
        if sanction == Sanction::Banned {
            extra.push(ui::row(vec![ui::button(
                ui::STYLE_SECONDARY,
                "Unban (false positive)",
                &format!("{UNBAN_PREFIX}{user}:{hash}"),
            )]));
        }
    }
    let mut msg = Msg::banner(tone, what, "", extra);
    for (name, bytes) in files {
        msg = msg.file(&name, bytes);
    }
    if let Err(error) = app.api.send_message(channel, &msg).await {
        tracing::warn!(guild, %error, "anti-scam report failed");
    }
}

// ---------------------------------------------------------------- pictures

struct Picture {
    url: String,
    fallback: String,
    mime: String,
    name: String,
}

/// Pictures of the message, forwarded ones included.
fn pictures(message: &Value) -> Vec<Picture> {
    let snapshots = message["message_snapshots"]
        .as_array()
        .into_iter()
        .flatten();
    let forwarded =
        snapshots.flat_map(|s| s["message"]["attachments"].as_array().into_iter().flatten());
    message["attachments"]
        .as_array()
        .into_iter()
        .flatten()
        .chain(forwarded)
        .filter_map(|file| {
            let mime = str_of(file, "content_type")
                .split(';')
                .next()?
                .trim()
                .to_owned();
            if !SCANNED_TYPES.contains(&mime.as_str())
                || file["size"].as_u64().unwrap_or(0) > MAX_IMAGE_BYTES
            {
                return None;
            }
            let (width, height) = (
                file["width"].as_u64().unwrap_or(0),
                file["height"].as_u64().unwrap_or(0),
            );
            let proxy = str_of(file, "proxy_url").to_owned();
            let url = if width > MAX_SIDE || height > MAX_SIDE {
                // Discord's media proxy shrinks it: faster to fetch and to read.
                let scale = MAX_SIDE as f64 / width.max(height) as f64;
                let separator = if proxy.contains('?') { '&' } else { '?' };
                format!(
                    "{proxy}{separator}width={}&height={}",
                    ((width as f64) * scale).round().max(1.0),
                    ((height as f64) * scale).round().max(1.0)
                )
            } else {
                str_of(file, "url").to_owned()
            };
            Some(Picture {
                url,
                fallback: proxy,
                mime,
                name: str_of(file, "filename").to_owned(),
            })
        })
        .collect()
}

async fn download(app: &App, picture: &Picture) -> Option<Vec<u8>> {
    for url in [&picture.url, &picture.fallback] {
        let Ok(response) = app
            .web
            .get(url)
            .timeout(Duration::from_secs(10))
            .send()
            .await
        else {
            continue;
        };
        if !response.status().is_success()
            || response
                .content_length()
                .is_some_and(|len| len > MAX_IMAGE_BYTES)
        {
            continue;
        }
        if let Ok(bytes) = response.bytes().await {
            return Some(bytes.to_vec());
        }
    }
    None
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn parse_hex(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (index, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(text.get(index * 2..index * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

/// The first picture judged a scam: `(bytes, file name, hash)`.
async fn scam_picture(app: &App, message: &Value) -> Option<(Vec<u8>, String, [u8; 32])> {
    if !app.gemini.enabled() {
        return None;
    }
    let caption = str_of(message, "content");
    let mut checks: FuturesUnordered<_> = pictures(message)
        .into_iter()
        .map(|picture| async move {
            let bytes = download(app, &picture).await?;
            let hash: [u8; 32] = Sha256::digest(&bytes).into();
            let memory = &app.anti_scam;
            if memory.scam_images.lock().unwrap().contains(&hash) {
                return Some((bytes, picture.name, hash));
            }
            if memory.safe_images.lock().unwrap().contains(&hash) {
                return None;
            }
            match app
                .gemini
                .is_scam_image(&bytes, &picture.mime, caption)
                .await
            {
                Ok(true) => {
                    remember(&memory.scam_images, hash);
                    Some((bytes, picture.name, hash))
                }
                Ok(false) => {
                    // The text can change the verdict: only a bare picture is remembered.
                    if caption.trim().is_empty() {
                        remember(&memory.safe_images, hash);
                    }
                    None
                }
                Err(error) => {
                    tracing::warn!("checking a picture failed: {error:#}");
                    None
                }
            }
        })
        .collect();
    while let Some(result) = checks.next().await {
        if result.is_some() {
            return result;
        }
    }
    None
}

// ---------------------------------------------------------------- entry points

/// Runs every enabled check. `true` when the message was removed.
pub async fn check(app: &App, guild: &str, message: &Value, is_edit: bool) -> bool {
    let config = app.config(guild);
    let rules = &config.anti_scam;
    if !(rules.invites || rules.insider || rules.images) {
        return false;
    }
    let user = str_of(&message["author"], "id");
    if app.anti_scam.recently_banned(guild, user) {
        delete(app, message).await;
        return true;
    }
    if app.is_staff(guild, user, &crate::app::roles_of(&message["member"])) {
        return false;
    }
    let content = str_of(message, "content");

    if rules.invites && has_invite(content) {
        let reason = "Discord invite link posted by a member";
        let path = format!("/guilds/{guild}/members/{user}");
        let kick = app.api.delete(&path, Some(reason));
        let (_, kicked) = tokio::join!(delete(app, message), kick);
        let sanction = if kicked.is_ok() {
            Sanction::Kicked
        } else if timeout(app, guild, user, 3600, reason).await {
            Sanction::TimedOut
        } else {
            Sanction::Failed
        };
        let entry = format!(
            "**Member:** <@{user}>\n**Channel:** <#{}>\n**Result:** {}",
            str_of(message, "channel_id"),
            match sanction {
                Sanction::Kicked => "kicked",
                Sanction::TimedOut => "timed out for 1 hour (the kick failed)",
                _ => "not sanctioned: check my permissions",
            }
        );
        logs::moderation(app, guild, "Invite link removed", &entry).await;
        return true;
    }

    if rules.insider && is_insider_pitch(content) {
        if let Some(sanction) =
            delete_and_ban(app, guild, message, "Known scam message posted by a member").await
        {
            report(app, guild, message, "Scam message removed", sanction, None).await;
        }
        return true;
    }

    if rules.images
        && !is_edit
        && let Some((bytes, name, hash)) = scam_picture(app, message).await
    {
        if let Some(sanction) =
            delete_and_ban(app, guild, message, "Scam picture detected automatically").await
        {
            let hash = hex(&hash);
            report(
                app,
                guild,
                message,
                "Scam picture removed",
                sanction,
                Some((&name, &bytes, &hash)),
            )
            .await;
        }
        return true;
    }
    false
}

/// The Unban button under a scam picture (staff only).
pub async fn unban(ctx: &Ctx) {
    if !ctx.is_admin()
        && !ctx.app.is_staff(
            &ctx.guild_id,
            ctx.user_id(),
            &crate::app::roles_of(&ctx.member),
        )
    {
        return ctx.reply(Msg::denied()).await;
    }
    let mut parts = ctx.custom_id().trim_start_matches(UNBAN_PREFIX).split(':');
    let Some(user) = parts
        .next()
        .filter(|id| dc_core::is_snowflake(id))
        .map(str::to_owned)
    else {
        return ctx
            .reply(Msg::error("This button is no longer active."))
            .await;
    };
    // A false positive: that picture must not trigger a ban again.
    if let Some(hash) = parts.next().and_then(parse_hex) {
        ctx.app.anti_scam.scam_images.lock().unwrap().remove(&hash);
        remember(&ctx.app.anti_scam.safe_images, hash);
    }
    let reason = format!(
        "Anti-scam false positive, unbanned by {}",
        str_of(&ctx.user, "username")
    );
    let path = format!("/guilds/{}/bans/{user}", ctx.guild_id);
    match ctx.app.api.delete(&path, Some(&reason)).await {
        Ok(_) => {}
        Err(error) if error.is_not_found() => {}
        Err(error) => return ctx.reply(Msg::error(&error.user_message())).await,
    }
    let text = format!("<@{user}> was unbanned by <@{}>.", ctx.user_id());
    ctx.update(Msg::banner(Tone::Info, "Unbanned", &text, Vec::new()))
        .await;
    logs::moderation(&ctx.app, &ctx.guild_id, "Anti-scam ban lifted", &text).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invites_even_disguised() {
        assert!(has_invite("rejoins https://discord.gg/abc123"));
        assert!(has_invite("discord.com/invite/xyz"));
        assert!(has_invite("> di\n> sco\n> rd。gg/free-nitro"));
        assert!(has_invite("disc\u{200B}ord.gg/abc"));
        assert!(!has_invite("j'adore discord, gg à tous"));
    }

    #[test]
    fn insider_pitch_and_variants() {
        assert!(is_insider_pitch(INSIDER_REFERENCE));
        assert!(is_insider_pitch(
            "Anyone new to crypto & don’t know how to trade?? I will suggest u to join an insider were you get \
             early coins to Ape in and profit, text me. Note: only if you are serious and wanna get good at \
             trading meme coins and being profitable 🚀"
        ));
        assert!(is_insider_pitch(
            "Perso je grind encore pas mal les airdrops sur Solana, mais comme dit plus haut c'est fini la méta \
             farm sybil avec 30% du supply lol"
        ));
        assert!(!is_insider_pitch(
            "I'm new to crypto and don't know how to trade, any tips for a beginner on axiom?"
        ));
        assert!(!is_insider_pitch(
            "the dev is an insider, I aped in early and now I'm down 50%"
        ));
    }

    #[test]
    fn pictures_of_a_message() {
        let message = json!({
            "attachments": [
                { "content_type": "image/png", "size": 10, "url": "u1", "proxy_url": "p1", "width": 100, "height": 100, "filename": "a.png" },
                { "content_type": "image/png", "size": 10, "url": "u2", "proxy_url": "https://p2", "width": 4000, "height": 2000, "filename": "big.png" },
                { "content_type": "video/mp4", "size": 10, "url": "u3", "proxy_url": "p3", "filename": "v.mp4" }
            ],
            "message_snapshots": [{ "message": { "attachments": [
                { "content_type": "image/jpeg; charset=x", "size": 10, "url": "u4", "proxy_url": "p4", "width": 10, "height": 10, "filename": "f.jpg" }
            ] } }]
        });
        let found = pictures(&message);
        assert_eq!(found.len(), 3);
        assert_eq!(found[0].url, "u1");
        assert_eq!(found[1].url, "https://p2?width=1280&height=640");
        assert_eq!(found[2].mime, "image/jpeg");
    }

    #[test]
    fn hashes_round_trip() {
        let hash: [u8; 32] = Sha256::digest(b"scam").into();
        assert_eq!(parse_hex(&hex(&hash)), Some(hash));
        assert!(format!("{UNBAN_PREFIX}{}:{}", u64::MAX, hex(&hash)).len() <= 100);
    }
}
