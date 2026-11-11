//! Bot registry. The admin creates one folder per bot:
//!
//! ```text
//! bots/<slug>/bot.toml       name, owners, container      (committed)
//! bots/<slug>/compose.yaml   the bot's own Docker container (committed)
//! bots/<slug>/.env           DISCORD_TOKEN=..., secrets    (git-ignored)
//! bots/<slug>/settings.json  written by the panel          (git-ignored)
//! ```
//!
//! Folders starting with `_` or `.` (such as `_template`) are ignored.

use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;

use crate::is_snowflake;

/// Docker label every bot container must carry. The panel refuses to touch a
/// container whose label does not match the bot slug, so a typo in `bot.toml`
/// can never let a user stop an unrelated container.
pub const CONTAINER_LABEL: &str = "win.enderr.dc.bot";

const SLUG_MAX_LEN: usize = 32;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BotFile {
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    container: Option<String>,
    owners: Vec<String>,
    /// Extra content shipped in the bot's `assets/` folder, e.g. `scanline`.
    #[serde(default)]
    packs: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct BotDef {
    pub slug: String,
    pub name: String,
    pub description: Option<String>,
    pub container: String,
    /// Discord user ids allowed to manage this bot.
    pub owners: Vec<String>,
    pub packs: Vec<String>,
    pub dir: PathBuf,
}

impl BotDef {
    pub fn is_owned_by(&self, user_id: &str) -> bool {
        self.owners.iter().any(|owner| owner == user_id)
    }

    /// Bot token from `bots/<slug>/.env`, else from the `DISCORD_TOKEN_<SLUG>`
    /// environment variable (how Coolify and similar platforms pass secrets).
    pub fn token(&self) -> Option<String> {
        self.env_value("DISCORD_TOKEN")
    }

    /// OAuth2 client secret of the bot's application, from the same places
    /// (`DISCORD_CLIENT_SECRET_<SLUG>`). Only needed for the verification in a
    /// Discord Activity.
    pub fn client_secret(&self) -> Option<String> {
        self.env_value("DISCORD_CLIENT_SECRET")
    }

    fn env_value(&self, key: &str) -> Option<String> {
        read_env(&self.dir, key)
    }
}

pub fn is_valid_slug(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= SLUG_MAX_LEN
        && slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !slug.starts_with('-')
}

/// Loads every bot of `dir`. Invalid folders are skipped and reported in the
/// second value so one broken file never hides the other bots.
pub fn load(dir: &Path) -> (Vec<BotDef>, Vec<String>) {
    let mut bots = Vec::new();
    let mut errors = Vec::new();
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => {
            errors.push(format!("{}: {error}", dir.display()));
            return (bots, errors);
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Some(slug) = path.file_name().and_then(|n| n.to_str()).map(str::to_owned) else {
            continue;
        };
        if !path.is_dir() || slug.starts_with('_') || slug.starts_with('.') {
            continue;
        }
        match load_one(&path, &slug) {
            Ok(bot) => bots.push(bot),
            Err(error) => errors.push(format!("bots/{slug}: {error}")),
        }
    }
    bots.sort_by_key(|bot| bot.name.to_lowercase());
    (bots, errors)
}

/// Loads a single bot by slug. `Ok(None)` when no such bot exists.
pub fn find(dir: &Path, slug: &str) -> Result<Option<BotDef>, String> {
    if !is_valid_slug(slug) {
        return Ok(None);
    }
    let path = dir.join(slug);
    if !path.join("bot.toml").is_file() {
        return Ok(None);
    }
    load_one(&path, slug).map(Some)
}

/// Loads the bot defined in the folder `path`.
pub fn load_one(path: &Path, slug: &str) -> Result<BotDef, String> {
    if !is_valid_slug(slug) {
        return Err("folder name must use a-z, 0-9 and '-' only (32 chars max)".into());
    }
    let raw = fs::read_to_string(path.join("bot.toml")).map_err(|e| format!("bot.toml: {e}"))?;
    let file: BotFile = toml::from_str(&raw).map_err(|e| format!("bot.toml: {e}"))?;

    let name = file.name.trim().to_owned();
    if name.is_empty() {
        return Err("`name` must not be empty".into());
    }
    if let Some(bad) = file.owners.iter().find(|id| !is_snowflake(id)) {
        return Err(format!("owner `{bad}` is not a Discord user id"));
    }
    let container = file.container.unwrap_or_else(|| format!("dc-bot-{slug}"));
    let container_ok = !container.is_empty()
        && container
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'));
    if !container_ok {
        return Err(format!("invalid container name `{container}`"));
    }

    Ok(BotDef {
        slug: slug.to_owned(),
        name,
        description: file.description.filter(|d| !d.trim().is_empty()),
        container,
        owners: with_extra_owners(file.owners, std::env::var(env_name("OWNERS", slug)).ok()),
        packs: file.packs,
        dir: path.to_owned(),
    })
}

/// Folder of a bot holding the files it sends (filters, documents...).
pub const ASSETS_DIR: &str = "assets";

/// Copies a folder and its sub-folders, replacing files already there.
fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)?.flatten() {
        let path = entry.path();
        let target = to.join(entry.file_name());
        if path.is_dir() {
            copy_tree(&path, &target)?;
        } else if path.is_file() {
            fs::copy(&path, &target)?;
        }
    }
    Ok(())
}

