//! Images sent by the bot (welcome card, rank card, captcha). Each one is an
//! SVG string rendered to PNG with resvg, using the bundled Inter font so the
//! result is identical on every machine.

use std::{
    borrow::Cow,
    path::Path,
    sync::{Arc, LazyLock, RwLock},
};

use crate::{crypto::random_range, thousands};
use base64::Engine;
use resvg::{tiny_skia, usvg};

static OPTIONS: LazyLock<usvg::Options<'static>> = LazyLock::new(|| {
    let mut options = usvg::Options::default();
    let fonts = options.fontdb_mut();
    fonts.load_font_data(include_bytes!("../assets/fonts/Inter-Regular.otf").to_vec());
    fonts.load_font_data(include_bytes!("../assets/fonts/Inter-SemiBold.otf").to_vec());
    fonts.load_font_data(include_bytes!("../assets/fonts/Inter-Bold.otf").to_vec());
    options.font_family = "Inter".into();
    options
});

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("invalid SVG: {0}")]
    Svg(#[from] usvg::Error),
    #[error("empty image")]
    Empty,
    #[error("PNG encoding: {0}")]
    Png(String),
}

/// Characters that cannot be confused with one another in a captcha.
const CAPTCHA_ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";
pub const CAPTCHA_LENGTH: usize = 5;

/// File of a bot folder holding the picture drawn behind its cards.
pub const BACKGROUND_FILE: &str = "background";
/// Largest background picture accepted.
pub const BACKGROUND_MAX_BYTES: usize = 3 * 1024 * 1024;

/// MIME type of a picture, from its first bytes.
pub fn image_type(bytes: &[u8]) -> Option<&'static str> {
    match bytes {
        [0x89, b'P', b'N', b'G', ..] => Some("image/png"),
        [0xFF, 0xD8, 0xFF, ..] => Some("image/jpeg"),
        [b'G', b'I', b'F', b'8', ..] => Some("image/gif"),
        [
            b'R',
            b'I',
            b'F',
            b'F',
            _,
            _,
            _,
            _,
            b'W',
            b'E',
            b'B',
            b'P',
            ..,
        ] => Some("image/webp"),
        _ => None,
    }
}

/// A picture drawn behind every card of a bot (notices, welcome, rank,
/// leaderboard, settings), darkened so the text stays readable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Background {
    href: String,
    /// Darkening, 0 (none) to 90 (%).
    dim: u8,
}

impl Background {
    pub fn from_bytes(bytes: &[u8], dim: u8) -> Option<Self> {
        if bytes.len() > BACKGROUND_MAX_BYTES {
            return None;
        }
        let mime = image_type(bytes)?;
        let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
        Some(Self {
            href: format!("data:{mime};base64,{encoded}"),
            dim: dim.min(90),
        })
    }

    /// The background of a bot folder, if it has one.
    pub fn load(bot_dir: &Path, dim: u8) -> Option<Self> {
        let bytes = std::fs::read(bot_dir.join(BACKGROUND_FILE)).ok()?;
        Self::from_bytes(&bytes, dim)
    }

    fn draw(&self, width: u32, height: u32) -> String {
        format!(
            r##"<clipPath id="bg-clip"><rect width="{width}" height="{height}" rx="22"/></clipPath><g clip-path="url(#bg-clip)"><image href="{}" x="0" y="0" width="{width}" height="{height}" preserveAspectRatio="xMidYMid slice"/><rect width="{width}" height="{height}" fill="#050505" fill-opacity="{:.2}"/></g>"##,
            self.href,
            f32::from(self.dim) / 100.0
        )
    }
}

/// The background of the running bot (`png` uses it).
static BACKGROUND: RwLock<Option<Arc<Background>>> = RwLock::new(None);

/// Sets the background drawn by `png` in this process (the bot).
pub fn set_background(background: Option<Background>) {
    if let Ok(mut current) = BACKGROUND.write() {
        *current = background.map(Arc::new);
    }
}

/// Where `frame` leaves room for a background: `<!--background W H-->`.
const SLOT: &str = "<!--background ";

