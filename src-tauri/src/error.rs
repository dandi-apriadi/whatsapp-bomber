use serde::Serialize;
use thiserror::Error;

/// Application-wide error type. Implements `Serialize` so it can flow back to
/// the Tauri frontend through `#[command]` return values.
#[allow(dead_code)] // Some variants are reserved for future commands.
#[derive(Debug, Error)]
pub enum AppError {
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),

    #[error("migration error: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),

    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("serde error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("csv error: {0}")]
    Csv(#[from] csv::Error),

    #[error("xlsx error: {0}")]
    Xlsx(#[from] calamine::Error),

    #[error("xlsx range error: {0}")]
    XlsxRange(#[from] calamine::XlsxError),

    #[error("invalid input: {0}")]
    Invalid(String),

    #[error("fonnte error: {0}")]
    Fonnte(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("blaster busy")]
    Busy,
}

impl AppError {
    pub fn invalid(msg: impl Into<String>) -> Self {
        Self::Invalid(msg.into())
    }
}

impl Serialize for AppError {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_str(&self.to_string())
    }
}

pub type AppResult<T> = std::result::Result<T, AppError>;