/// `OWNERS` + `my-bot` -> `OWNERS_MY_BOT`.
pub fn env_name(key: &str, slug: &str) -> String {
    format!("{key}_{}", slug.to_uppercase().replace('-', "_"))
}

/// Owners from `bot.toml` plus those of the `OWNERS_<SLUG>` variable
/// (comma-separated ids), so access can be given without a commit.
fn with_extra_owners(mut owners: Vec<String>, extra: Option<String>) -> Vec<String> {
    let extra = extra.unwrap_or_default();
    for id in extra.split(',').map(str::trim) {
        if is_snowflake(id) && !owners.iter().any(|owner| owner == id) {
            owners.push(id.to_owned());
        }
    }
    owners
}

/// Reads one secret of a bot: `KEY` in the `.env` of its folder, else the
/// `KEY_<SLUG>` environment variable (the slug is the folder name).
pub fn read_env(bot_dir: &Path, key: &str) -> Option<String> {
    let from_file = fs::read_to_string(bot_dir.join(".env"))
        .ok()
        .and_then(|content| read_env_value(&content, key));
    from_file.or_else(|| {
        let slug = bot_dir
            .canonicalize()
            .ok()?
            .file_name()?
            .to_str()?
            .to_owned();
        std::env::var(env_name(key, &slug))
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
    })
}

/// Copies `<seed>/<slug>/bot.toml` into `<bots_dir>/<slug>/` for every bot
/// (or only `only`). Used in containers: the definitions come with the
/// image, while the bot folders live in a volume shared by the panel and the
/// bots. The image's copy always wins, so the repository stays the source of
/// truth. Returns the slugs copied.
pub fn seed(seed_dir: &Path, bots_dir: &Path, only: Option<&str>) -> std::io::Result<Vec<String>> {
    let mut copied = Vec::new();
    for entry in fs::read_dir(seed_dir)?.flatten() {
        let Some(slug) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let source = entry.path().join("bot.toml");
        let wanted = only.is_none_or(|only| only == slug);
        if !wanted || !is_valid_slug(&slug) || !source.is_file() {
            continue;
        }
        let target = bots_dir.join(&slug);
        fs::create_dir_all(&target)?;
        fs::copy(&source, target.join("bot.toml"))?;
        let assets = entry.path().join(ASSETS_DIR);
        if assets.is_dir() {
            copy_tree(&assets, &target.join(ASSETS_DIR))?;
        }
        copied.push(slug);
    }
    copied.sort();
    Ok(copied)
}