fn with_background<'a>(svg: &'a str, background: Option<&Background>) -> Cow<'a, str> {
    let Some(start) = svg.find(SLOT) else {
        return Cow::Borrowed(svg);
    };
    let Some(length) = svg[start..].find("-->") else {
        return Cow::Borrowed(svg);
    };
    let end = start + length + 3;
    let drawn = background
        .and_then(|background| {
            let mut size = svg[start + SLOT.len()..end - 3].split_whitespace();
            let width = size.next()?.parse().ok()?;
            let height = size.next()?.parse().ok()?;
            Some(background.draw(width, height))
        })
        .unwrap_or_default();
    Cow::Owned(format!("{}{drawn}{}", &svg[..start], &svg[end..]))
}

/// Renders an SVG document to PNG bytes, with the bot's background behind
/// cards. CPU-bound: call it from `spawn_blocking`.
pub fn png(svg: &str) -> Result<Vec<u8>, RenderError> {
    let background = BACKGROUND.read().ok().and_then(|current| current.clone());
    png_with(svg, background.as_deref())
}

/// Like `png`, with a given background (the panel draws for many bots).
pub fn png_with(svg: &str, background: Option<&Background>) -> Result<Vec<u8>, RenderError> {
    let svg = with_background(svg, background);
    let tree = usvg::Tree::from_str(&svg, &OPTIONS)?;
    let size = tree.size().to_int_size();
    let mut pixmap =
        tiny_skia::Pixmap::new(size.width(), size.height()).ok_or(RenderError::Empty)?;
    resvg::render(&tree, tiny_skia::Transform::default(), &mut pixmap.as_mut());
    pixmap
        .encode_png()
        .map_err(|error| RenderError::Png(error.to_string()))
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Characters the bundled font can draw: Latin, Greek, Cyrillic and common
/// punctuation. Emoji and other scripts would show as empty boxes.
fn drawable(c: char) -> bool {
    let code = c as u32;
    !c.is_control()
        && (code < 0x0250
            || (0x0370..0x0530).contains(&code)
            || (0x1E00..0x1F00).contains(&code)
            || (0x2000..0x2070).contains(&code)
            || (0x20A0..0x20C0).contains(&code)
            || c == '·')
}

/// Text cleaned for drawing: unsupported characters dropped, spaces collapsed.
fn clean(s: &str) -> String {
    let kept: String = s.chars().filter(|c| drawable(*c)).collect();
    kept.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Keeps a name on one line of the card.
fn fit(s: &str, max_chars: usize) -> String {
    let clean = clean(s);
    if clean.chars().count() <= max_chars {
        return esc(&clean);
    }
    let cut: String = clean.chars().take(max_chars - 1).collect();
    esc(&format!("{}…", cut.trim_end()))
}

/// Escaped text cut to one line of at most `max_chars` characters.
pub(crate) fn fit_text(s: &str, max_chars: usize) -> String {
    fit(s, max_chars)
}

/// Splits a text into lines of at most `max_chars` characters.
pub(crate) fn wrap(s: &str, max_chars: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for word in clean(s).split(' ').filter(|w| !w.is_empty()) {
        match lines.last_mut() {
            Some(line) if line.chars().count() + 1 + word.chars().count() <= max_chars => {
                line.push(' ');
                line.push_str(word);
            }
            _ => lines.push(word.to_owned()),
        }
    }
    lines
}

/// Black card with the dashed frame used on the dashboard.
pub(crate) fn frame(width: u32, height: u32, body: &str) -> String {
    let (w, h) = (width, height);
    let (r, b) = (w - 28, h - 28);
    let marker = |x: u32, y: u32| {
        format!(
            r##"<rect x="{}" y="{}" width="9" height="9" fill="#050505" stroke="#3d3d3d"/>"##,
            x - 4,
            y - 4
        )
    };
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}">
<defs><radialGradient id="glow" cx="50%" cy="0%" r="75%"><stop offset="0" stop-color="#ffffff" stop-opacity="0.09"/><stop offset="1" stop-color="#ffffff" stop-opacity="0"/></radialGradient></defs>
<rect width="{w}" height="{h}" rx="22" fill="#050505"/>
<!--background {w} {h}-->
<rect width="{w}" height="{h}" rx="22" fill="url(#glow)"/>
<g stroke="#2a2a2a" stroke-width="1" stroke-dasharray="4 4">
<line x1="28" y1="0" x2="28" y2="{h}"/><line x1="{r}" y1="0" x2="{r}" y2="{h}"/>
<line x1="0" y1="28" x2="{w}" y2="28"/><line x1="0" y1="{b}" x2="{w}" y2="{b}"/>
</g>
{}{}{}{}
{body}
</svg>"##,
        marker(28, 28),
        marker(r, 28),
        marker(28, b),
        marker(r, b),
    )
}

