//! One-off requests from the panel to a running bot, exchanged as files in
//! `bots/<slug>/requests/`:
//!
//! 1. the panel writes `<id>.json`;
//! 2. the bot (which polls the folder) takes it, acts, and writes `<id>.done`;
//! 3. the panel reads the answer and removes it.
//!
//! Both sides already share the bot folder, so no network port is needed.

use std::{
    fs, io,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use serde::{Deserialize, Serialize};

use crate::crypto::random_token;

pub const DIR_NAME: &str = "requests";

/// Files older than this were abandoned by one side and are deleted.
const STALE_AFTER: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Request {
    /// Send a sample of what a section posts (welcome message, panels, ...).
    Test {
        guild_id: String,
        module: String,
        /// The dashboard user, used as the sample member.
        user_id: String,
    },
    /// A member passed the verification in the Discord Activity.
    Verify { guild_id: String, user_id: String },
    /// Erase every member's XP on a server.
    ResetXp { guild_id: String },
}

/// The bot's answer: a short sentence shown on the dashboard.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answer {
    pub ok: bool,
    pub message: String,
}

fn dir(bot_dir: &Path) -> PathBuf {
    bot_dir.join(DIR_NAME)
}

fn is_stale(path: &Path) -> bool {
    fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .is_some_and(|age| age > STALE_AFTER)
}

/// Writes atomically, so the other side never reads half a file.
fn write(dir: &Path, name: &str, json: String) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".{name}.tmp"));
    fs::write(&tmp, json)?;
    fs::rename(tmp, dir.join(name))
}

/// Panel side: queues a request and returns its id.
pub fn submit(bot_dir: &Path, request: &Request) -> io::Result<String> {
    let id = random_token(12);
    let json = serde_json::to_string(request).map_err(io::Error::other)?;
    write(&dir(bot_dir), &format!("{id}.json"), json)?;
    Ok(id)
}

/// Panel side: the bot's answer, once it exists. Reading removes it.
pub fn take_answer(bot_dir: &Path, id: &str) -> Option<Answer> {
    let path = dir(bot_dir).join(format!("{id}.done"));
    let raw = fs::read_to_string(&path).ok()?;
    let _ = fs::remove_file(path);
    serde_json::from_str(&raw).ok()
}

/// Panel side: gives up on a request the bot never picked up.
pub fn cancel(bot_dir: &Path, id: &str) {
    let _ = fs::remove_file(dir(bot_dir).join(format!("{id}.json")));
}

/// Bot side: takes every waiting request. Requests left over from a time the
/// bot was not running are dropped rather than run late.
pub fn take_pending(bot_dir: &Path) -> Vec<(String, Request)> {
    let Ok(entries) = fs::read_dir(dir(bot_dir)) else {
        return Vec::new();
    };
    let mut pending = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if is_stale(&path) {
            let _ = fs::remove_file(&path);
            continue;
        }
        let Some(id) = name.strip_suffix(".json").filter(|id| !id.starts_with('.')) else {
            continue;
        };
        let request = fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok());
        let _ = fs::remove_file(&path);
        if let Some(request) = request {
            pending.push((id.to_owned(), request));
        }
    }
    pending
}

/// Bot side: answers a request.
pub fn answer(bot_dir: &Path, id: &str, result: Result<String, String>) -> io::Result<()> {
    let (ok, message) = match result {
        Ok(message) => (true, message),
        Err(message) => (false, message),
    };
    let json = serde_json::to_string(&Answer { ok, message }).map_err(io::Error::other)?;
    write(&dir(bot_dir), &format!("{id}.done"), json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let bot = std::env::temp_dir().join(format!("dc-core-requests-{}", std::process::id()));
        let _ = fs::remove_dir_all(&bot);
        assert!(take_pending(&bot).is_empty());

        let request = Request::Test {
            guild_id: "175928847299117063".into(),
            module: "welcome".into(),
            user_id: "175928847299117064".into(),
        };
        let id = submit(&bot, &request).unwrap();
        assert_eq!(take_answer(&bot, &id), None);

        let pending = take_pending(&bot);
        assert_eq!(pending, vec![(id.clone(), request)]);
        // Taken once only.
        assert!(take_pending(&bot).is_empty());

        answer(&bot, &id, Err("no channel".into())).unwrap();
        let reply = take_answer(&bot, &id).unwrap();
        assert!(!reply.ok);
        assert_eq!(reply.message, "no channel");
        assert_eq!(take_answer(&bot, &id), None);

        let cancelled = submit(
            &bot,
            &Request::ResetXp {
                guild_id: "1".into(),
            },
        )
        .unwrap();
        cancel(&bot, &cancelled);
        assert!(take_pending(&bot).is_empty());
        fs::remove_dir_all(bot).unwrap();
    }

    #[test]
    fn unreadable_requests_are_dropped() {
        let bot = std::env::temp_dir().join(format!("dc-core-requests-bad-{}", std::process::id()));
        let _ = fs::remove_dir_all(&bot);
        fs::create_dir_all(dir(&bot)).unwrap();
        fs::write(dir(&bot).join("x.json"), "{\"kind\":\"format_disk\"}").unwrap();
        assert!(take_pending(&bot).is_empty());
        assert!(!dir(&bot).join("x.json").exists());
        fs::remove_dir_all(bot).unwrap();
    }
}
