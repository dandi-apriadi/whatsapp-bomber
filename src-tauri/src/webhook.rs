//! Embedded Fonnte webhook listener.
//!
//! Fonnte forwards every incoming WhatsApp message to the URL configured in
//! their dashboard. The payload (per
//! https://docs.fonnte.com/category/webhook/) is a JSON object with at least:
//!
//! ```json
//! {
//!   "device":  "6281234567890",
//!   "sender":  "6281200000099",
//!   "message": "halo",
//!   "name":    "Push Name",
//!   "member":  "6281211111111"   // present only when sender is a group id
//! }
//! ```
//!
//! On every payload we upsert `{phone: sender_or_member, name: pushname}` into
//! the `contacts` table — that's how the "passive sync" feature populates the
//! local phone book without any explicit user action.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Emitter};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::db::{normalize_phone, Db};

/// Live status of the webhook listener — exposed to the frontend so it can
/// render an indicator card.
#[derive(Clone, Debug, Default, Serialize)]
pub struct WebhookStatus {
    pub running: bool,
    pub port: u16,
    pub total_received: u64,
    pub total_contacts_added: u64,
    pub total_contacts_updated: u64,
    pub last_received_at: Option<String>,
    pub last_sender: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Clone)]
pub struct WebhookManager {
    db: Db,
    inner: Arc<RwLock<Inner>>,
}

#[derive(Default)]
struct Inner {
    handle: Option<JoinHandle<()>>,
    shutdown: Option<oneshot::Sender<()>>,
    status: WebhookStatus,
}

#[derive(Clone)]
struct AppCtx {
    db: Db,
    manager: WebhookManager,
    app: AppHandle,
}

impl WebhookManager {
    pub fn new(db: Db) -> Self {
        Self {
            db,
            inner: Arc::new(RwLock::new(Inner::default())),
        }
    }

    pub fn status(&self) -> WebhookStatus {
        self.inner.read().status.clone()
    }

    /// Start the webhook server on `port`. If a server is already running, it
    /// is stopped first.
    pub async fn start(&self, app: AppHandle, port: u16) -> Result<(), String> {
        self.stop().await;

        let addr = SocketAddr::from(([0, 0, 0, 0], port));
        let listener = match tokio::net::TcpListener::bind(addr).await {
            Ok(l) => l,
            Err(e) => {
                let mut g = self.inner.write();
                g.status.running = false;
                g.status.last_error = Some(format!("bind {port}: {e}"));
                return Err(g.status.last_error.clone().unwrap_or_default());
            }
        };

        let ctx = AppCtx {
            db: self.db.clone(),
            manager: self.clone(),
            app: app.clone(),
        };

        let router = Router::new()
            .route("/health", get(handle_health))
            .route("/fonnte-webhook", post(handle_webhook))
            .with_state(ctx);

        let (tx, rx) = oneshot::channel::<()>();

        {
            let mut g = self.inner.write();
            g.status = WebhookStatus {
                running: true,
                port,
                total_received: g.status.total_received, // preserve counters across restarts
                total_contacts_added: g.status.total_contacts_added,
                total_contacts_updated: g.status.total_contacts_updated,
                last_received_at: g.status.last_received_at.clone(),
                last_sender: g.status.last_sender.clone(),
                last_error: None,
            };
        }
        emit_status(&app, &self.status());

        let manager = self.clone();
        let app_emit = app.clone();
        let handle = tokio::spawn(async move {
            let serve = axum::serve(listener, router).with_graceful_shutdown(async move {
                let _ = rx.await;
            });
            if let Err(e) = serve.await {
                tracing::error!("webhook server error: {e}");
                let mut g = manager.inner.write();
                g.status.running = false;
                g.status.last_error = Some(e.to_string());
                emit_status(&app_emit, &g.status);
            }
        });

        let mut g = self.inner.write();
        g.handle = Some(handle);
        g.shutdown = Some(tx);
        Ok(())
    }

    pub async fn stop(&self) {
        let (tx, handle) = {
            let mut g = self.inner.write();
            (g.shutdown.take(), g.handle.take())
        };
        if let Some(tx) = tx {
            let _ = tx.send(());
        }
        if let Some(h) = handle {
            // Bound the wait so a stuck worker doesn't deadlock the UI.
            let _ = tokio::time::timeout(Duration::from_secs(3), h).await;
        }
        let mut g = self.inner.write();
        g.status.running = false;
    }
}

fn emit_status(app: &AppHandle, status: &WebhookStatus) {
    let _ = app.emit("webhook://status", status);
}

async fn handle_health() -> &'static str {
    "ok"
}