/// Round avatar, or a neutral disc when the picture could not be downloaded.
fn avatar(cx: u32, cy: u32, radius: u32, picture: Option<&[u8]>) -> String {
    let (x, y, d) = (cx - radius, cy - radius, radius * 2);
    let image = match picture {
        Some(bytes) => {
            let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
            format!(
                r##"<clipPath id="av"><circle cx="{cx}" cy="{cy}" r="{radius}"/></clipPath><image href="data:image/png;base64,{b64}" x="{x}" y="{y}" width="{d}" height="{d}" clip-path="url(#av)" preserveAspectRatio="xMidYMid slice"/>"##
            )
        }
        None => format!(r##"<circle cx="{cx}" cy="{cy}" r="{radius}" fill="#1a1a1a"/>"##),
    };
    format!(
        r##"{image}<circle cx="{cx}" cy="{cy}" r="{}" fill="none" stroke="#3d3d3d" stroke-width="2"/>"##,
        radius + 5
    )
}

/// Welcome card: avatar, name and a subtitle such as "Member #128 · Server".
pub fn welcome(name: &str, subtitle: &str, picture: Option<&[u8]>) -> String {
    let body = format!(
        r##"{}
<text x="330" y="140" font-size="22" font-weight="600" fill="#8f8f8f" letter-spacing="5">WELCOME</text>
<text x="328" y="208" font-size="60" font-weight="700" fill="#ededed">{}</text>
<text x="330" y="256" font-size="26" fill="#8f8f8f">{}</text>"##,
        avatar(180, 180, 96, picture),
        fit(name, 18),
        fit(subtitle, 42),
    );
    frame(1000, 360, &body)
}

pub struct RankCard<'a> {
    pub name: &'a str,
    pub rank: i64,
    pub level: u64,
    pub xp_in_level: u64,
    pub xp_for_level: u64,
    pub total_xp: u64,
    pub picture: Option<&'a [u8]>,
}

pub fn rank(card: &RankCard) -> String {
    let (bar_x, bar_w) = (300.0, 630.0);
    let ratio = if card.xp_for_level == 0 {
        0.0
    } else {
        (card.xp_in_level as f64 / card.xp_for_level as f64).clamp(0.0, 1.0)
    };
    // Keep a visible sliver so the rounded bar never collapses.
    let fill = (bar_w * ratio).max(24.0);
    let body = format!(
        r##"{}
<text x="300" y="118" font-size="48" font-weight="700" fill="#ededed">{}</text>
<text x="930" y="96" font-size="20" font-weight="600" fill="#8f8f8f" text-anchor="end" letter-spacing="3">RANK</text>
<text x="930" y="140" font-size="40" font-weight="700" fill="#ededed" text-anchor="end">#{}</text>
<text x="300" y="176" font-size="24" fill="#8f8f8f">Level <tspan fill="#ededed" font-weight="600">{}</tspan>  ·  {} XP total</text>
<rect x="{bar_x}" y="204" width="{bar_w}" height="24" rx="12" fill="#161616" stroke="#2a2a2a"/>
<rect x="{bar_x}" y="204" width="{fill}" height="24" rx="12" fill="#ededed"/>
<text x="930" y="258" font-size="20" fill="#8f8f8f" text-anchor="end">{} / {} XP</text>"##,
        avatar(165, 150, 82, card.picture),
        fit(card.name, 16),
        card.rank,
        card.level,
        thousands(card.total_xp),
        thousands(card.xp_in_level),
        thousands(card.xp_for_level),
    );
    frame(1000, 300, &body)
}

/// Random code for a captcha.
pub fn captcha_code() -> String {
    (0..CAPTCHA_LENGTH)
        .map(|_| {
            let index = random_range(0, CAPTCHA_ALPHABET.len() as u32 - 1) as usize;
            CAPTCHA_ALPHABET[index] as char
        })
        .collect()
}