/// Reads `KEY=value` from a dotenv-style file (quotes optional).
fn read_env_value(content: &str, key: &str) -> Option<String> {
    content.lines().find_map(|line| {
        let line = line.trim();
        let line = line.strip_prefix("export ").unwrap_or(line);
        let (k, v) = line.split_once('=')?;
        if k.trim() != key {
            return None;
        }
        let v = v.trim().trim_matches(|c| c == '"' || c == '\'');
        (!v.is_empty()).then(|| v.to_owned())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dc-core-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_bot(root: &Path, slug: &str, toml: &str) {
        fs::create_dir_all(root.join(slug)).unwrap();
        fs::write(root.join(slug).join("bot.toml"), toml).unwrap();
    }

    #[test]
    fn slugs() {
        assert!(is_valid_slug("music-bot2"));
        assert!(!is_valid_slug("Music"));
        assert!(!is_valid_slug("-a"));
        assert!(!is_valid_slug("a/b"));
        assert!(!is_valid_slug(""));
    }

    #[test]
    fn loads_valid_and_reports_invalid() {
        let root = temp_dir("load");
        write_bot(
            &root,
            "music",
            "name = \"Music\"\nowners = [\"175928847299117063\"]\n",
        );
        write_bot(&root, "broken", "name = \"Broken\"\nowners = [\"abc\"]\n");
        write_bot(&root, "typo", "name = \"Typo\"\nowner = []\n");
        write_bot(&root, "_template", "nonsense");
        fs::write(
            root.join("music").join(".env"),
            "# c\nDISCORD_TOKEN=\"abc.def\"\n",
        )
        .unwrap();

        let (bots, errors) = load(&root);
        assert_eq!(bots.len(), 1);
        assert_eq!(bots[0].container, "dc-bot-music");
        assert!(bots[0].is_owned_by("175928847299117063"));
        assert!(!bots[0].is_owned_by("1"));
        assert_eq!(bots[0].token().as_deref(), Some("abc.def"));
        assert_eq!(errors.len(), 2, "{errors:?}");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn extra_owners() {
        let base = vec!["175928847299117063".to_owned()];
        let extra = Some(" 175928847299117064, nope,175928847299117063 ".to_owned());
        assert_eq!(
            with_extra_owners(base.clone(), extra),
            vec!["175928847299117063", "175928847299117064"]
        );
        assert_eq!(with_extra_owners(base.clone(), None), base);
        assert_eq!(env_name("OWNERS", "my-bot"), "OWNERS_MY_BOT");
    }

    #[test]
    fn secrets_fall_back_to_the_environment() {
        let root = temp_dir("secrets");
        let dir = root.join("envbot");
        fs::create_dir_all(&dir).unwrap();
        assert_eq!(read_env(&dir, "DISCORD_TOKEN"), None);
        // SAFETY: this test is the only one using this variable.
        unsafe { std::env::set_var("DISCORD_TOKEN_ENVBOT", "from.env.var") };
        assert_eq!(
            read_env(&dir, "DISCORD_TOKEN").as_deref(),
            Some("from.env.var")
        );
        fs::write(dir.join(".env"), "DISCORD_TOKEN=from.file\n").unwrap();
        assert_eq!(
            read_env(&dir, "DISCORD_TOKEN").as_deref(),
            Some("from.file")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn seeding() {
        let seed_dir = temp_dir("seed-src");
        let bots = temp_dir("seed-dst");
        write_bot(&seed_dir, "sang", "name = \"Sang\"\nowners = []\n");
        fs::create_dir_all(seed_dir.join("sang").join("assets").join("filters")).unwrap();
        fs::write(
            seed_dir
                .join("sang")
                .join("assets")
                .join("filters")
                .join("a.json"),
            "{}",
        )
        .unwrap();
        write_bot(&seed_dir, "music", "name = \"Music\"\nowners = []\n");
        fs::create_dir_all(seed_dir.join("_template")).unwrap();
        fs::write(seed_dir.join("_template").join("bot.toml"), "x").unwrap();

        assert_eq!(seed(&seed_dir, &bots, Some("sang")).unwrap(), vec!["sang"]);
        assert!(!bots.join("music").exists());
        // Existing data is kept, bot.toml is refreshed.
        fs::create_dir_all(bots.join("sang").join("guilds")).unwrap();
        write_bot(&seed_dir, "sang", "name = \"Sang 2\"\nowners = []\n");
        assert_eq!(seed(&seed_dir, &bots, None).unwrap(), vec!["music", "sang"]);
        assert!(bots.join("sang").join("guilds").is_dir());
        assert!(
            bots.join("sang")
                .join("assets")
                .join("filters")
                .join("a.json")
                .is_file()
        );
        let toml = fs::read_to_string(bots.join("sang").join("bot.toml")).unwrap();
        assert!(toml.contains("Sang 2"));
        fs::remove_dir_all(seed_dir).unwrap();
        fs::remove_dir_all(bots).unwrap();
    }

    #[test]
    fn env_values() {
        let env = "A=1\nexport DISCORD_TOKEN = 'x.y'\nEMPTY=\n";
        assert_eq!(read_env_value(env, "DISCORD_TOKEN").as_deref(), Some("x.y"));
        assert_eq!(read_env_value(env, "EMPTY"), None);
        assert_eq!(read_env_value(env, "MISSING"), None);
    }
}
