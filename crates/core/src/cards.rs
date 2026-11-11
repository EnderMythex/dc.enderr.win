//! Image cards for the bot's answers: a banner for every notice (success,
//! warning, error, access denied, confirmation...), the settings panel and
//! the leaderboard. Same black, dashed-frame style as the dashboard.

use std::fmt::Write;

use crate::{
    render::{fit_text, frame, wrap},
    thousands,
};

const WIDTH: u32 = 1000;
const TEXT: &str = "#ededed";
const MUTED: &str = "#8f8f8f";
const DIM: &str = "#5c5c5c";
const LINE: &str = "#222222";

/// The kind of a notice: its color and its icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tone {
    Success,
    Warning,
    Error,
    Denied,
    Info,
    Confirm,
    Gift,
    Announce,
    Maintenance,
}

impl Tone {
    pub fn color(self) -> &'static str {
        match self {
            Self::Success => "#3ecf8e",
            Self::Warning | Self::Confirm => "#f5a524",
            Self::Error | Self::Denied => "#ff5c5c",
            Self::Info => "#ededed",
            Self::Gift => "#f5c542",
            Self::Announce => "#7aa2ff",
            Self::Maintenance => "#f5a524",
        }
    }

    /// Icon drawn in a 64 × 64 box centered on (0, 0).
    fn icon(self) -> &'static str {
        match self {
            Self::Success => r#"<path d="M-15 1 L-5 11 L16 -11" fill="none" stroke-width="6"/>"#,
            Self::Error => {
                r#"<path d="M-12 -12 L12 12 M12 -12 L-12 12" fill="none" stroke-width="6"/>"#
            }
            Self::Warning => {
                r#"<path d="M0 -16 V5" fill="none" stroke-width="6"/><circle cx="0" cy="16" r="3.6" stroke="none"/>"#
            }
            Self::Denied => {
                r#"<rect x="-15" y="-3" width="30" height="22" rx="5" stroke="none"/><path d="M-9 -3 V-9 a9 9 0 0 1 18 0 V-3" fill="none" stroke-width="5"/>"#
            }
            Self::Info => {
                r#"<circle cx="0" cy="-14" r="3.6" stroke="none"/><path d="M0 -4 V16" fill="none" stroke-width="6"/>"#
            }
            Self::Confirm => {
                r#"<path d="M-9 -9 a9 9 0 1 1 13 8 c-4 2 -4 4 -4 8" fill="none" stroke-width="6"/><circle cx="0" cy="17" r="3.6" stroke="none"/>"#
            }
            Self::Announce => {
                r#"<path d="M-16 -6 h8 l16 -10 v32 l-16 -10 h-8 z" stroke="none"/><path d="M-12 6 l3 12" fill="none" stroke-width="5"/><path d="M14 -6 a8 8 0 0 1 0 12 M18 -12 a15 15 0 0 1 0 24" fill="none" stroke-width="3.5"/>"#
            }
            Self::Maintenance => {
                r#"<path d="M9 -17 a10 10 0 0 0 -11 13 l-14 14 a4 4 0 0 0 6 6 l14 -14 a10 10 0 0 0 13 -11 l-6 6 l-6 -2 l-2 -6 z" stroke="none"/>"#
            }
            Self::Gift => {
                r#"<rect x="-17" y="-4" width="34" height="22" rx="3" fill="none" stroke-width="4.5"/><rect x="-20" y="-11" width="40" height="8" rx="2.5" stroke="none"/><path d="M0 -11 V18" fill="none" stroke-width="4.5"/><path d="M0 -11 c-4 -9 -15 -9 -11 -2 c2 3 7 2 11 2 c4 0 9 1 11 -2 c4 -7 -7 -7 -11 2" fill="none" stroke-width="3.5"/>"#
            }
        }
    }
}

/// Approximate width of a text in Inter, to size chips.
fn text_width(s: &str, size: f32) -> f32 {
    s.chars()
        .map(|c| match c {
            'i' | 'l' | 'j' | '.' | ',' | ':' | ';' | '!' | '|' | '\'' => 0.28,
            'f' | 't' | 'r' | ' ' | '(' | ')' | '-' | '#' => 0.38,
            'm' | 'w' | 'M' | 'W' | '@' => 0.86,
            c if c.is_uppercase() => 0.68,
            c if c.is_ascii_digit() => 0.6,
            _ => 0.56,
        })
        .sum::<f32>()
        * size
}

