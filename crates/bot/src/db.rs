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
        "CREATE TABLE IF NOT EXISTS giveaways (
            guild_id      TEXT NOT NULL,
            number        INTEGER NOT NULL,
            channel_id    TEXT NOT NULL,
            message_id    TEXT NOT NULL DEFAULT '',
            prize         TEXT NOT NULL,
            winners       INTEGER NOT NULL,
            ends_at       INTEGER NOT NULL,
            host_id       TEXT NOT NULL,
            required_role TEXT,
            ended         INTEGER NOT NULL DEFAULT 0,
            winner_ids    TEXT NOT NULL DEFAULT '',
            PRIMARY KEY (guild_id, number)
        )",
        "CREATE INDEX IF NOT EXISTS giveaways_due ON giveaways (ended, ends_at)",
        "CREATE TABLE IF NOT EXISTS giveaway_entries (
            guild_id TEXT NOT NULL,
            number   INTEGER NOT NULL,
            user_id  TEXT NOT NULL,
            PRIMARY KEY (guild_id, number, user_id)
        )",
        "CREATE TABLE IF NOT EXISTS link_reviews (
            id         TEXT PRIMARY KEY,
            guild_id   TEXT NOT NULL,
            user_id    TEXT NOT NULL,
            channel_id TEXT NOT NULL,
            content    TEXT NOT NULL
        )",
        "CREATE TABLE IF NOT EXISTS panels (
            guild_id   TEXT NOT NULL,
            kind       TEXT NOT NULL,
            channel_id TEXT NOT NULL,
            message_id TEXT NOT NULL,
            hash       TEXT NOT NULL,
            PRIMARY KEY (guild_id, kind)
        )",
        "CREATE TABLE IF NOT EXISTS todos (
            guild_id   TEXT NOT NULL,
            number     INTEGER NOT NULL,
            title      TEXT NOT NULL,
            details    TEXT NOT NULL DEFAULT '',
            author_id  TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            claimed_by TEXT,
            claimed_at INTEGER,
            done       INTEGER NOT NULL DEFAULT 0,
            done_by    TEXT,
            done_at    INTEGER,
            PRIMARY KEY (guild_id, number)
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
    async fn link_reviews_and_counters() {
        let db = memory().await;
        let review = LinkReview {
            id: "r1".into(),
            guild_id: "g".into(),
            user_id: "u".into(),
            channel_id: "c".into(),
            content: "https://site.example".into(),
        };
        save_link_review(&db, &review).await.unwrap();
        assert_eq!(take_link_review(&db, "r1").await.unwrap(), Some(review));
        // Taken once only.
        assert_eq!(take_link_review(&db, "r1").await.unwrap(), None);

        assert_eq!(get_counter(&db, "g", "reminder").await.unwrap(), 0);
        set_counter(&db, "g", "reminder", 42).await.unwrap();
        set_counter(&db, "g", "reminder", 43).await.unwrap();
        assert_eq!(get_counter(&db, "g", "reminder").await.unwrap(), 43);
    }

    #[tokio::test]
    async fn giveaway_lifecycle() {
        let db = memory().await;
        let mut giveaway = Giveaway {
            guild_id: "g".into(),
            number: 0,
            channel_id: "c".into(),
            message_id: String::new(),
            prize: "Nitro".into(),
            winners: 1,
            ends_at: 100,
            host_id: "h".into(),
            required_role: None,
            ended: false,
            winner_ids: String::new(),
        };
        assert_eq!(create_giveaway(&db, &giveaway).await.unwrap(), 1);
        giveaway.number = create_giveaway(&db, &giveaway).await.unwrap();
        assert_eq!(giveaway.number, 2);
        // Not due until its message exists.
        assert!(due_giveaways(&db, 200).await.unwrap().is_empty());
        set_giveaway_message(&db, "g", 2, "m").await.unwrap();
        assert_eq!(due_giveaways(&db, 99).await.unwrap().len(), 0);
        assert_eq!(due_giveaways(&db, 100).await.unwrap()[0].number, 2);

        assert!(enter_giveaway(&db, "g", 2, "a").await.unwrap());
        assert!(!enter_giveaway(&db, "g", 2, "a").await.unwrap());
        assert!(enter_giveaway(&db, "g", 2, "b").await.unwrap());
        assert_eq!(count_entries(&db, "g", 2).await.unwrap(), 2);
        assert!(leave_giveaway(&db, "g", 2, "b").await.unwrap());
        assert_eq!(giveaway_entries(&db, "g", 2).await.unwrap(), vec!["a"]);

        // Drawn once only.
        assert!(
            finish_giveaway(&db, "g", 2, &["a".into()], false)
                .await
                .unwrap()
        );
        assert!(
            !finish_giveaway(&db, "g", 2, &["b".into()], false)
                .await
                .unwrap()
        );
        assert!(
            finish_giveaway(&db, "g", 2, &["c".into()], true)
                .await
                .unwrap()
        );
        let ended = get_giveaway(&db, "g", 2).await.unwrap().unwrap();
        assert!(ended.ended);
        assert_eq!(ended.winner_list(), vec!["a", "c"]);
        assert!(due_giveaways(&db, 200).await.unwrap().is_empty());

        delete_giveaway(&db, "g", 2).await.unwrap();
        assert!(get_giveaway(&db, "g", 2).await.unwrap().is_none());
        assert_eq!(count_entries(&db, "g", 2).await.unwrap(), 0);
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

// ---------------------------------------------------------------- giveaways

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Giveaway {
    pub guild_id: String,
    pub number: i64,
    pub channel_id: String,
    pub message_id: String,
    pub prize: String,
    pub winners: i64,
    pub ends_at: i64,
    pub host_id: String,
    pub required_role: Option<String>,
    pub ended: bool,
    /// Every winner drawn so far, rerolls included, comma-separated.
    pub winner_ids: String,
}

impl Giveaway {
    pub fn winner_list(&self) -> Vec<String> {
        self.winner_ids
            .split(',')
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
            .collect()
    }
}

const GIVEAWAY_COLUMNS: &str = "guild_id, number, channel_id, message_id, prize, winners, \
    ends_at, host_id, required_role, ended, winner_ids";

/// Creates a giveaway and returns its number on the server (#1, #2...).
pub async fn create_giveaway(db: &SqlitePool, giveaway: &Giveaway) -> Result<i64, sqlx::Error> {
    let number: i64 = sqlx::query_scalar(
        "INSERT INTO counters (guild_id, name, value) VALUES (?, 'giveaway', 1)
         ON CONFLICT (guild_id, name) DO UPDATE SET value = value + 1
         RETURNING value",
    )
    .bind(&giveaway.guild_id)
    .fetch_one(db)
    .await?;
    sqlx::query(
        "INSERT INTO giveaways (guild_id, number, channel_id, prize, winners, ends_at, host_id, required_role)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&giveaway.guild_id)
    .bind(number)
    .bind(&giveaway.channel_id)
    .bind(&giveaway.prize)
    .bind(giveaway.winners)
    .bind(giveaway.ends_at)
    .bind(&giveaway.host_id)
    .bind(&giveaway.required_role)
    .execute(db)
    .await?;
    Ok(number)
}

pub async fn set_giveaway_message(
    db: &SqlitePool,
    guild: &str,
    number: i64,
    message: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE giveaways SET message_id = ? WHERE guild_id = ? AND number = ?")
        .bind(message)
        .bind(guild)
        .bind(number)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn get_giveaway(
    db: &SqlitePool,
    guild: &str,
    number: i64,
) -> Result<Option<Giveaway>, sqlx::Error> {
    sqlx::query_as(&format!(
        "SELECT {GIVEAWAY_COLUMNS} FROM giveaways WHERE guild_id = ? AND number = ?"
    ))
    .bind(guild)
    .bind(number)
    .fetch_optional(db)
    .await
}

/// Giveaways of a server that are still running, soonest first.
pub async fn running_giveaways(db: &SqlitePool, guild: &str) -> Result<Vec<Giveaway>, sqlx::Error> {
    sqlx::query_as(&format!(
        "SELECT {GIVEAWAY_COLUMNS} FROM giveaways WHERE guild_id = ? AND ended = 0 ORDER BY ends_at"
    ))
    .bind(guild)
    .fetch_all(db)
    .await
}

/// Running giveaways, on every server, whose time is up.
pub async fn due_giveaways(db: &SqlitePool, now: i64) -> Result<Vec<Giveaway>, sqlx::Error> {
    sqlx::query_as(&format!(
        "SELECT {GIVEAWAY_COLUMNS} FROM giveaways WHERE ended = 0 AND ends_at <= ? AND message_id != ''"
    ))
    .bind(now)
    .fetch_all(db)
    .await
}

/// Marks a giveaway ended (or adds rerolled winners). `false` when it was
/// already ended by someone else, so it is never drawn twice.
pub async fn finish_giveaway(
    db: &SqlitePool,
    guild: &str,
    number: i64,
    winners: &[String],
    reroll: bool,
) -> Result<bool, sqlx::Error> {
    let done = if reroll {
        sqlx::query(
            "UPDATE giveaways
             SET winner_ids = CASE WHEN winner_ids = '' THEN ?1 ELSE winner_ids || ',' || ?1 END
             WHERE guild_id = ?2 AND number = ?3 AND ended = 1",
        )
    } else {
        sqlx::query(
            "UPDATE giveaways SET ended = 1, winner_ids = ?1
             WHERE guild_id = ?2 AND number = ?3 AND ended = 0",
        )
    }
    .bind(winners.join(","))
    .bind(guild)
    .bind(number)
    .execute(db)
    .await?;
    Ok(done.rows_affected() == 1)
}

pub async fn delete_giveaway(db: &SqlitePool, guild: &str, number: i64) -> Result<(), sqlx::Error> {
    for table in ["giveaway_entries", "giveaways"] {
        sqlx::query(&format!(
            "DELETE FROM {table} WHERE guild_id = ? AND number = ?"
        ))
        .bind(guild)
        .bind(number)
        .execute(db)
        .await?;
    }
    Ok(())
}

/// Adds an entry; `false` when the member had already entered.
pub async fn enter_giveaway(
    db: &SqlitePool,
    guild: &str,
    number: i64,
    user: &str,
) -> Result<bool, sqlx::Error> {
    let done = sqlx::query(
        "INSERT OR IGNORE INTO giveaway_entries (guild_id, number, user_id) VALUES (?, ?, ?)",
    )
    .bind(guild)
    .bind(number)
    .bind(user)
    .execute(db)
    .await?;
    Ok(done.rows_affected() == 1)
}

pub async fn leave_giveaway(
    db: &SqlitePool,
    guild: &str,
    number: i64,
    user: &str,
) -> Result<bool, sqlx::Error> {
    let done = sqlx::query(
        "DELETE FROM giveaway_entries WHERE guild_id = ? AND number = ? AND user_id = ?",
    )
    .bind(guild)
    .bind(number)
    .bind(user)
    .execute(db)
    .await?;
    Ok(done.rows_affected() == 1)
}

pub async fn giveaway_entries(
    db: &SqlitePool,
    guild: &str,
    number: i64,
) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT user_id FROM giveaway_entries WHERE guild_id = ? AND number = ?")
        .bind(guild)
        .bind(number)
        .fetch_all(db)
        .await
}

pub async fn count_entries(db: &SqlitePool, guild: &str, number: i64) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("SELECT COUNT(*) FROM giveaway_entries WHERE guild_id = ? AND number = ?")
        .bind(guild)
        .bind(number)
        .fetch_one(db)
        .await
}

// ---------------------------------------------------------------- link reviews

/// A link a member sent to the staff, waiting for Accept or Refuse.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct LinkReview {
    pub id: String,
    pub guild_id: String,
    pub user_id: String,
    pub channel_id: String,
    pub content: String,
}