/// Captcha image: tilted characters crossed by random lines and dots.
pub fn captcha(code: &str) -> String {
    let (w, h) = (460u32, 170u32);
    let mut body = String::new();
    let between = |min: u32, max: u32| random_range(min, max) as i32;

    for _ in 0..7 {
        body.push_str(&format!(
            r##"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="#{}" stroke-width="{}"/>"##,
            between(0, w / 3),
            between(10, h - 10),
            between(w * 2 / 3, w),
            between(10, h - 10),
            ["5a5a5a", "7a7a7a", "444444"][random_range(0, 2) as usize],
            between(1, 3),
        ));
    }
    for _ in 0..90 {
        body.push_str(&format!(
            r##"<circle cx="{}" cy="{}" r="{}" fill="#6a6a6a"/>"##,
            between(0, w),
            between(0, h),
            between(1, 2),
        ));
    }
    for (index, character) in code.chars().enumerate() {
        let x = 62 + index as i32 * 74 + between(0, 14) - 7;
        let y = 108 + between(0, 24) - 12;
        let angle = between(0, 50) - 25;
        let size = between(62, 78);
        body.push_str(&format!(
            r##"<text x="{x}" y="{y}" font-size="{size}" font-weight="700" fill="#ededed" text-anchor="middle" transform="rotate({angle} {x} {y})">{character}</text>"##
        ));
    }
    // Lines over the characters too, so they cannot be cut out cleanly.
    for _ in 0..3 {
        body.push_str(&format!(
            r##"<path d="M0 {} Q {} {} {w} {}" fill="none" stroke="#ededed" stroke-width="2" opacity="0.55"/>"##,
            between(40, h - 40),
            between(w / 3, w * 2 / 3),
            between(0, h),
            between(40, h - 40),
        ));
    }
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}"><rect width="{w}" height="{h}" rx="16" fill="#0a0a0a"/>{body}</svg>"##
    )
}

/// Size of the slide captcha's background.
pub const SLIDE_WIDTH: u32 = 460;
pub const SLIDE_HEIGHT: u32 = 220;
/// Size of the image of the puzzle piece (a 56 px square with two knobs).
pub const PIECE_SIZE: u32 = 66;
/// The knob on top of the piece rises this far above its square.
pub const PIECE_RISE: u32 = 10;
/// How far from the right place, in pixels, an answer may be.
pub const SLIDE_TOLERANCE: u32 = 6;

/// A slide captcha: a background with a hole, and the piece that fills it.
/// The member slides the piece horizontally; the answer is `x`.
pub struct SlidePuzzle {
    pub x: u32,
    pub y: u32,
    pub background: String,
    pub piece: String,
}

/// Outline of the piece with the top-left corner of its square at (x, y).
fn piece_path(x: u32, y: u32) -> String {
    format!("M{x} {y} h18 a10 10 0 0 1 20 0 h18 v18 a10 10 0 0 1 0 20 v18 h-56 z")
}

/// File of a bot folder holding the picture of its slide captcha.
pub const CAPTCHA_FILE: &str = "captcha";

/// A chosen picture as the scene: zoomed and moved at random, with a few
/// faint shapes on top, so no two puzzles are the same picture.
fn picture_scene(bytes: &[u8]) -> Option<String> {
    let mime = image_type(bytes)?;
    let (w, h) = (SLIDE_WIDTH, SLIDE_HEIGHT);
    let scale = random_range(100, 135);
    let (sw, sh) = (w * scale / 100, h * scale / 100);
    let (dx, dy) = (random_range(0, sw - w), random_range(0, sh - h));
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    let mut scene = format!(
        r##"<rect width="{w}" height="{h}" fill="#101010"/><image href="data:{mime};base64,{encoded}" x="-{dx}" y="-{dy}" width="{sw}" height="{sh}" preserveAspectRatio="xMidYMid slice"/>"##
    );
    for _ in 0..6 {
        scene.push_str(&format!(
            r##"<circle cx="{}" cy="{}" r="{}" fill="#ffffff" opacity="0.{:02}"/>"##,
            random_range(0, w),
            random_range(0, h),
            random_range(10, 40),
            random_range(6, 16)
        ));
    }
    Some(scene)
}

