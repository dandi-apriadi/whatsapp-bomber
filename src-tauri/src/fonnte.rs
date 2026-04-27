//! Fonnte gateway HTTP client with multi-account key rotation.
//!
//! Fonnte's send endpoint expects a `POST https://api.fonnte.com/send` with the
//! API key in the `Authorization` header, and form-encoded fields `target` /
//! `message`. See https://docs.fonnte.com/.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use reqwest::Client;
use serde::Deserialize;

use crate::error::{AppError, AppResult};
use crate::settings::AccountMode;

const FONNTE_SEND_URL: &str = "https://api.fonnte.com/send";

/// Result of a single Fonnte send call.
#[derive(Debug, Clone)]
pub struct SendOutcome {
    /// API key used for this attempt — exposed so callers can correlate
    /// outcomes with the key in the logs table.
    #[allow(dead_code)]
    pub api_key: String,
    pub success: bool,
    pub detail: String,
}

/// Lightweight subset of Fonnte's JSON response. Their API is loosely typed
/// (`status` may be a bool, a string, or omitted entirely on errors), so we
/// only deserialize the fields we care about and fall back to raw text otherwise.
#[derive(Debug, Default, Deserialize)]
struct FonnteResponse {
    #[serde(default)]
    status: Option<serde_json::Value>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    detail: Option<String>,
}

#[derive(Clone)]
pub struct FonnteClient {
    http: Client,
    rotation: Arc<AtomicUsize>,
}

impl Default for FonnteClient {
    fn default() -> Self {
        Self::new()
    }
}

impl FonnteClient {
    pub fn new() -> Self {
        let http = Client::builder()
            .user_agent("whatsapp-bomber/0.1")
            .timeout(Duration::from_secs(30))
            .build()
            .expect("reqwest client");
        Self {
            http,
            rotation: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Pick the next API key according to `mode`.
    pub fn pick_key(&self, keys: &[String], mode: AccountMode) -> AppResult<String> {
        let usable: Vec<&String> = keys.iter().filter(|k| !k.trim().is_empty()).collect();
        if usable.is_empty() {
            return Err(AppError::invalid("no Fonnte API keys configured"));
        }
        let key = match mode {
            AccountMode::Single => usable[0].clone(),
            AccountMode::RoundRobin => {
                let idx = self.rotation.fetch_add(1, Ordering::Relaxed) % usable.len();
                usable[idx].clone()
            }
        };
        Ok(key)
    }

    /// Send a single WhatsApp message via Fonnte.
    ///
    /// `target` should be in international format (e.g. `6281234567890`) — Fonnte
    /// accepts a leading `+` but not local-format numbers.
    pub async fn send(&self, api_key: &str, target: &str, message: &str) -> AppResult<SendOutcome> {
        let form = [("target", target), ("message", message)];

        let resp = self
            .http
            .post(FONNTE_SEND_URL)
            .header("Authorization", api_key)
            .form(&form)
            .send()
            .await?;

        let http_status = resp.status();
        let body = resp.text().await.unwrap_or_default();

        // Fonnte returns HTTP 200 even on logical failures, so we have to inspect
        // the body to decide success.
        let parsed: FonnteResponse = serde_json::from_str(&body).unwrap_or_default();

        let logical_ok = match parsed.status.as_ref() {
            Some(serde_json::Value::Bool(b)) => *b,
            Some(serde_json::Value::String(s)) => {
                let s = s.to_ascii_lowercase();
                s == "true" || s == "success" || s == "ok"
            }
            Some(serde_json::Value::Number(n)) => n.as_i64().map(|v| v != 0).unwrap_or(false),
            _ => false,
        };

        let success = http_status.is_success() && logical_ok;
        let detail = parsed
            .reason
            .or(parsed.detail)
            .unwrap_or_else(|| truncate(&body, 500));

        Ok(SendOutcome {
            api_key: api_key.to_string(),
            success,
            detail,
        })
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_key_round_robin_cycles() {
        let client = FonnteClient::new();
        let keys = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let mut picks = Vec::new();
        for _ in 0..6 {
            picks.push(client.pick_key(&keys, AccountMode::RoundRobin).unwrap());
        }
        assert_eq!(picks, vec!["a", "b", "c", "a", "b", "c"]);
    }

    #[test]
    fn pick_key_skips_empty() {
        let client = FonnteClient::new();
        let keys = vec!["".to_string(), "  ".to_string(), "real".to_string()];
        assert_eq!(
            client.pick_key(&keys, AccountMode::Single).unwrap(),
            "real".to_string()
        );
    }

    #[test]
    fn pick_key_errors_when_no_keys() {
        let client = FonnteClient::new();
        let err = client.pick_key(&[], AccountMode::Single).unwrap_err();
        assert!(err.to_string().contains("no Fonnte API keys"));
    }
}
