//! The bot's own SQLite database (`bots/<slug>/data/bot.db`): XP, open
//! tickets and the panels it posted.

use std::path::Path;

use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};

/// Opens (and creates if needed) the database file, then its tables.
pub async fn open(path: &Path) -> Result<SqlitePool, sqlx::Error> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(sqlx::Error::Io)?;
    }
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal);
    let db = SqlitePoolOptions::new()
        .max_connections(4)
        .connect_with(options)
        .await?;
    init(&db).await?;
    Ok(db)
}

pub async fn init(db: &SqlitePool) -> Result<(), sqlx::Error> {
    for statement in [
        "CREATE TABLE IF NOT EXISTS xp (
            guild_id TEXT NOT NULL,
            user_id  TEXT NOT NULL,
            xp       INTEGER NOT NULL DEFAULT 0,
            messages INTEGER NOT NULL DEFAULT 0,
            username TEXT NOT NULL DEFAULT '',
            PRIMARY KEY (guild_id, user_id)
        )",
        "CREATE INDEX IF NOT EXISTS xp_rank ON xp (guild_id, xp DESC)",
        "CREATE TABLE IF NOT EXISTS tickets (
            channel_id TEXT PRIMARY KEY,
            guild_id   TEXT NOT NULL,
            user_id    TEXT NOT NULL,
            number     INTEGER NOT NULL,
            topic      TEXT NOT NULL DEFAULT '',
            claimed_by TEXT,
            opened_at  INTEGER NOT NULL
        )",
        "CREATE TABLE IF NOT EXISTS counters (
            guild_id TEXT NOT NULL,
            name     TEXT NOT NULL,
            value    INTEGER NOT NULL,
            PRIMARY KEY (guild_id, name)
        )",
        "CREATE TABLE IF NOT EXISTS panels (
            guild_id   TEXT NOT NULL,
            kind       TEXT NOT NULL,
            channel_id TEXT NOT NULL,
            message_id TEXT NOT NULL,
            hash       TEXT NOT NULL,
            PRIMARY KEY (guild_id, kind)
        )",
    ] {
        sqlx::query(statement).execute(db).await?;
    }
    Ok(())
}

// ---------------------------------------------------------------- xp

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct XpRow {
    pub user_id: String,
    pub xp: i64,
    /// Display name when the member last earned XP.
    pub username: String,
}

/// Adds XP for one message and returns the total before and after.
pub async fn add_message_xp(
    db: &SqlitePool,
    guild: &str,
    user: &str,
    username: &str,
    amount: i64,
) -> Result<(i64, i64), sqlx::Error> {
    let after: i64 = sqlx::query_scalar(
        "INSERT INTO xp (guild_id, user_id, xp, messages, username) VALUES (?, ?, ?, 1, ?)
         ON CONFLICT (guild_id, user_id) DO UPDATE SET
             xp = xp + excluded.xp, messages = messages + 1, username = excluded.username
         RETURNING xp",
    )
    .bind(guild)
    .bind(user)
    .bind(amount)
    .bind(username)
    .fetch_one(db)
    .await?;
    Ok((after - amount, after))
}

/// Sets a member's XP (admin commands); never below zero.
pub async fn set_xp(db: &SqlitePool, guild: &str, user: &str, xp: i64) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO xp (guild_id, user_id, xp) VALUES (?, ?, ?)
         ON CONFLICT (guild_id, user_id) DO UPDATE SET xp = excluded.xp",
    )
    .bind(guild)
    .bind(user)
    .bind(xp.max(0))
    .execute(db)
    .await?;
    Ok(())
}

pub async fn get_xp(
    db: &SqlitePool,
    guild: &str,
    user: &str,
) -> Result<Option<XpRow>, sqlx::Error> {
    sqlx::query_as("SELECT user_id, xp, username FROM xp WHERE guild_id = ? AND user_id = ?")
        .bind(guild)
        .bind(user)
        .fetch_optional(db)
        .await
}

/// Erases the XP of every member of a server; returns how many had some.
pub async fn reset_xp(db: &SqlitePool, guild: &str) -> Result<u64, sqlx::Error> {
    let done = sqlx::query("DELETE FROM xp WHERE guild_id = ?")
        .bind(guild)
        .execute(db)
        .await?;
    Ok(done.rows_affected())
}

/// 1-based position on the server leaderboard for a given XP total.
pub async fn rank_of(db: &SqlitePool, guild: &str, xp: i64) -> Result<i64, sqlx::Error> {
    let ahead: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM xp WHERE guild_id = ? AND xp > ?")
        .bind(guild)
        .bind(xp)
        .fetch_one(db)
        .await?;
    Ok(ahead + 1)
}

pub async fn top_xp(db: &SqlitePool, guild: &str, limit: i64) -> Result<Vec<XpRow>, sqlx::Error> {
    sqlx::query_as(
        "SELECT user_id, xp, username FROM xp
         WHERE guild_id = ? AND xp > 0 ORDER BY xp DESC, user_id LIMIT ?",
    )
    .bind(guild)
    .bind(limit)
    .fetch_all(db)
    .await
}

// ---------------------------------------------------------------- tickets

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Ticket {
    pub channel_id: String,
    pub user_id: String,
    pub number: i64,
    pub topic: String,
    pub claimed_by: Option<String>,
    pub opened_at: i64,
}

pub async fn next_ticket_number(db: &SqlitePool, guild: &str) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "INSERT INTO counters (guild_id, name, value) VALUES (?, 'ticket', 1)
         ON CONFLICT (guild_id, name) DO UPDATE SET value = value + 1
         RETURNING value",
    )
    .bind(guild)
    .fetch_one(db)
    .await
}

