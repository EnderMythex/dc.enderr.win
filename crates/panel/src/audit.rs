//! What owners did on the panel, per bot: profile, presence, power actions
//! and server settings. Shown under the bot's console on the Logs tab.

use serde_json::Value;

use crate::{AppState, auth::Session, discord::GuildContext};

/// Entries kept per bot; older ones are dropped.
const KEEP: i64 = 500;
/// Entries shown on the Logs tab.
pub const SHOWN: i64 = 100;

pub struct Entry {
    pub at: i64,
    pub user_id: String,
    pub user_name: String,
    pub action: String,
}

/// Records an action. Never fails: a missing entry must not block the change.
pub async fn record(state: &AppState, session: &Session, bot: &str, action: impl Into<String>) {
    let action: String = action.into();
    let action: String = action.chars().take(1000).collect();
    let user = &session.user;
    let inserted = sqlx::query(
        "INSERT INTO audit (bot, at, user_id, user_name, action) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(bot)
    .bind(dc_core::now())
    .bind(&user.id)
    .bind(user.display_name())
    .bind(&action)
    .execute(&state.db)
    .await;
    if let Err(error) = inserted {
        tracing::warn!(%error, bot, "panel activity not recorded");
        return;
    }
    let pruned = sqlx::query(
        "DELETE FROM audit WHERE bot = ? AND id <= \
         (SELECT id FROM audit WHERE bot = ? ORDER BY id DESC LIMIT 1 OFFSET ?)",
    )
    .bind(bot)
    .bind(bot)
    .bind(KEEP)
    .execute(&state.db)
    .await;
    if let Err(error) = pruned {
        tracing::warn!(%error, bot, "old panel activity not pruned");
    }
}

/// The latest entries of a bot, newest first.
pub async fn recent(state: &AppState, bot: &str) -> Result<Vec<Entry>, sqlx::Error> {
    let rows: Vec<(i64, String, String, String)> = sqlx::query_as(
        "SELECT at, user_id, user_name, action FROM audit WHERE bot = ? ORDER BY id DESC LIMIT ?",
    )
    .bind(bot)
    .bind(SHOWN)
    .fetch_all(&state.db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(at, user_id, user_name, action)| Entry {
            at,
            user_id,
            user_name,
            action,
        })
        .collect())
}

/// `auto_roles` → `auto roles`.
fn human(key: &str) -> String {
    key.replace('_', " ")
}

/// A channel or role id as its name, other values as short text.
fn show(value: &Value, context: &GuildContext) -> Option<String> {
    match value {
        Value::Null => Some("none".into()),
        Value::Bool(on) => Some(if *on { "on" } else { "off" }.into()),
        Value::Number(number) => Some(number.to_string()),
        Value::String(text) if dc_core::is_snowflake(text) => {
            if let Some(channel) = context.channels.iter().find(|c| &c.id == text) {
                Some(format!("#{}", channel.name))
            } else if let Some(role) = context.roles.iter().find(|r| &r.id == text) {
                Some(format!("@{}", role.name))
            } else {
                Some(text.clone())
            }
        }
        Value::String(text) if text.chars().count() <= 40 && !text.contains('\n') => {
            Some(format!("“{text}”"))
        }
        _ => None,
    }
}

/// The fields that differ between two versions of a server's settings,
/// e.g. `welcome: enabled on, channel → #general, message changed`.
pub fn changes(before: &Value, after: &Value, context: &GuildContext) -> Vec<String> {
    let (Some(before), Some(after)) = (before.as_object(), after.as_object()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (section, new) in after {
        let old = before.get(section).unwrap_or(&Value::Null);
        if old == new {
            continue;
        }
        let mut fields = Vec::new();
        match (old.as_object(), new.as_object()) {
            (Some(old), Some(new)) => {
                // On / off first, the rest in name order.
                let mut keys: Vec<&String> = new.keys().collect();
                keys.sort_by_key(|key| key.as_str() != "enabled");
                for key in keys {
                    let value = &new[key];
                    let previous = old.get(key).unwrap_or(&Value::Null);
                    if previous == value {
                        continue;
                    }
                    let name = human(key);
                    fields.push(match (value, show(value, context)) {
                        (Value::Bool(_), Some(shown)) => format!("{name} {shown}"),
                        (Value::Array(items), _) => {
                            let added = items
                                .iter()
                                .filter(|item| {
                                    !previous.as_array().is_some_and(|p| p.contains(item))
                                })
                                .filter_map(|item| show(item, context))
                                .collect::<Vec<_>>();
                            let removed = previous
                                .as_array()
                                .map(|p| {
                                    p.iter()
                                        .filter(|item| !items.contains(item))
                                        .filter_map(|item| show(item, context))
                                        .collect::<Vec<_>>()
                                })
                                .unwrap_or_default();
                            let mut parts = Vec::new();
                            if !added.is_empty() {
                                parts.push(format!("+{}", added.join(", +")));
                            }
                            if !removed.is_empty() {
                                parts.push(format!("−{}", removed.join(", −")));
                            }
                            if parts.is_empty() {
                                format!("{name} changed")
                            } else {
                                format!("{name} {}", parts.join(" "))
                            }
                        }
                        (_, Some(shown)) => format!("{name} → {shown}"),
                        _ => format!("{name} changed"),
                    });
                }
            }
            _ => fields.push("changed".into()),
        }
        if !fields.is_empty() {
            out.push(format!("{}: {}", human(section), fields.join(", ")));
        }
    }
    out
}

/// `2026-11-12 10:12 UTC`.
pub fn format_time(at: i64) -> String {
    time::OffsetDateTime::from_unix_timestamp(at)
        .map(|date| {
            format!(
                "{}-{:02}-{:02} {:02}:{:02} UTC",
                date.year(),
                u8::from(date.month()),
                date.day(),
                date.hour(),
                date.minute()
            )
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discord::Role;
    use serde_json::json;

    fn context() -> GuildContext {
        GuildContext {
            guild: serde_json::from_value(json!({ "id": "1", "name": "Test", "icon": null }))
                .unwrap(),
            channels: vec![
                serde_json::from_value(
                    json!({ "id": "175928847299117063", "name": "general", "type": 0 }),
                )
                .unwrap(),
            ],
            roles: vec![Role {
                id: "175928847299117064".into(),
                name: "Member".into(),
                position: 1,
                managed: false,
                permissions: "0".into(),
            }],
        }
    }

    #[test]
    fn describes_changes() {
        let before = json!({
            "welcome": { "enabled": false, "channel": null, "auto_roles": [], "message": "Hi" },
            "logs": { "enabled": true },
        });
        let after = json!({
            "welcome": {
                "enabled": true,
                "channel": "175928847299117063",
                "auto_roles": ["175928847299117064"],
                "message": "Hello\nthere",
            },
            "logs": { "enabled": true },
        });
        assert_eq!(
            changes(&before, &after, &context()),
            vec!["welcome: enabled on, auto roles +@Member, channel → #general, message changed"]
        );
        assert!(changes(&after, &after, &context()).is_empty());
    }

    #[test]
    fn times() {
        assert_eq!(format_time(0), "1970-01-01 00:00 UTC");
    }
}