/// Random shapes, so the hole can only be matched by looking.
fn slide_scene() -> String {
    let (w, h) = (SLIDE_WIDTH, SLIDE_HEIGHT);
    let between = |min: u32, max: u32| random_range(min, max);
    let gray = || {
        let level = between(0x30, 0xb0);
        format!("#{level:02x}{level:02x}{level:02x}")
    };
    let mut scene = format!(
        r##"<defs><linearGradient id="sky" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#151515"/><stop offset="1" stop-color="#050505"/></linearGradient></defs><rect width="{w}" height="{h}" fill="url(#sky)"/>"##
    );
    for _ in 0..16 {
        scene.push_str(&format!(
            r##"<circle cx="{}" cy="{}" r="{}" fill="{}" opacity="0.{}"/>"##,
            between(0, w),
            between(0, h),
            between(12, 58),
            gray(),
            between(35, 85)
        ));
    }
    for _ in 0..8 {
        let (x, y) = (between(0, w), between(0, h));
        scene.push_str(&format!(
            r##"<path d="M{x} {y} l{} {} l{} {} z" fill="{}" opacity="0.{}"/>"##,
            between(20, 90),
            between(0, 70),
            between(0, 60),
            between(30, 90),
            gray(),
            between(40, 90)
        ));
    }
    for _ in 0..10 {
        scene.push_str(&format!(
            r##"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{}" stroke-width="{}" opacity="0.7"/>"##,
            between(0, w),
            between(0, h),
            between(0, w),
            between(0, h),
            gray(),
            between(1, 4)
        ));
    }
    scene
}

pub fn slide_puzzle() -> SlidePuzzle {
    slide_puzzle_with(None)
}

/// A slide captcha on a chosen picture (`None` or an unreadable file:
/// random shapes).
pub fn slide_puzzle_with(picture: Option<&[u8]>) -> SlidePuzzle {
    let (w, h) = (SLIDE_WIDTH, SLIDE_HEIGHT);
    let x = random_range(140, w - PIECE_SIZE - 14);
    let y = random_range(PIECE_RISE + 14, h - 56 - 14);
    let scene = picture.and_then(picture_scene).unwrap_or_else(slide_scene);
    let outline = piece_path(x, y);
    let background = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}">{scene}<path d="{outline}" fill="#000000" fill-opacity="0.62" stroke="#ffffff" stroke-opacity="0.75" stroke-width="2"/></svg>"##
    );
    let piece = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{PIECE_SIZE}" height="{PIECE_SIZE}" viewBox="{x} {} {PIECE_SIZE} {PIECE_SIZE}"><defs><clipPath id="piece"><path d="{outline}"/></clipPath></defs><g clip-path="url(#piece)">{scene}</g><path d="{outline}" fill="none" stroke="#ffffff" stroke-width="2.5"/></svg>"##,
        y - PIECE_RISE
    );
    SlidePuzzle {
        x,
        y,
        background,
        piece,
    }
}

