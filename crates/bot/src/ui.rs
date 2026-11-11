//! Components V2 building blocks. Every message the bot sends is a `Msg`
//! made of containers, text, sections, separators, galleries and buttons.
//!
//! Notices (success, warning, error, access denied, confirmations) carry an
//! image banner, drawn by `Msg::finish` right before the message is sent.

use std::{
    collections::HashMap,
    sync::{LazyLock, Mutex, OnceLock},
};

use dc_core::{cards, cards::Tone, crypto::sha256_hex, render};
use serde_json::{Value, json};

/// Message flag: the message uses Components V2 (no `content`, no embeds).
const FLAG_COMPONENTS_V2: u64 = 1 << 15;
const FLAG_EPHEMERAL: u64 = 1 << 6;

pub const WHITE: u32 = 0xEDEDED;
pub const GREEN: u32 = 0x3ECF8E;
pub const AMBER: u32 = 0xF5A524;
pub const RED: u32 = 0xFF5C5C;
pub const GREY: u32 = 0x4A4A4A;
pub const GOLD: u32 = 0xF5C542;
pub const BLUE: u32 = 0x7AA2FF;

/// Discord allows 4000 characters of text per message; stay under it.
const TEXT_LIMIT: usize = 3500;

pub const STYLE_PRIMARY: u8 = 1;
pub const STYLE_SECONDARY: u8 = 2;
pub const STYLE_SUCCESS: u8 = 3;
pub const STYLE_DANGER: u8 = 4;

#[derive(Debug, Clone, Default)]
pub struct Msg {
    pub components: Vec<Value>,
    pub files: Vec<(String, Vec<u8>)>,
    pub ephemeral: bool,
    /// User ids the message is allowed to ping. Nothing else ever pings.
    pub ping_users: Vec<String>,
    /// Role ids the message is allowed to ping.
    pub ping_roles: Vec<String>,
    /// Allows @everyone and @here (announcements chosen by an admin only).
    pub ping_everyone: bool,
    /// Image still to draw; see `finish`.
    pending: Option<PendingImage>,
}

/// An image the first container shows, drawn just before sending.
#[derive(Debug, Clone)]
struct PendingImage {
    file: String,
    svg: String,
    /// Replaces the first container if the image cannot be drawn.
    fallback: Value,
}

static BOT_NAME: OnceLock<String> = OnceLock::new();

/// The bot's name, written on every banner.
pub fn set_bot_name(name: &str) {
    let _ = BOT_NAME.set(name.to_owned());
}

fn bot_name() -> &'static str {
    BOT_NAME.get().map_or("Bot", String::as_str)
}

fn tone_color(tone: Tone) -> u32 {
    match tone {
        Tone::Success => GREEN,
        Tone::Warning | Tone::Confirm => AMBER,
        Tone::Error | Tone::Denied => RED,
        Tone::Info => WHITE,
        Tone::Gift => GOLD,
        Tone::Announce => BLUE,
        Tone::Maintenance => AMBER,
    }
}

/// Text that reads the same drawn on an image: no markdown, mentions or links.
fn is_plain(text: &str) -> bool {
    text.chars().count() <= 200
        && !text.contains("http")
        && !text.chars().any(|c| {
            matches!(
                c,
                '*' | '_' | '~' | '`' | '|' | '<' | '>' | '[' | ']' | '#' | '\n'
            )
        })
}

/// Rendered images by SVG hash: the same notices come back often.
static IMAGE_CACHE: LazyLock<Mutex<HashMap<String, Vec<u8>>>> = LazyLock::new(Mutex::default);
const IMAGE_CACHE_SIZE: usize = 64;

async fn draw(svg: String) -> Option<Vec<u8>> {
    let key = sha256_hex(&svg);
    if let Some(png) = IMAGE_CACHE.lock().unwrap().get(&key) {
        return Some(png.clone());
    }
    let png = tokio::task::spawn_blocking(move || render::png(&svg))
        .await
        .ok()?
        .map_err(|error| tracing::error!(%error, "drawing an image failed"))
        .ok()?;
    let mut cache = IMAGE_CACHE.lock().unwrap();
    if cache.len() >= IMAGE_CACHE_SIZE {
        cache.clear();
    }
    cache.insert(key, png.clone());
    Some(png)
}