pub async fn save_link_review(db: &SqlitePool, review: &LinkReview) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT OR REPLACE INTO link_reviews (id, guild_id, user_id, channel_id, content)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&review.id)
    .bind(&review.guild_id)
    .bind(&review.user_id)
    .bind(&review.channel_id)
    .bind(&review.content)
    .execute(db)
    .await?;
    Ok(())
}

/// Takes a review out (so two staff clicks cannot both act on it).
pub async fn take_link_review(
    db: &SqlitePool,
    id: &str,
) -> Result<Option<LinkReview>, sqlx::Error> {
    sqlx::query_as(
        "DELETE FROM link_reviews WHERE id = ?
         RETURNING id, guild_id, user_id, channel_id, content",
    )
    .bind(id)
    .fetch_optional(db)
    .await
}

// ---------------------------------------------------------------- counters

pub async fn get_counter(db: &SqlitePool, guild: &str, name: &str) -> Result<i64, sqlx::Error> {
    let value: Option<i64> =
        sqlx::query_scalar("SELECT value FROM counters WHERE guild_id = ? AND name = ?")
            .bind(guild)
            .bind(name)
            .fetch_optional(db)
            .await?;
    Ok(value.unwrap_or(0))
}

pub async fn set_counter(
    db: &SqlitePool,
    guild: &str,
    name: &str,
    value: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO counters (guild_id, name, value) VALUES (?, ?, ?)
         ON CONFLICT (guild_id, name) DO UPDATE SET value = excluded.value",
    )
    .bind(guild)
    .bind(name)
    .bind(value)
    .execute(db)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------- staff tasks

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Todo {
    pub guild_id: String,
    pub number: i64,
    pub title: String,
    pub details: String,
    pub author_id: String,
    pub created_at: i64,
    pub claimed_by: Option<String>,
    pub claimed_at: Option<i64>,
    pub done: bool,
    pub done_by: Option<String>,
    pub done_at: Option<i64>,
}