/// Is a slide answer close enough to the hole?
pub fn slide_matches(answer: u32, x: u32) -> bool {
    answer.abs_diff(x) <= SLIDE_TOLERANCE
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG_MAGIC: &[u8] = &[0x89, b'P', b'N', b'G'];

    #[test]
    fn slide_puzzles_on_a_picture() {
        let picture = png(&captcha("ABCDE")).unwrap();
        let puzzle = slide_puzzle_with(Some(&picture));
        assert!(puzzle.background.contains("data:image/png;base64,"));
        assert!(puzzle.piece.contains("data:image/png;base64,"));
        assert!(png(&puzzle.background).unwrap().starts_with(PNG_MAGIC));
        assert!(png(&puzzle.piece).unwrap().starts_with(PNG_MAGIC));
        // Not a picture: the random shapes.
        assert!(
            !slide_puzzle_with(Some(b"nope"))
                .background
                .contains("<image")
        );
    }

    #[test]
    fn backgrounds() {
        // Any PNG will do as a picture: a captcha.
        let picture = png(&captcha("ABCDE")).unwrap();
        let background = Background::from_bytes(&picture, 60).unwrap();
        assert!(Background::from_bytes(b"not a picture", 60).is_none());

        let card = welcome("Ender", "Member #1", None);
        let plain = with_background(&card, None);
        assert!(!plain.contains("<image") && !plain.contains(SLOT));
        let drawn = with_background(&card, Some(&background));
        assert!(drawn.contains("data:image/png;base64,") && drawn.contains("0.60"));
        let with = png_with(&card, Some(&background)).unwrap();
        assert!(with.starts_with(PNG_MAGIC));
        assert_ne!(with, png_with(&card, None).unwrap());
    }

    #[test]
    fn cards_render_to_png() {
        let welcome = png(&welcome("Ender <&> Test", "Member #128 · My Server", None)).unwrap();
        assert!(welcome.starts_with(PNG_MAGIC));

        let rank = png(&rank(&RankCard {
            name: "A very long display name that overflows",
            rank: 3,
            level: 12,
            xp_in_level: 340,
            xp_for_level: 1420,
            total_xp: 15_730,
            picture: None,
        }))
        .unwrap();
        assert!(rank.starts_with(PNG_MAGIC));
    }

    #[test]
    fn text_is_actually_drawn() {
        // Without a usable font the card would be identical whatever the name.
        let a = png(&welcome("AAAA", "x", None)).unwrap();
        let b = png(&welcome("WWWW", "x", None)).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn captcha_codes() {
        let code = captcha_code();
        assert_eq!(code.len(), CAPTCHA_LENGTH);
        assert!(code.bytes().all(|b| CAPTCHA_ALPHABET.contains(&b)));
        assert!(png(&captcha(&code)).unwrap().starts_with(PNG_MAGIC));
    }

    /// Writes sample images to `DC_PREVIEW_DIR`, to check them by eye:
    /// `DC_PREVIEW_DIR=out cargo test -p dc-core preview -- --ignored`
    #[test]
    #[ignore]
    fn preview() {
        let dir = std::path::PathBuf::from(std::env::var("DC_PREVIEW_DIR").unwrap());
        let avatar = std::fs::read(dir.join("avatar.png")).ok();
        let card = RankCard {
            name: "EnderMythex",
            rank: 3,
            level: 12,
            xp_in_level: 340,
            xp_for_level: 1420,
            total_xp: 15_730,
            picture: avatar.as_deref(),
        };
        let images = [
            (
                "welcome.png",
                welcome("EnderMythex", "Member #128 · Sang", avatar.as_deref()),
            ),
            ("rank.png", rank(&card)),
            ("captcha.png", captcha(&captcha_code())),
            ("slide-background.png", slide_puzzle().background),
        ];
        for (name, svg) in images {
            std::fs::write(dir.join(name), png(&svg).unwrap()).unwrap();
        }
    }

    #[test]
    fn names_are_escaped_and_fitted() {
        assert_eq!(fit("<b>", 10), "&lt;b&gt;");
        assert_eq!(fit("abcdefghij", 5), "abcd…");
        // Emoji cannot be drawn with the font: dropped rather than shown as boxes.
        assert_eq!(fit("🔥 Ender  🔥 Mythex", 30), "Ender Mythex");
        assert_eq!(fit("Mélodie Привет", 30), "Mélodie Привет");
    }

    #[test]
    fn slide_puzzles() {
        for _ in 0..20 {
            let puzzle = slide_puzzle();
            assert!(puzzle.x + PIECE_SIZE <= SLIDE_WIDTH);
            assert!(puzzle.y >= PIECE_RISE && puzzle.y + 56 <= SLIDE_HEIGHT);
        }
        let puzzle = slide_puzzle();
        assert!(png(&puzzle.background).unwrap().starts_with(PNG_MAGIC));
        let piece = png(&puzzle.piece).unwrap();
        assert!(piece.starts_with(PNG_MAGIC));
        assert!(slide_matches(puzzle.x + 4, puzzle.x));
        assert!(!slide_matches(puzzle.x + 20, puzzle.x));
    }

    #[test]
    fn wrapping() {
        assert_eq!(
            wrap("one two three four", 9),
            vec!["one two", "three", "four"]
        );
        assert!(wrap("  ", 10).is_empty());
        assert_eq!(
            wrap("averyveryverylongword", 5),
            vec!["averyveryverylongword"]
        );
    }
}