impl Msg {
    /// A single container (the rounded card with a colored edge).
    pub fn card(color: u32, children: Vec<Value>) -> Self {
        Self {
            components: vec![container(color, children)],
            ..Self::default()
        }
    }

    pub fn success(title: &str, body: &str) -> Self {
        Self::notice(GREEN, title, body)
    }

    pub fn warning(title: &str, body: &str) -> Self {
        Self::notice(AMBER, title, body)
    }

    pub fn error(body: &str) -> Self {
        Self::notice(RED, "Something went wrong", body).ephemeral()
    }

    pub fn denied() -> Self {
        Self::denied_with("This command is reserved for the administrators of this bot.")
    }

    pub fn denied_with(reason: &str) -> Self {
        Self::banner(Tone::Denied, "Access denied", reason, Vec::new()).ephemeral()
    }

    pub fn notice(color: u32, title: &str, body: &str) -> Self {
        let tone = match color {
            GREEN => Tone::Success,
            AMBER => Tone::Warning,
            RED => Tone::Error,
            _ => Tone::Info,
        };
        Self::banner(tone, title, body, Vec::new())
    }

    /// A card topped by an image banner. A plain `body` is drawn on the
    /// image; one with formatting or mentions is shown as text below it, so
    /// Discord can render it. `extra` follows (buttons, more text).
    pub fn banner(tone: Tone, title: &str, body: &str, extra: Vec<Value>) -> Self {
        let plain = is_plain(body);
        let svg = cards::banner(tone, bot_name(), title, if plain { body } else { "" });
        let mut children = Vec::new();
        if !plain && !body.is_empty() {
            children.push(text(body));
        }
        children.extend(extra);
        let mut fallback = vec![text(format!("### {title}"))];
        if !body.is_empty() {
            fallback.push(text(body));
        }
        Self::with_image(tone_color(tone), "banner.png", svg, children, fallback)
    }

    /// A card whose first block is an image drawn from `svg` when sent,
    /// followed by `children`. `fallback` replaces the image if drawing fails.
    pub fn with_image(
        color: u32,
        file: &str,
        svg: String,
        children: Vec<Value>,
        fallback: Vec<Value>,
    ) -> Self {
        let mut shown = vec![gallery(&format!("attachment://{file}"))];
        shown.extend(children.iter().cloned());
        let mut replacement = fallback;
        replacement.extend(children);
        Self {
            components: vec![container(color, shown)],
            pending: Some(PendingImage {
                file: file.to_owned(),
                svg,
                fallback: container(color, replacement),
            }),
            ..Self::default()
        }
    }

    /// Draws the pending image, if any. Called right before sending.
    pub async fn finish(mut self) -> Self {
        let Some(pending) = self.pending.take() else {
            return self;
        };
        match draw(pending.svg).await {
            Some(png) => self.files.insert(0, (pending.file, png)),
            None => self.components[0] = pending.fallback,
        }
        self
    }

    pub fn ephemeral(mut self) -> Self {
        self.ephemeral = true;
        self
    }

    pub fn ping(mut self, user_id: &str) -> Self {
        self.ping_users.push(user_id.to_owned());
        self
    }

    pub fn file(mut self, name: &str, bytes: Vec<u8>) -> Self {
        self.files.push((name.to_owned(), bytes));
        self
    }

    /// Extra row of components after the cards.
    pub fn push(mut self, component: Value) -> Self {
        self.components.push(component);
        self
    }

    pub fn flags(&self) -> u64 {
        FLAG_COMPONENTS_V2 | if self.ephemeral { FLAG_EPHEMERAL } else { 0 }
    }

    /// JSON body for creating or editing a message.
    pub fn payload(&self) -> Value {
        let mut payload = json!({
            "flags": self.flags(),
            "components": self.components,
            "allowed_mentions": {
                "parse": if self.ping_everyone { vec!["everyone"] } else { Vec::new() },
                "users": self.ping_users,
                "roles": self.ping_roles,
            },
        });
        if !self.files.is_empty() {
            let attachments: Vec<Value> = self
                .files
                .iter()
                .enumerate()
                .map(|(id, (name, _))| json!({ "id": id, "filename": name }))
                .collect();
            payload["attachments"] = json!(attachments);
        }
        payload
    }
}

pub fn container(color: u32, children: Vec<Value>) -> Value {
    json!({ "type": 17, "accent_color": color, "components": children })
}