/// Round badge with the tone's icon.
fn badge(out: &mut String, cx: u32, cy: u32, tone: Tone) {
    let color = tone.color();
    let _ = write!(
        out,
        r##"<circle cx="{cx}" cy="{cy}" r="46" fill="{color}" fill-opacity="0.1" stroke="{color}" stroke-opacity="0.45" stroke-width="2"/>
<g transform="translate({cx} {cy})" fill="{color}" stroke="{color}" stroke-linecap="round" stroke-linejoin="round">{}</g>"##,
        tone.icon()
    );
}

/// Banner: icon, a small label (the bot's name), a title and up to four
/// lines of plain text.
pub fn banner(tone: Tone, label: &str, title: &str, body: &str) -> String {
    let mut lines = wrap(body, 52);
    lines.truncate(4);
    let height = 232 + 34 * lines.len() as u32;
    let center = height / 2;
    let text_top = center - (90 + 34 * lines.len() as u32) / 2;
    let mut out = String::new();
    badge(&mut out, 132, center, tone);
    let _ = write!(
        out,
        r##"<text x="216" y="{}" font-size="18" font-weight="600" fill="{}" letter-spacing="4">{}</text>
<text x="214" y="{}" font-size="44" font-weight="700" fill="{TEXT}">{}</text>"##,
        text_top + 18,
        tone.color(),
        fit_text(&label.to_uppercase(), 40),
        text_top + 70,
        fit_text(title, 30),
    );
    for (index, line) in lines.iter().enumerate() {
        let _ = write!(
            out,
            r##"<text x="216" y="{}" font-size="24" fill="{MUTED}">{}</text>"##,
            text_top + 112 + 34 * index as u32,
            fit_text(line, 60),
        );
    }
    frame(WIDTH, height, &out)
}

/// Right-hand side of a settings row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    /// A drawn on/off switch.
    Switch(bool),
    /// A channel, a role, a mode: text in a pill.
    Chip(String),
    /// "Not set", "None": discreet text.
    Muted(String),
    /// Module state: a switch and, left of it, its channel.
    Module(bool, Option<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub label: String,
    pub hint: String,
    pub value: Value,
}

impl Row {
    pub fn new(label: &str, hint: &str, value: Value) -> Self {
        Self {
            label: label.to_owned(),
            hint: hint.to_owned(),
            value,
        }
    }
}

fn switch(out: &mut String, right: f32, cy: f32, on: bool) {
    let (w, h) = (64.0, 34.0);
    let x = right - w;
    let (track, stroke, knob, knob_x) = if on {
        (TEXT, TEXT, "#050505", x + w - h / 2.0)
    } else {
        ("#050505", "#3d3d3d", "#666666", x + h / 2.0)
    };
    let _ = write!(
        out,
        r##"<rect x="{x}" y="{}" width="{w}" height="{h}" rx="17" fill="{track}" stroke="{stroke}" stroke-width="1.5"/><circle cx="{knob_x}" cy="{cy}" r="12" fill="{knob}"/>"##,
        cy - h / 2.0,
    );
}

fn chip(out: &mut String, right: f32, cy: f32, label: &str) {
    let label: String = label.chars().take(28).collect();
    let w = text_width(&label, 18.0) + 32.0;
    let x = right - w;
    let _ = write!(
        out,
        r##"<rect x="{x}" y="{}" width="{w}" height="36" rx="18" fill="#161616" stroke="#333333"/>
<text x="{}" y="{}" font-size="18" font-weight="600" fill="{TEXT}" text-anchor="middle">{}</text>"##,
        cy - 18.0,
        x + w / 2.0,
        cy + 6.0,
        fit_text(&label, 28),
    );
}

fn muted(out: &mut String, right: f32, cy: f32, label: &str) {
    let _ = write!(
        out,
        r##"<text x="{right}" y="{}" font-size="19" fill="{DIM}" text-anchor="end">{}</text>"##,
        cy + 7.0,
        fit_text(label, 30),
    );
}

