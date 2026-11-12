//! Bot-wide settings (its Discord presence, the darkening of its cards'
//! background picture), stored in
//! `bots/<slug>/settings.json` and applied live by the bot.

use std::{fs, io, path::Path};

use serde::{Deserialize, Serialize};

pub const FILE_NAME: &str = "settings.json";

pub const PRESENCE_STATUSES: &[&str] = &["online", "idle", "dnd", "invisible"];
pub const ACTIVITY_TYPES: &[&str] = &[
    "none",
    "playing",
    "watching",
    "listening",
    "competing",
    "custom",
    // "Watching SOL: $123.45", refreshed every 10 minutes by the bot.
    "sol_price",
    // "Watching 1,234 players · 0.3.7" (or downloads), like the Misty bot:
    // from the status panel settings, refreshed every 5 minutes.
    "project_stats",
];

pub const ACTIVITY_TEXT_MAX_CHARS: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BotSettings {
    pub presence_status: String,
    pub activity_type: String,
    pub activity_text: String,
    /// Darkening of the background picture of the cards (0 to 90 %).
    pub background_dim: u8,
}

/// Default darkening of the background picture.
pub const BACKGROUND_DIM: u8 = 55;

impl Default for BotSettings {
    fn default() -> Self {
        Self {
            presence_status: "online".into(),
            activity_type: "none".into(),
            activity_text: String::new(),
            background_dim: BACKGROUND_DIM,
        }
    }
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum SettingsError {
    #[error("Unknown presence status.")]
    Presence,
    #[error("Unknown activity type.")]
    ActivityType,
    #[error("The activity text cannot exceed 128 characters.")]
    ActivityText,
}

impl BotSettings {
    /// Normalizes and validates user input.
    pub fn parse(
        presence_status: &str,
        activity_type: &str,
        activity_text: &str,
    ) -> Result<Self, SettingsError> {
        if !PRESENCE_STATUSES.contains(&presence_status) {
            return Err(SettingsError::Presence);
        }
        if !ACTIVITY_TYPES.contains(&activity_type) {
            return Err(SettingsError::ActivityType);
        }
        let activity_text = activity_text.trim();
        if activity_text.chars().count() > ACTIVITY_TEXT_MAX_CHARS
            || activity_text.chars().any(|c| c.is_control())
        {
            return Err(SettingsError::ActivityText);
        }
        Ok(Self {
            presence_status: presence_status.to_owned(),
            activity_type: activity_type.to_owned(),
            activity_text: activity_text.to_owned(),
            ..Self::default()
        })
    }

    /// Reads the settings of a bot folder; defaults when the file is missing.
    pub fn load(bot_dir: &Path) -> io::Result<Self> {
        match fs::read_to_string(bot_dir.join(FILE_NAME)) {
            Ok(raw) => serde_json::from_str(&raw).map_err(io::Error::other),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error),
        }
    }

    /// Writes the settings atomically (temp file + rename), so the bot never
    /// reads a half-written file.
    pub fn save(&self, bot_dir: &Path) -> io::Result<()> {
        let json = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
        let tmp = bot_dir.join(format!(".{FILE_NAME}.tmp"));
        fs::write(&tmp, json + "\n")?;
        fs::rename(&tmp, bot_dir.join(FILE_NAME))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation() {
        let ok = BotSettings::parse("idle", "playing", "  Minecraft ").unwrap();
        assert_eq!(ok.activity_text, "Minecraft");

        let err = |s, t, x| BotSettings::parse(s, t, x).unwrap_err();
        assert_eq!(err("away", "none", ""), SettingsError::Presence);
        assert_eq!(err("idle", "dancing", ""), SettingsError::ActivityType);
        let long = "x".repeat(129);
        assert_eq!(err("idle", "playing", &long), SettingsError::ActivityText);
    }

    #[test]
    fn save_and_load() {
        let dir = std::env::temp_dir().join(format!("dc-core-settings-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let _ = fs::remove_file(dir.join(FILE_NAME));

        assert_eq!(BotSettings::load(&dir).unwrap(), BotSettings::default());
        let custom = BotSettings::parse("dnd", "watching", "you").unwrap();
        custom.save(&dir).unwrap();
        assert_eq!(BotSettings::load(&dir).unwrap(), custom);
        fs::remove_dir_all(dir).unwrap();
    }
}