pub fn text(content: impl Into<String>) -> Value {
    json!({ "type": 10, "content": truncate(&content.into(), TEXT_LIMIT) })
}

/// Thin divider line.
pub fn separator() -> Value {
    json!({ "type": 14, "divider": true, "spacing": 1 })
}

/// Text with a small image on the right.
pub fn section_thumb(content: impl Into<String>, image_url: &str) -> Value {
    json!({
        "type": 9,
        "components": [text(content)],
        "accessory": { "type": 11, "media": { "url": image_url } },
    })
}

/// Text with a button on the right.
pub fn section_button(content: impl Into<String>, button: Value) -> Value {
    json!({ "type": 9, "components": [text(content)], "accessory": button })
}

/// Full-width image. Use `attachment://name.png` for an uploaded file.
pub fn gallery(url: &str) -> Value {
    json!({ "type": 12, "items": [{ "media": { "url": url } }] })
}

pub fn row(children: Vec<Value>) -> Value {
    json!({ "type": 1, "components": children })
}

pub fn button(style: u8, label: &str, custom_id: &str) -> Value {
    json!({ "type": 2, "style": style, "label": label, "custom_id": custom_id })
}

/// Button with an emoji before its label.
pub fn emoji_button(style: u8, emoji: &str, label: &str, custom_id: &str) -> Value {
    let mut value = button(style, label, custom_id);
    value["emoji"] = json!({ "name": emoji });
    value
}

pub fn disabled(mut component: Value) -> Value {
    component["disabled"] = json!(true);
    component
}

pub fn link_button(label: &str, url: &str) -> Value {
    json!({ "type": 2, "style": 5, "label": label, "url": url })
}

/// A menu with `(value, label, description)` options; `selected` are ticked.
pub fn string_select(
    custom_id: &str,
    placeholder: &str,
    options: &[(&str, &str, &str)],
    selected: &[&str],
    max_values: usize,
) -> Value {
    let options: Vec<Value> = options
        .iter()
        .map(|(value, label, description)| {
            let mut option = json!({ "label": truncate(label, 100), "value": value });
            if !description.is_empty() {
                option["description"] = json!(truncate(description, 100));
            }
            if selected.contains(value) {
                option["default"] = json!(true);
            }
            option
        })
        .collect();
    let max = max_values.clamp(1, options.len().max(1));
    json!({
        "type": 3,
        "custom_id": custom_id,
        "placeholder": placeholder,
        "options": options,
        "min_values": if max > 1 { 0 } else { 1 },
        "max_values": max,
    })
}

/// What an auto-populated menu offers.
pub enum Pick<'a> {
    Users,
    Roles,
    /// Channels of these types (0 text, 4 category, 5 announcement).
    Channels(&'a [u8]),
}

/// Menu of users, roles or channels, listed by Discord itself. `current`
/// shows as selected; with `max_values` 0..n, it can also be emptied.
pub fn entity_select(
    custom_id: &str,
    placeholder: &str,
    pick: Pick,
    current: &[String],
    max_values: usize,
) -> Value {
    let (kind, default_type) = match pick {
        Pick::Users => (5, "user"),
        Pick::Roles => (6, "role"),
        Pick::Channels(_) => (8, "channel"),
    };
    let defaults: Vec<Value> = current
        .iter()
        .take(max_values)
        .map(|id| json!({ "id": id, "type": default_type }))
        .collect();
    let mut value = json!({
        "type": kind,
        "custom_id": custom_id,
        "placeholder": placeholder,
        "min_values": 0,
        "max_values": max_values.clamp(1, 25),
        "default_values": defaults,
    });
    if let Pick::Channels(types) = pick {
        value["channel_types"] = json!(types);
    }
    value
}

pub fn select(custom_id: &str, placeholder: &str, options: &[String]) -> Value {
    let options: Vec<Value> = options
        .iter()
        .enumerate()
        .map(|(index, label)| json!({ "label": truncate(label, 100), "value": index.to_string() }))
        .collect();
    json!({
        "type": 3,
        "custom_id": custom_id,
        "placeholder": placeholder,
        "options": options,
    })
}