/// Fields we care about from Fonnte's incoming webhook. Everything else is
/// captured into `extra` so we don't fail on unexpected payloads.
#[derive(Debug, Default, Deserialize)]
#[allow(dead_code)]
struct FonntePayload {
    #[serde(default)]
    device: Option<String>,
    #[serde(default)]
    sender: Option<String>,
    #[serde(default)]
    message: Option<String>,
    /// Push-name of the sender as set on their WhatsApp profile.
    #[serde(default)]
    name: Option<String>,
    /// Set only when `sender` is a group id; the actual member's number.
    #[serde(default)]
    member: Option<String>,
    #[serde(flatten)]
    extra: std::collections::BTreeMap<String, Value>,
}

async fn handle_webhook(
    State(ctx): State<AppCtx>,
    Json(payload): Json<FonntePayload>,
) -> (StatusCode, &'static str) {
    let result = process_payload(&ctx.db, &payload).await;

    {
        let mut g = ctx.manager.inner.write();
        g.status.total_received = g.status.total_received.saturating_add(1);
        g.status.last_received_at = Some(Utc::now().to_rfc3339());
        g.status.last_sender = pick_phone(&payload).cloned();

        match result {
            Ok(Outcome::Added) => {
                g.status.total_contacts_added = g.status.total_contacts_added.saturating_add(1);
                g.status.last_error = None;
            }
            Ok(Outcome::Updated) => {
                g.status.total_contacts_updated = g.status.total_contacts_updated.saturating_add(1);
                g.status.last_error = None;
            }
            Ok(Outcome::Skipped(reason)) => {
                g.status.last_error = Some(reason);
            }
            Err(e) => {
                g.status.last_error = Some(e);
            }
        }

        emit_status(&ctx.app, &g.status);
    }

    (StatusCode::OK, "ok")
}

#[derive(Debug)]
enum Outcome {
    Added,
    Updated,
    #[allow(dead_code)]
    Skipped(String),
}

async fn process_payload(db: &Db, payload: &FonntePayload) -> Result<Outcome, String> {
    let phone = pick_phone(payload).cloned().unwrap_or_default();
    if phone.trim().is_empty() {
        return Ok(Outcome::Skipped("no sender/member field".into()));
    }
    let normalized = normalize_phone(&phone);
    if normalized.is_empty() {
        return Ok(Outcome::Skipped("phone normalized to empty".into()));
    }

    let name = payload.name.clone().unwrap_or_default();

    let existed_before = db
        .find_contact_by_phone(&normalized)
        .await
        .map_err(|e| e.to_string())?
        .is_some();

    db.upsert_contact(&normalized, &name)
        .await
        .map_err(|e| e.to_string())?;

    Ok(if existed_before {
        Outcome::Updated
    } else {
        Outcome::Added
    })
}

fn pick_phone(p: &FonntePayload) -> Option<&String> {
    // For group messages Fonnte sets `sender` to the group id and `member` to
    // the actual sender — prefer member then.
    if let Some(m) = p.member.as_ref().filter(|s| !s.trim().is_empty()) {
        return Some(m);
    }
    p.sender.as_ref().filter(|s| !s.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_member_when_group() {
        let p = FonntePayload {
            sender: Some("12345-67890@g.us".into()),
            member: Some("6281200000001".into()),
            name: Some("Alice".into()),
            ..Default::default()
        };
        assert_eq!(pick_phone(&p).unwrap(), "6281200000001");
    }

    #[test]
    fn falls_back_to_sender() {
        let p = FonntePayload {
            sender: Some("6281200000002".into()),
            member: Some("".into()),
            ..Default::default()
        };
        assert_eq!(pick_phone(&p).unwrap(), "6281200000002");
    }

    #[test]
    fn empty_when_neither() {
        let p = FonntePayload::default();
        assert!(pick_phone(&p).is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn process_payload_inserts_then_updates() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("t.db");
        let db = Db::connect(&db_path).await.unwrap();

        // First event: a brand-new contact is added.
        let p1 = FonntePayload {
            sender: Some("+62 812 3456 7890".into()),
            name: Some("Alice".into()),
            ..Default::default()
        };
        let out = process_payload(&db, &p1).await.unwrap();
        assert!(matches!(out, Outcome::Added));

        // Second event with the same number but different push-name updates it.
        let p2 = FonntePayload {
            sender: Some("+6281234567890".into()), // normalizes to the same value
            name: Some("Alice Renamed".into()),
            ..Default::default()
        };
        let out = process_payload(&db, &p2).await.unwrap();
        assert!(matches!(out, Outcome::Updated));

        let stored = db
            .find_contact_by_phone("+6281234567890")
            .await
            .unwrap()
            .expect("contact should exist");
        assert_eq!(stored.name, "Alice Renamed");

        // Third event without a sender is skipped silently.
        let p3 = FonntePayload::default();
        let out = process_payload(&db, &p3).await.unwrap();
        assert!(matches!(out, Outcome::Skipped(_)));
    }
}
