//! Persistent app settings: Fonnte API keys + anti-ban knobs.
//!
//! Stored as JSON next to the SQLite database so users can hand-edit if needed.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

/// Mode used to pick a Fonnte API key for the next message.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountMode {
    /// Always use the first key in the list.
    Single,
    /// Round-robin over the list of keys.
    #[default]
    RoundRobin,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AntiBanConfig {
    /// Lower bound for the per-message random delay, in seconds.
    pub min_delay_secs: u64,
    /// Upper bound for the per-message random delay, in seconds.
    pub max_delay_secs: u64,
    /// After this many sent messages, sleep `batch_pause_secs` before resuming.
    pub batch_size: u32,
    /// Pause duration applied between batches.
    pub batch_pause_secs: u64,
}

impl Default for AntiBanConfig {
    fn default() -> Self {
        Self {
            min_delay_secs: 10,
            max_delay_secs: 30,
            batch_size: 50,
            batch_pause_secs: 5 * 60,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct AppSettings {
    /// Ordered list of Fonnte API keys. The first non-empty key is used in
    /// `Single` mode; all are used in `RoundRobin` mode.
    pub api_keys: Vec<String>,
    pub account_mode: AccountMode,
    pub anti_ban: AntiBanConfig,
}

impl AppSettings {
    pub fn validate(&self) -> AppResult<()> {
        if self.anti_ban.min_delay_secs > self.anti_ban.max_delay_secs {
            return Err(AppError::invalid(
                "min_delay_secs must be <= max_delay_secs",
            ));
        }
        if self.anti_ban.batch_size == 0 {
            return Err(AppError::invalid("batch_size must be > 0"));
        }
        Ok(())
    }
}

/// Thread-safe handle around `AppSettings` persisted to disk on every change.
#[derive(Clone)]
pub struct SettingsStore {
    inner: Arc<RwLock<AppSettings>>,
    path: PathBuf,
}

impl SettingsStore {
    /// Load settings from `path` if it exists, otherwise create defaults and persist them.
    pub fn load(path: &Path) -> AppResult<Self> {
        let settings = if path.exists() {
            let raw = std::fs::read_to_string(path)?;
            serde_json::from_str::<AppSettings>(&raw).unwrap_or_default()
        } else {
            AppSettings::default()
        };
        let store = Self {
            inner: Arc::new(RwLock::new(settings)),
            path: path.to_path_buf(),
        };
        store.persist()?;
        Ok(store)
    }

    pub fn snapshot(&self) -> AppSettings {
        self.inner.read().clone()
    }

    pub fn replace(&self, new: AppSettings) -> AppResult<()> {
        new.validate()?;
        *self.inner.write() = new;
        self.persist()
    }

    fn persist(&self) -> AppResult<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(&*self.inner.read())?;
        std::fs::write(&self.path, json)?;
        Ok(())
    }
}