/// Settings panel: a header (badge, label, title, description) and one row
/// per setting with its value on the right.
pub fn settings(label: &str, title: &str, description: &str, rows: &[Row]) -> String {
    let mut lines = wrap(description, 56);
    lines.truncate(3);
    let header = 176 + 30 * lines.len() as u32;
    let row_h = 84;
    let height = header + row_h * rows.len() as u32 + 44;
    let mut out = String::new();
    badge(&mut out, 132, 128, Tone::Info);
    let _ = write!(
        out,
        r##"<text x="216" y="100" font-size="18" font-weight="600" fill="{MUTED}" letter-spacing="4">{}</text>
<text x="214" y="150" font-size="42" font-weight="700" fill="{TEXT}">{}</text>"##,
        fit_text(&label.to_uppercase(), 40),
        fit_text(title, 30),
    );
    for (index, line) in lines.iter().enumerate() {
        let _ = write!(
            out,
            r##"<text x="216" y="{}" font-size="21" fill="{MUTED}">{}</text>"##,
            186 + 30 * index as u32,
            fit_text(line, 64),
        );
    }

    let right = (WIDTH - 72) as f32;
    for (index, row) in rows.iter().enumerate() {
        let top = header + 16 + row_h * index as u32;
        let cy = (top + row_h / 2) as f32;
        let _ = write!(
            out,
            r##"<line x1="72" y1="{top}" x2="{}" y2="{top}" stroke="{LINE}" stroke-width="1"/>"##,
            WIDTH - 72
        );
        let (label_y, hint) = if row.hint.is_empty() {
            (cy + 8.0, None)
        } else {
            (cy - 4.0, Some(&row.hint))
        };
        let _ = write!(
            out,
            r##"<text x="72" y="{label_y}" font-size="23" font-weight="600" fill="{TEXT}">{}</text>"##,
            fit_text(&row.label, 30)
        );
        if let Some(hint) = hint {
            let _ = write!(
                out,
                r##"<text x="72" y="{}" font-size="16" fill="{DIM}">{}</text>"##,
                cy + 22.0,
                fit_text(hint, 46)
            );
        }
        match &row.value {
            Value::Switch(on) => switch(&mut out, right, cy, *on),
            Value::Chip(text) => chip(&mut out, right, cy, text),
            Value::Muted(text) => muted(&mut out, right, cy, text),
            Value::Module(on, channel) => {
                switch(&mut out, right, cy, *on);
                match channel {
                    Some(channel) => chip(&mut out, right - 84.0, cy, channel),
                    None => muted(&mut out, right - 84.0, cy, "No channel"),
                }
            }
        }
    }
    frame(WIDTH, height, &out)
}

pub struct LeaderboardRow {
    pub name: String,
    pub level: u64,
    pub xp: u64,
}

