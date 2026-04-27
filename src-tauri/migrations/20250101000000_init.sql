-- Initial schema for the WhatsApp Blaster desktop app.
-- Applied by sqlx::migrate!() at startup.

PRAGMA foreign_keys = ON;
PRAGMA journal_mode = WAL;

CREATE TABLE IF NOT EXISTS contacts (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    phone           TEXT    NOT NULL UNIQUE,
    name            TEXT    NOT NULL DEFAULT '',
    last_sent_date  TEXT,                            -- ISO yyyy-mm-dd
    created_at      TEXT    NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_contacts_phone           ON contacts(phone);
CREATE INDEX IF NOT EXISTS idx_contacts_last_sent_date  ON contacts(last_sent_date);

CREATE TABLE IF NOT EXISTS logs (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    contact_id    INTEGER NOT NULL REFERENCES contacts(id) ON DELETE CASCADE,
    status        TEXT    NOT NULL CHECK(status IN ('success','failed','skipped')),
    api_key_used  TEXT    NOT NULL,
    detail        TEXT,
    timestamp     TEXT    NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_logs_contact_id  ON logs(contact_id);
CREATE INDEX IF NOT EXISTS idx_logs_timestamp   ON logs(timestamp);
CREATE INDEX IF NOT EXISTS idx_logs_status      ON logs(status);
