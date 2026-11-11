-- Bots are not stored here: they are defined by the admin in bots/<slug>/bot.toml.

CREATE TABLE users (
    id            TEXT PRIMARY KEY,          -- Discord user id
    username      TEXT NOT NULL,
    global_name   TEXT,
    avatar        TEXT,
    created_at    INTEGER NOT NULL,
    last_login_at INTEGER NOT NULL
);

CREATE TABLE sessions (
    id_hash    TEXT PRIMARY KEY,             -- SHA-256 of the session token
    user_id    TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    csrf_token TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
);
CREATE INDEX sessions_user ON sessions(user_id);
CREATE INDEX sessions_expiry ON sessions(expires_at);
