-- What was done on the panel, per bot: settings saved, profile changed,
-- container started… Shown under the bot's console on the Logs tab.
CREATE TABLE audit (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    bot       TEXT NOT NULL,               -- bot slug
    at        INTEGER NOT NULL,            -- unix seconds
    user_id   TEXT NOT NULL,               -- Discord id of who did it
    user_name TEXT NOT NULL,
    action    TEXT NOT NULL
);
CREATE INDEX audit_bot ON audit(bot, id);
