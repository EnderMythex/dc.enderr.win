//! `/role add-all` and `/role remove-all`: a role given to, or taken from,
//! every member of a server. Runs in the background after confirmation, with
//! a progress message in the channel the command was used in.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use dc_core::cards::Tone;
use serde_json::Value;

use crate::{
    api::ApiError,
    app::{App, roles_of, str_of},
    logs,
    ui::{self, Msg},
};

/// Members fetched per request (Discord's maximum).
const PAGE: usize = 1000;
/// Pause between two role changes, to stay under Discord's rate limits.
const PACE: Duration = Duration::from_millis(250);
/// How often the progress message is refreshed.
const PROGRESS_EVERY: Duration = Duration::from_secs(4);
/// Attempts per member when Discord asks to slow down.
const RATE_LIMIT_RETRIES: u32 = 5;

/// What the job does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    pub guild_id: String,
    pub role_id: String,
    pub add: bool,
    pub include_bots: bool,
    /// Where the progress message is posted.
    pub channel_id: String,
    pub by: String,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Counts {
    changed: u64,
    unchanged: u64,
    failed: u64,
}

/// Members to change: `(user id, needs the change)`, bots left out unless
/// asked for.
fn targets(members: &[Value], job: &Job) -> Vec<(String, bool)> {
    members
        .iter()
        .filter(|member| job.include_bots || member["user"]["bot"] != true)
        .map(|member| {
            let has = roles_of(member).contains(&job.role_id);
            (str_of(&member["user"], "id").to_owned(), has != job.add)
        })
        .filter(|(id, _)| !id.is_empty())
        .collect()
}

async fn list_members(app: &App, guild_id: &str) -> Result<Vec<Value>, ApiError> {
    let mut all = Vec::new();
    let mut after = "0".to_owned();
    loop {
        let page = app
            .api
            .get(&format!(
                "/guilds/{guild_id}/members?limit={PAGE}&after={after}"
            ))
            .await?;
        let page = page.as_array().cloned().unwrap_or_default();
        let count = page.len();
        if let Some(last) = page.last() {
            after = str_of(&last["user"], "id").to_owned();
        }
        all.extend(page);
        if count < PAGE {
            return Ok(all);
        }
    }
}

fn progress_message(job: &Job, role_name: &str, done: usize, total: usize, counts: Counts) -> Msg {
    let verb = if job.add { "Giving" } else { "Taking" };
    let preposition = if job.add { "to" } else { "from" };
    let body = format!(
        "{done} of {total} members checked. {} changed, {} failed.",
        counts.changed, counts.failed
    );
    Msg::banner(
        Tone::Info,
        &format!("{verb} {role_name} {preposition} everyone"),
        &body,
        vec![ui::text(format!("-# Started by <@{}>", job.by))],
    )
}

fn summary(job: &Job, counts: Counts) -> String {
    let (verb, already) = if job.add {
        ("now have", "already had it")
    } else {
        ("no longer have", "did not have it")
    };
    let mut text = format!(
        "**{}** member(s) {verb} <@&{}>.\n**{}** {already}.",
        counts.changed, job.role_id, counts.unchanged
    );
    if counts.failed > 0 {
        text.push_str(&format!(
            "\n**{}** could not be changed (they may have left the server).",
            counts.failed
        ));
    }
    text.push_str(&format!("\n**By:** <@{}>", job.by));
    text
}

