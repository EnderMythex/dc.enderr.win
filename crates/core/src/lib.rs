//! Shared foundation for the panel and the bot.

pub mod cards;
pub mod commands;
pub mod crypto;
pub mod db;
pub mod guild;
pub mod misty;
pub mod registry;
pub mod render;
pub mod requests;
pub mod settings;
pub mod tips;
pub mod verify_link;

use std::time::{SystemTime, UNIX_EPOCH};

/// Current Unix timestamp, in seconds.
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// True for a Discord id (user, server, channel, role...).
pub fn is_snowflake(id: &str) -> bool {
    (15..=21).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_digit())
}

/// 12345 -> "12,345".
pub fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts() {
        assert_eq!(thousands(7), "7");
        assert_eq!(thousands(1_000), "1,000");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }
}
