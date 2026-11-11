use std::{str::FromStr, time::Duration};

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};

pub type Db = SqlitePool;

/// Opens a database file, creating it if needed. No migration is run.
pub async fn open(url: &str) -> Result<Db, sqlx::Error> {
    let options = SqliteConnectOptions::from_str(url)?
        .create_if_missing(true)
        .foreign_keys(true)
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(Duration::from_secs(5));

    if let Some(parent) = options.get_filename().parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(sqlx::Error::Io)?;
    }

    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(options)
        .await?;

    Ok(pool)
}

/// Opens the panel database and runs its migrations.
pub async fn connect(url: &str) -> Result<Db, sqlx::Error> {
    let pool = open(url).await?;
    sqlx::migrate!("./migrations").run(&pool).await?;
    Ok(pool)
}