/// The ten most active members of a server.
pub fn leaderboard(server: &str, rows: &[LeaderboardRow]) -> String {
    let row_h = 66;
    let header = 190;
    let height = header + row_h * rows.len().max(1) as u32 + 40;
    let mut out = String::new();
    let _ = write!(
        out,
        r##"<text x="72" y="104" font-size="18" font-weight="600" fill="{MUTED}" letter-spacing="4">{}</text>
<text x="70" y="154" font-size="44" font-weight="700" fill="{TEXT}">Leaderboard</text>"##,
        fit_text(&server.to_uppercase(), 44),
    );
    if rows.is_empty() {
        let _ = write!(
            out,
            r##"<text x="72" y="{}" font-size="22" fill="{MUTED}">Nobody has earned XP yet.</text>"##,
            header + 40
        );
    }
    for (index, row) in rows.iter().enumerate() {
        let top = header + row_h * index as u32;
        let cy = top + row_h / 2;
        let rank_color = match index {
            0 => "#f5c542",
            1 => "#c9d1d9",
            2 => "#d58b4a",
            _ => MUTED,
        };
        if index % 2 == 0 {
            let _ = write!(
                out,
                r##"<rect x="56" y="{}" width="{}" height="{}" rx="12" fill="#0f0f0f"/>"##,
                top + 4,
                WIDTH - 112,
                row_h - 8
            );
        }
        let _ = write!(
            out,
            r##"<text x="104" y="{}" font-size="24" font-weight="700" fill="{rank_color}" text-anchor="middle">#{}</text>
<text x="152" y="{}" font-size="24" font-weight="600" fill="{TEXT}">{}</text>
<text x="660" y="{}" font-size="20" fill="{MUTED}" text-anchor="end">Level <tspan fill="{TEXT}" font-weight="600">{}</tspan></text>
<text x="{}" y="{}" font-size="20" fill="{MUTED}" text-anchor="end">{} XP</text>"##,
            cy + 8,
            index + 1,
            cy + 8,
            fit_text(&row.name, 26),
            cy + 7,
            row.level,
            WIDTH - 84,
            cy + 7,
            thousands(row.xp),
        );
    }
    frame(WIDTH, height, &out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::png;

    const PNG_MAGIC: &[u8] = &[0x89, b'P', b'N', b'G'];

    #[test]
    fn every_card_renders() {
        for tone in [
            Tone::Success,
            Tone::Warning,
            Tone::Error,
            Tone::Denied,
            Tone::Info,
            Tone::Confirm,
            Tone::Gift,
            Tone::Announce,
            Tone::Maintenance,
        ] {
            let svg = banner(
                tone,
                "Sang",
                "Access denied",
                "Only admins <can> do this & more.",
            );
            assert!(png(&svg).unwrap().starts_with(PNG_MAGIC), "{tone:?}");
        }
        let rows = [
            Row::new(
                "Welcome",
                "New members",
                Value::Module(true, Some("#welcome".into())),
            ),
            Row::new("Logs", "", Value::Module(false, None)),
            Row::new("Image", "", Value::Switch(true)),
            Row::new("Role", "", Value::Chip("@Verified".into())),
            Row::new("Category", "", Value::Muted("Not set".into())),
        ];
        let svg = settings("Sang · Settings", "Settings", "Pick a module below.", &rows);
        assert!(png(&svg).unwrap().starts_with(PNG_MAGIC));

        let top = [LeaderboardRow {
            name: "Ender".into(),
            level: 12,
            xp: 15_730,
        }];
        assert!(
            png(&leaderboard("My server", &top))
                .unwrap()
                .starts_with(PNG_MAGIC)
        );
        assert!(
            png(&leaderboard("Empty", &[]))
                .unwrap()
                .starts_with(PNG_MAGIC)
        );
    }

    #[test]
    fn banner_grows_with_its_text() {
        let short = banner(Tone::Info, "x", "Title", "");
        let long = banner(Tone::Info, "x", "Title", &"word ".repeat(60));
        let height = |svg: &str| {
            let start = svg.find("height=\"").unwrap() + 8;
            svg[start..]
                .split('"')
                .next()
                .unwrap()
                .parse::<u32>()
                .unwrap()
        };
        assert!(height(&long) > height(&short));
        // At most four lines of text.
        assert_eq!(height(&long), 232 + 34 * 4);
    }

    /// Writes every card to `DC_PREVIEW_DIR`, to check them by eye:
    /// `DC_PREVIEW_DIR=out cargo test -p dc-core cards_preview -- --ignored`
    #[test]
    #[ignore]
    fn cards_preview() {
        let dir = std::path::PathBuf::from(std::env::var("DC_PREVIEW_DIR").unwrap());
        let rows = [
            Row::new(
                "Welcome",
                "Message and image for new members",
                Value::Module(true, Some("#welcome".into())),
            ),
            Row::new(
                "Rules & verification",
                "Captcha image · @Verified",
                Value::Module(true, Some("#rules".into())),
            ),
            Row::new(
                "Tickets",
                "Private channels with the staff",
                Value::Module(false, None),
            ),
            Row::new("Levels", "15–25 XP per message", Value::Module(true, None)),
            Row::new("Bot admins", "", Value::Chip("2 members · 1 role".into())),
        ];
        let top: Vec<LeaderboardRow> = (0..10)
            .map(|i| LeaderboardRow {
                name: ["EnderMythex", "Lyra", "Kaito", "Mélodie", "Zed"][i % 5].into(),
                level: 30 - i as u64 * 2,
                xp: 52_000 - i as u64 * 4_100,
            })
            .collect();
        let cards = [
            (
                "banner-denied.png",
                banner(
                    Tone::Denied,
                    "Sang",
                    "Access denied",
                    "This command is reserved for the administrators of this bot.",
                ),
            ),
            (
                "banner-success.png",
                banner(Tone::Success, "Sang", "Member banned", ""),
            ),
            (
                "banner-announce.png",
                banner(Tone::Announce, "Sang", "Summer event this weekend", ""),
            ),
            (
                "banner-maintenance.png",
                banner(
                    Tone::Maintenance,
                    "Sang",
                    "Maintenance in progress",
                    "We are updating the server.",
                ),
            ),
            (
                "banner-gift.png",
                banner(
                    Tone::Gift,
                    "Sang · Giveaway",
                    "Discord Nitro (1 month)",
                    "2 winners. Click Enter below to take part.",
                ),
            ),
            (
                "banner-confirm.png",
                banner(
                    Tone::Confirm,
                    "Sang",
                    "Ban EnderMythex?",
                    "They will be removed from the server and cannot come back until unbanned.",
                ),
            ),
            (
                "settings.png",
                settings(
                    "Sang · Settings",
                    "Settings",
                    "Choose a module in the menu below to change it. Everything is also on the dashboard.",
                    &rows,
                ),
            ),
            ("leaderboard.png", leaderboard("Sang Community", &top)),
        ];
        for (name, svg) in cards {
            std::fs::write(dir.join(name), png(&svg).unwrap()).unwrap();
        }
    }
}