const TODO_COLUMNS: &str = "guild_id, number, title, details, author_id, created_at, \
    claimed_by, claimed_at, done, done_by, done_at";

/// Finished tasks kept per server (the older ones are deleted).
const TODO_DONE_KEPT: i64 = 50;

/// Adds a task, claimed right away by `claimed_by` if given, and returns
/// its number on the server (#1, #2...).
pub async fn create_todo(
    db: &SqlitePool,
    guild: &str,
    (title, details): (&str, &str),
    author: &str,
    claimed_by: Option<&str>,
) -> Result<i64, sqlx::Error> {
    let number: i64 = sqlx::query_scalar(
        "INSERT INTO counters (guild_id, name, value) VALUES (?, 'todo', 1)
         ON CONFLICT (guild_id, name) DO UPDATE SET value = value + 1
         RETURNING value",
    )
    .bind(guild)
    .fetch_one(db)
    .await?;
    let now = dc_core::now();
    sqlx::query(
        "INSERT INTO todos (guild_id, number, title, details, author_id, created_at, claimed_by, claimed_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(guild)
    .bind(number)
    .bind(title)
    .bind(details)
    .bind(author)
    .bind(now)
    .bind(claimed_by)
    .bind(claimed_by.map(|_| now))
    .execute(db)
    .await?;
    Ok(number)
}

pub async fn get_todo(
    db: &SqlitePool,
    guild: &str,
    number: i64,
) -> Result<Option<Todo>, sqlx::Error> {
    sqlx::query_as(&format!(
        "SELECT {TODO_COLUMNS} FROM todos WHERE guild_id = ? AND number = ?"
    ))
    .bind(guild)
    .bind(number)
    .fetch_optional(db)
    .await
}

/// Tasks still to do, oldest first.
pub async fn open_todos(db: &SqlitePool, guild: &str) -> Result<Vec<Todo>, sqlx::Error> {
    sqlx::query_as(&format!(
        "SELECT {TODO_COLUMNS} FROM todos WHERE guild_id = ? AND done = 0 ORDER BY number"
    ))
    .bind(guild)
    .fetch_all(db)
    .await
}

/// The last finished tasks, newest first.
pub async fn done_todos(
    db: &SqlitePool,
    guild: &str,
    limit: i64,
) -> Result<Vec<Todo>, sqlx::Error> {
    sqlx::query_as(&format!(
        "SELECT {TODO_COLUMNS} FROM todos WHERE guild_id = ? AND done = 1
         ORDER BY done_at DESC, number DESC LIMIT ?"
    ))
    .bind(guild)
    .bind(limit)
    .fetch_all(db)
    .await
}

/// Claims an open task nobody has claimed. `false` when it is done, gone or
/// already claimed (two clicks at the same time: only one wins).
pub async fn claim_todo(
    db: &SqlitePool,
    guild: &str,
    number: i64,
    user: &str,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE todos SET claimed_by = ?, claimed_at = ?
         WHERE guild_id = ? AND number = ? AND done = 0 AND claimed_by IS NULL",
    )
    .bind(user)
    .bind(dc_core::now())
    .bind(guild)
    .bind(number)
    .execute(db)
    .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn unclaim_todo(db: &SqlitePool, guild: &str, number: i64) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE todos SET claimed_by = NULL, claimed_at = NULL
         WHERE guild_id = ? AND number = ? AND done = 0 AND claimed_by IS NOT NULL",
    )
    .bind(guild)
    .bind(number)
    .execute(db)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Marks an open task done. `false` when it was already done or is gone.
