//! SQLite layer: pool construction, migrations, and typed repository helpers.
//!
//! Uses runtime `sqlx::query*` calls (not the compile-time macros) so the project
//! can be `cargo check`ed without a populated `DATABASE_URL` or a `.sqlx` cache.

use std::path::Path;

use chrono::NaiveDate;
use serde::Serialize;
use sqlx::{
    migrate::Migrator,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions, SqliteRow},
    FromRow, Row, SqlitePool,
};

use crate::error::AppResult;

/// `sqlx::migrate!()` embeds the `migrations/` directory at compile time so the
/// binary is self-contained (this macro does NOT need a live DB).
static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

#[derive(Clone, Debug, Serialize, FromRow)]
pub struct Contact {
    pub id: i64,
    pub phone: String,
    pub name: String,
    pub last_sent_date: Option<String>,
}

#[derive(Clone, Debug, Serialize, FromRow)]
pub struct LogRow {
    pub id: i64,
    pub contact_id: i64,
    pub status: String,
    pub api_key_used: String,
    pub detail: Option<String>,
    pub timestamp: String,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct DashboardStats {
    pub total_contacts: i64,
    pub total_sent: i64,
    pub total_failed: i64,
    pub total_skipped: i64,
}

#[derive(Clone)]
pub struct Db {
    pool: SqlitePool,
}

impl Db {
    /// Open (or create) the SQLite database at `path` and apply all pending migrations.
    pub async fn connect(path: &Path) -> AppResult<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let opts = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal);

        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(opts)
            .await?;

        MIGRATOR.run(&pool).await?;

        Ok(Self { pool })
    }

    #[allow(dead_code)]
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    // -------------------- contacts --------------------

    /// Insert a contact, or update its name if the phone already exists.
    /// Returns the contact id.
    pub async fn upsert_contact(&self, phone: &str, name: &str) -> AppResult<i64> {
        let phone = normalize_phone(phone);
        let row: SqliteRow = sqlx::query(
            r#"
            INSERT INTO contacts (phone, name) VALUES (?1, ?2)
            ON CONFLICT(phone) DO UPDATE SET name = excluded.name
            RETURNING id
            "#,
        )
        .bind(phone)
        .bind(name)
        .fetch_one(&self.pool)
        .await?;
        let id: i64 = row.try_get("id")?;
        Ok(id)
    }

    pub async fn list_contacts(&self, limit: i64) -> AppResult<Vec<Contact>> {
        let rows = sqlx::query_as::<_, Contact>(
            r#"
            SELECT id, phone, name, last_sent_date
            FROM contacts
            ORDER BY id DESC
            LIMIT ?1
            "#,
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn delete_all_contacts(&self) -> AppResult<u64> {
        let res = sqlx::query("DELETE FROM contacts")
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected())
    }

    /// Set `contacts.last_sent_date` to `today` for the given contact id.
    pub async fn mark_sent_today(&self, contact_id: i64, today: NaiveDate) -> AppResult<()> {
        let date_str = today.format("%Y-%m-%d").to_string();
        sqlx::query("UPDATE contacts SET last_sent_date = ?1 WHERE id = ?2")
            .bind(date_str)
            .bind(contact_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// True when the given contact already has `last_sent_date == today`.
    pub async fn was_sent_today(&self, contact_id: i64, today: NaiveDate) -> AppResult<bool> {
        let date_str = today.format("%Y-%m-%d").to_string();
        let row: Option<SqliteRow> =
            sqlx::query("SELECT last_sent_date FROM contacts WHERE id = ?1")
                .bind(contact_id)
                .fetch_optional(&self.pool)
                .await?;
        let Some(row) = row else {
            return Ok(false);
        };
        let stored: Option<String> = row.try_get("last_sent_date")?;
        Ok(matches!(stored, Some(d) if d == date_str))
    }

    // -------------------- logs --------------------

    pub async fn insert_log(
        &self,
        contact_id: i64,
        status: &str,
        api_key_used: &str,
        detail: Option<&str>,
    ) -> AppResult<i64> {
        let row: SqliteRow = sqlx::query(
            r#"
            INSERT INTO logs (contact_id, status, api_key_used, detail)
            VALUES (?1, ?2, ?3, ?4)
            RETURNING id
            "#,
        )
        .bind(contact_id)
        .bind(status)
        .bind(api_key_used)
        .bind(detail)
        .fetch_one(&self.pool)
        .await?;
        let id: i64 = row.try_get("id")?;
        Ok(id)
    }

    pub async fn recent_logs(&self, limit: i64) -> AppResult<Vec<LogRow>> {
        let rows = sqlx::query_as::<_, LogRow>(
            r#"
            SELECT id, contact_id, status, api_key_used, detail, timestamp
            FROM logs
            ORDER BY id DESC
            LIMIT ?1
            "#,
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn dashboard_stats(&self) -> AppResult<DashboardStats> {
        let total_contacts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM contacts")
            .fetch_one(&self.pool)
            .await?;
        let total_sent: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM logs WHERE status = 'success'")
                .fetch_one(&self.pool)
                .await?;
        let total_failed: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM logs WHERE status = 'failed'")
                .fetch_one(&self.pool)
                .await?;
        let total_skipped: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM logs WHERE status = 'skipped'")
                .fetch_one(&self.pool)
                .await?;

        Ok(DashboardStats {
            total_contacts,
            total_sent,
            total_failed,
            total_skipped,
        })
    }
}

/// Normalize a phone number to digits only, preserving an optional leading `+`.
pub fn normalize_phone(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let trimmed = raw.trim();
    let mut chars = trimmed.chars().peekable();
    if matches!(chars.peek(), Some('+')) {
        out.push('+');
        chars.next();
    }
    for c in chars {
        if c.is_ascii_digit() {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_phone_strips_separators() {
        assert_eq!(normalize_phone(" +62 812-3456-7890 "), "+6281234567890");
        assert_eq!(normalize_phone("0812.3456.7890"), "081234567890");
        assert_eq!(normalize_phone(""), "");
    }
}