pub async fn insert_ticket(
    db: &SqlitePool,
    guild: &str,
    ticket: &Ticket,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO tickets (channel_id, guild_id, user_id, number, topic, opened_at)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&ticket.channel_id)
    .bind(guild)
    .bind(&ticket.user_id)
    .bind(ticket.number)
    .bind(&ticket.topic)
    .bind(ticket.opened_at)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn get_ticket(db: &SqlitePool, channel: &str) -> Result<Option<Ticket>, sqlx::Error> {
    sqlx::query_as(
        "SELECT channel_id, user_id, number, topic, claimed_by, opened_at
         FROM tickets WHERE channel_id = ?",
    )
    .bind(channel)
    .fetch_optional(db)
    .await
}

pub async fn open_tickets(
    db: &SqlitePool,
    guild: &str,
    user: &str,
) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT channel_id FROM tickets WHERE guild_id = ? AND user_id = ?")
        .bind(guild)
        .bind(user)
        .fetch_all(db)
        .await
}

/// Marks the ticket as claimed. False when someone already claimed it.
pub async fn claim_ticket(
    db: &SqlitePool,
    channel: &str,
    staff: &str,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE tickets SET claimed_by = ? WHERE channel_id = ? AND claimed_by IS NULL",
    )
    .bind(staff)
    .bind(channel)
    .execute(db)
    .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn delete_ticket(db: &SqlitePool, channel: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM tickets WHERE channel_id = ?")
        .bind(channel)
        .execute(db)
        .await?;
    Ok(())
}

// ---------------------------------------------------------------- panels

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Panel {
    pub channel_id: String,
    pub message_id: String,
    pub hash: String,
}

pub async fn get_panel(
    db: &SqlitePool,
    guild: &str,
    kind: &str,
) -> Result<Option<Panel>, sqlx::Error> {
    sqlx::query_as(
        "SELECT channel_id, message_id, hash FROM panels WHERE guild_id = ? AND kind = ?",
    )
    .bind(guild)
    .bind(kind)
    .fetch_optional(db)
    .await
}

pub async fn set_panel(
    db: &SqlitePool,
    guild: &str,
    kind: &str,
    panel: &Panel,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO panels (guild_id, kind, channel_id, message_id, hash) VALUES (?, ?, ?, ?, ?)
         ON CONFLICT (guild_id, kind) DO UPDATE SET
             channel_id = excluded.channel_id, message_id = excluded.message_id, hash = excluded.hash",
    )
    .bind(guild)
    .bind(kind)
    .bind(&panel.channel_id)
    .bind(&panel.message_id)
    .bind(&panel.hash)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn delete_panel(db: &SqlitePool, guild: &str, kind: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM panels WHERE guild_id = ? AND kind = ?")
        .bind(guild)
        .bind(kind)
        .execute(db)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn memory() -> SqlitePool {
        // One connection: each in-memory connection is its own database.
        let db = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        init(&db).await.unwrap();
        db
    }

    #[tokio::test]
    async fn xp_accumulates_and_ranks() {
        let db = memory().await;
        assert_eq!(
            add_message_xp(&db, "g", "a", "Alice", 20).await.unwrap(),
            (0, 20)
        );
        assert_eq!(
            add_message_xp(&db, "g", "a", "Alice", 5).await.unwrap(),
            (20, 25)
        );
        add_message_xp(&db, "g", "b", "Bob", 100).await.unwrap();
        add_message_xp(&db, "other", "c", "Carol", 999)
            .await
            .unwrap();

        let alice = get_xp(&db, "g", "a").await.unwrap().unwrap();
        assert_eq!(alice.xp, 25);
        assert_eq!(rank_of(&db, "g", alice.xp).await.unwrap(), 2);
        let top = top_xp(&db, "g", 10).await.unwrap();
        assert_eq!(
            top.iter().map(|r| r.user_id.as_str()).collect::<Vec<_>>(),
            ["b", "a"]
        );

        set_xp(&db, "g", "b", -5).await.unwrap();
        assert_eq!(get_xp(&db, "g", "b").await.unwrap().unwrap().xp, 0);
    }

    #[tokio::test]
    async fn tickets_lifecycle() {
        let db = memory().await;
        assert_eq!(next_ticket_number(&db, "g").await.unwrap(), 1);
        assert_eq!(next_ticket_number(&db, "g").await.unwrap(), 2);
        let ticket = Ticket {
            channel_id: "c1".into(),
            user_id: "u".into(),
            number: 2,
            topic: "Help".into(),
            claimed_by: None,
            opened_at: 0,
        };
        insert_ticket(&db, "g", &ticket).await.unwrap();
        assert_eq!(open_tickets(&db, "g", "u").await.unwrap(), ["c1"]);
        assert!(claim_ticket(&db, "c1", "staff").await.unwrap());
        assert!(!claim_ticket(&db, "c1", "other").await.unwrap());
        delete_ticket(&db, "c1").await.unwrap();
        assert!(get_ticket(&db, "c1").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn panels_upsert() {
        let db = memory().await;
        let mut panel = Panel {
            channel_id: "c".into(),
            message_id: "m".into(),
            hash: "h1".into(),
        };
        set_panel(&db, "g", "rules", &panel).await.unwrap();
        panel.hash = "h2".into();
        set_panel(&db, "g", "rules", &panel).await.unwrap();
        assert_eq!(
            get_panel(&db, "g", "rules").await.unwrap().unwrap().hash,
            "h2"
        );
        delete_panel(&db, "g", "rules").await.unwrap();
        assert!(get_panel(&db, "g", "rules").await.unwrap().is_none());
    }
}