/// One role change, retried while Discord asks to slow down.
async fn change(app: &App, job: &Job, user_id: &str, reason: &str) -> Result<(), ApiError> {
    let mut attempt = 0;
    loop {
        let result = if job.add {
            app.api
                .add_role(&job.guild_id, user_id, &job.role_id, reason)
                .await
        } else {
            app.api
                .remove_role(&job.guild_id, user_id, &job.role_id, reason)
                .await
        };
        match result {
            Err(ApiError::Status { status: 429, .. }) if attempt < RATE_LIMIT_RETRIES => {
                attempt += 1;
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
            other => return other.map(|_| ()),
        }
    }
}

/// Runs a confirmed job. Only one runs per server at a time (see
/// `App::mass_roles`), which the caller reserved.
pub async fn run(app: Arc<App>, job: Job) {
    let role_name = app.role_name(&job.guild_id, &job.role_id);
    let reason = format!(
        "{} for everyone, requested by {}",
        if job.add { "Role given" } else { "Role taken" },
        job.by
    );

    let progress = app
        .api
        .send_message(
            &job.channel_id,
            &progress_message(&job, &role_name, 0, 0, Counts::default()),
        )
        .await
        .ok()
        .map(|sent| str_of(&sent, "id").to_owned());
    let show = async |msg: Msg| match &progress {
        Some(id) => {
            let _ = app.api.edit_message(&job.channel_id, id, &msg).await;
        }
        None => {
            let _ = app.api.send_message(&job.channel_id, &msg).await;
        }
    };

    let outcome = async {
        let members = list_members(&app, &job.guild_id)
            .await
            .map_err(|error| match error {
                ApiError::Status { status: 403, .. } => "I cannot list the members. Enable Server \
                 Members Intent for this bot on the Discord developer portal."
                    .to_owned(),
                other => other.user_message(),
            })?;
        let targets = targets(&members, &job);
        let total = targets.len();
        let mut counts = Counts::default();
        let mut last_update = Instant::now();
        for (index, (user_id, needs_change)) in targets.iter().enumerate() {
            if !needs_change {
                counts.unchanged += 1;
                continue;
            }
            match change(&app, &job, user_id, &reason).await {
                Ok(()) => counts.changed += 1,
                // Missing permissions fail for everyone: stop at once.
                Err(error @ ApiError::Status { status: 403, .. }) => {
                    return Err(error.user_message());
                }
                Err(error) => {
                    tracing::warn!(guild = %job.guild_id, %error, "mass role change failed");
                    counts.failed += 1;
                }
            }
            if last_update.elapsed() >= PROGRESS_EVERY {
                last_update = Instant::now();
                show(progress_message(&job, &role_name, index + 1, total, counts)).await;
            }
            tokio::time::sleep(PACE).await;
        }
        Ok(counts)
    }
    .await;

    match outcome {
        Ok(counts) => {
            let title = if job.add {
                "Role given to everyone"
            } else {
                "Role taken from everyone"
            };
            let text = summary(&job, counts);
            show(Msg::success(title, &text)).await;
            logs::moderation(&app, &job.guild_id, title, &text).await;
        }
        Err(reason) => {
            let msg = Msg::banner(Tone::Error, "Stopped", &reason, Vec::new());
            show(msg).await;
        }
    }
    app.mass_roles.lock().unwrap().remove(&job.guild_id);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn job(add: bool, include_bots: bool) -> Job {
        Job {
            guild_id: "1".into(),
            role_id: "9".into(),
            add,
            include_bots,
            channel_id: "2".into(),
            by: "3".into(),
        }
    }

    #[test]
    fn who_needs_a_change() {
        let members = [
            json!({ "user": { "id": "10" }, "roles": [] }),
            json!({ "user": { "id": "11" }, "roles": ["9"] }),
            json!({ "user": { "id": "12", "bot": true }, "roles": [] }),
        ];
        assert_eq!(
            targets(&members, &job(true, false)),
            vec![("10".into(), true), ("11".into(), false)]
        );
        assert_eq!(targets(&members, &job(true, true)).len(), 3);
        assert_eq!(
            targets(&members, &job(false, false)),
            vec![("10".into(), false), ("11".into(), true)]
        );
    }

    #[test]
    fn summaries() {
        let counts = Counts {
            changed: 5,
            unchanged: 2,
            failed: 1,
        };
        let text = summary(&job(true, false), counts);
        assert!(text.contains("**5** member(s) now have <@&9>"));
        assert!(text.contains("**1** could not be changed"));
        assert!(!summary(&job(false, false), Counts::default()).contains("could not"));
    }
}