pub async fn finish_todo(
    db: &SqlitePool,
    guild: &str,
    number: i64,
    user: &str,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE todos SET done = 1, done_by = ?, done_at = ?
         WHERE guild_id = ? AND number = ? AND done = 0",
    )
    .bind(user)
    .bind(dc_core::now())
    .bind(guild)
    .bind(number)
    .execute(db)
    .await?;
    sqlx::query(
        "DELETE FROM todos WHERE guild_id = ? AND done = 1 AND number NOT IN (
            SELECT number FROM todos WHERE guild_id = ? AND done = 1
            ORDER BY done_at DESC, number DESC LIMIT ?
        )",
    )
    .bind(guild)
    .bind(guild)
    .bind(TODO_DONE_KEPT)
    .execute(db)
    .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn delete_todo(db: &SqlitePool, guild: &str, number: i64) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM todos WHERE guild_id = ? AND number = ?")
        .bind(guild)
        .bind(number)
        .execute(db)
        .await?;
    Ok(result.rows_affected() == 1)
}

#[cfg(test)]
mod todo_tests {
    use super::*;

    async fn memory() -> SqlitePool {
        let db = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        init(&db).await.unwrap();
        db
    }

    #[tokio::test]
    async fn todos() {
        let db = memory().await;
        let one = create_todo(&db, "g", ("Pizza module", ""), "1", None)
            .await
            .unwrap();
        let two = create_todo(&db, "g", ("Docs", "the wiki"), "1", Some("5"))
            .await
            .unwrap();
        assert_eq!((one, two), (1, 2));
        let other = create_todo(&db, "h", ("Other", ""), "1", None).await;
        assert_eq!(other.unwrap(), 1);

        assert!(claim_todo(&db, "g", 1, "7").await.unwrap());
        // Already claimed: the second click loses.
        assert!(!claim_todo(&db, "g", 1, "8").await.unwrap());
        let claimed = get_todo(&db, "g", 1).await.unwrap().unwrap();
        assert_eq!(claimed.claimed_by.as_deref(), Some("7"));
        assert!(unclaim_todo(&db, "g", 1).await.unwrap());
        assert!(claim_todo(&db, "g", 1, "8").await.unwrap());

        assert!(finish_todo(&db, "g", 1, "8").await.unwrap());
        assert!(!finish_todo(&db, "g", 1, "8").await.unwrap());
        assert!(!claim_todo(&db, "g", 1, "9").await.unwrap());
        let open = open_todos(&db, "g").await.unwrap();
        assert_eq!(open.iter().map(|t| t.number).collect::<Vec<_>>(), vec![2]);
        let done = done_todos(&db, "g", 5).await.unwrap();
        assert_eq!(done[0].done_by.as_deref(), Some("8"));

        assert!(delete_todo(&db, "g", 2).await.unwrap());
        assert!(open_todos(&db, "g").await.unwrap().is_empty());
        // Numbers are never reused.
        let next = create_todo(&db, "g", ("Next", ""), "1", None).await;
        assert_eq!(next.unwrap(), 3);
    }
}