/// Modal with single-line text inputs: `(custom_id, label, placeholder, max_length)`.
pub fn modal(custom_id: &str, title: &str, inputs: &[(&str, &str, &str, u32)]) -> Value {
    let rows: Vec<Value> = inputs
        .iter()
        .map(|(id, label, placeholder, max)| {
            row(vec![json!({
                "type": 4,
                "custom_id": id,
                "label": label,
                "style": 1,
                "placeholder": placeholder,
                "max_length": max,
                "required": true,
            })])
        })
        .collect();
    json!({ "custom_id": custom_id, "title": title, "components": rows })
}

/// A field of `form_modal`.
pub struct Field<'a> {
    pub id: &'a str,
    pub label: &'a str,
    pub placeholder: &'a str,
    pub max: u32,
    /// Multi-line text box.
    pub paragraph: bool,
    pub required: bool,
}

pub fn form_modal(custom_id: &str, title: &str, fields: &[Field]) -> Value {
    let rows: Vec<Value> = fields
        .iter()
        .map(|field| {
            row(vec![json!({
                "type": 4,
                "custom_id": field.id,
                "label": field.label,
                "style": if field.paragraph { 2 } else { 1 },
                "placeholder": field.placeholder,
                "max_length": field.max,
                "required": field.required,
            })])
        })
        .collect();
    json!({ "custom_id": custom_id, "title": title, "components": rows })
}

pub fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Neutralizes markdown and mentions in user-provided text shown inline.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(
            c,
            '\\' | '*' | '_' | '~' | '`' | '|' | '>' | '#' | '[' | ']' | '@'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// 12345 -> "12,345".
pub fn number(n: u64) -> String {
    dc_core::thousands(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_is_components_v2() {
        let msg = Msg::card(WHITE, vec![text("Hi")]).ephemeral();
        let payload = msg.payload();
        assert_eq!(payload["flags"], (1 << 15) | (1 << 6));
        assert_eq!(payload["components"][0]["type"], 17);
        assert_eq!(payload["components"][0]["components"][0]["type"], 10);
        assert!(payload.get("content").is_none());
        assert!(payload.get("embeds").is_none());
        assert_eq!(payload["allowed_mentions"]["parse"], json!([]));
    }

    #[tokio::test]
    async fn notices_get_an_image_banner() {
        // Plain text goes on the image.
        let msg = Msg::denied().finish().await;
        let payload = msg.payload();
        let card = &payload["components"][0];
        assert_eq!(card["accent_color"], RED);
        assert_eq!(card["components"][0]["type"], 12);
        assert_eq!(card["components"].as_array().unwrap().len(), 1);
        assert_eq!(payload["attachments"][0]["filename"], "banner.png");
        assert!(msg.files[0].1.starts_with(&[0x89, b'P', b'N', b'G']));

        // Mentions and formatting stay as text, so Discord renders them.
        let msg = Msg::success("Member muted", "**Member:** <@42>")
            .finish()
            .await;
        let card = &msg.payload()["components"][0];
        assert_eq!(card["components"][0]["type"], 12);
        assert_eq!(card["components"][1]["content"], "**Member:** <@42>");
    }

    #[test]
    fn plain_text_detection() {
        assert!(is_plain("This command is reserved for the administrators."));
        assert!(!is_plain("<@42> was muted"));
        assert!(!is_plain("Use `10m`"));
        assert!(!is_plain("line\nbreak"));
        assert!(!is_plain(&"x".repeat(300)));
    }

    #[test]
    fn menus() {
        let menu = string_select("m", "Pick", &[("a", "A", ""), ("b", "B", "Bee")], &["b"], 1);
        assert_eq!(menu["min_values"], 1);
        assert_eq!(menu["options"][1]["default"], true);
        assert!(menu["options"][0].get("description").is_none());
        let channels = entity_select("c", "Channel", Pick::Channels(&[0]), &["1".into()], 1);
        assert_eq!(channels["type"], 8);
        assert_eq!(channels["default_values"][0]["type"], "channel");
        assert_eq!(channels["channel_types"], json!([0]));
    }

    #[test]
    fn files_are_declared() {
        let payload = Msg::card(WHITE, vec![gallery("attachment://a.png")])
            .file("a.png", vec![1])
            .payload();
        assert_eq!(payload["attachments"][0]["filename"], "a.png");
    }

    #[test]
    fn text_helpers() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello world", 6), "hello…");
        assert_eq!(escape("**@everyone**"), "\\*\\*\\@everyone\\*\\*");
        assert_eq!(number(1234567), "1,234,567");
    }
}
