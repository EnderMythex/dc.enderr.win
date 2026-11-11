//! Totals shown on the home page: servers the bots are in and their members.
//!
//! Computed in the background every few minutes, never on a page view, so a
//! public page cannot make the panel hammer Discord's API.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use dc_core::registry;

use crate::AppState;

const REFRESH: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Totals {
    pub servers: usize,
    pub members: u64,
}

#[derive(Clone, Default)]
pub struct Stats(Arc<Mutex<Option<Totals>>>);

impl Stats {
    pub fn get(&self) -> Option<Totals> {
        *self.0.lock().unwrap()
    }
}

/// Members counted once per server, even when several bots are on it.
fn total(per_server: &HashMap<String, u64>) -> Totals {
    Totals {
        servers: per_server.len(),
        members: per_server.values().sum(),
    }
}

async fn compute(state: &AppState) -> Option<Totals> {
    let (bots, _) = registry::load(&state.cfg.bots_dir);
    let mut per_server: HashMap<String, u64> = HashMap::new();
    let mut answered = false;
    for bot in bots {
        let Some(token) = bot.token() else { continue };
        match state.discord.bot_guilds(&token).await {
            Ok(guilds) => {
                answered = true;
                for guild in guilds {
                    let members = guild.approximate_member_count.unwrap_or(0);
                    let entry = per_server.entry(guild.id).or_default();
                    *entry = (*entry).max(members);
                }
            }
            Err(error) => tracing::warn!(bot = %bot.slug, %error, "counting servers failed"),
        }
    }
    // Keep the previous totals rather than showing zero during an outage.
    answered.then(|| total(&per_server))
}

/// Background task refreshing the totals.
pub async fn watch(state: AppState) {
    loop {
        if let Some(totals) = compute(&state).await {
            *state.stats.0.lock().unwrap() = Some(totals);
        }
        tokio::time::sleep(REFRESH).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn servers_are_counted_once() {
        let per_server = HashMap::from([("a".to_owned(), 120), ("b".to_owned(), 30)]);
        assert_eq!(
            total(&per_server),
            Totals {
                servers: 2,
                members: 150
            }
        );
    }
}
