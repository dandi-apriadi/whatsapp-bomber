//! Blaster engine — drives the async send-loop with anti-ban delays,
//! batching, and same-day deduplication.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::Local;
use rand::Rng;
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::sync::Mutex;

use crate::db::{normalize_phone, Db};
use crate::error::{AppError, AppResult};
use crate::fonnte::FonnteClient;
use crate::settings::SettingsStore;

/// One entry in a blast job. The frontend builds this list (e.g. by importing
/// a CSV/XLSX or keying contacts in manually) and submits it to `BlasterEngine`.
#[derive(Clone, Debug, Serialize, serde::Deserialize)]
pub struct BlastTarget {
    pub phone: String,
    #[serde(default)]
    pub name: String,
}

/// Aggregate counters surfaced to the dashboard while a job is running.
#[derive(Clone, Debug, Default, Serialize)]
pub struct BlastProgress {
    pub total: usize,
    pub processed: usize,
    pub sent: usize,
    pub skipped: usize,
    pub failed: usize,
    pub current_phone: Option<String>,
    pub current_status: Option<String>,
    pub running: bool,
}

#[derive(Clone)]
pub struct BlasterEngine {
    db: Db,
    settings: SettingsStore,
    fonnte: FonnteClient,
    state: Arc<Mutex<BlastProgress>>,
    running: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
}

impl BlasterEngine {
    pub fn new(db: Db, settings: SettingsStore, fonnte: FonnteClient) -> Self {
        Self {
            db,
            settings,
            fonnte,
            state: Arc::new(Mutex::new(BlastProgress::default())),
            running: Arc::new(AtomicBool::new(false)),
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    pub async fn snapshot(&self) -> BlastProgress {
        self.state.lock().await.clone()
    }

    #[allow(dead_code)]
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    pub fn request_stop(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    /// Spawn a blast job. Returns immediately; progress flows to the frontend
    /// via `blast://progress` events.
    pub async fn start(
        &self,
        app: AppHandle,
        targets: Vec<BlastTarget>,
        message_template: String,
    ) -> AppResult<()> {
        if self
            .running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err(AppError::Busy);
        }
        self.cancel.store(false, Ordering::SeqCst);

        // Reset state.
        {
            let mut s = self.state.lock().await;
            *s = BlastProgress {
                total: targets.len(),
                running: true,
                ..Default::default()
            };
            emit_progress(&app, &s);
        }

        let this = self.clone();
        tokio::spawn(async move {
            let result = this.run_loop(&app, targets, message_template).await;

            this.running.store(false, Ordering::SeqCst);
            {
                let mut s = this.state.lock().await;
                s.running = false;
                s.current_phone = None;
                s.current_status = match &result {
                    Ok(()) => Some("done".into()),
                    Err(e) => Some(format!("error: {e}")),
                };
                emit_progress(&app, &s);
            }
        });

        Ok(())
    }

    async fn run_loop(
        &self,
        app: &AppHandle,
        targets: Vec<BlastTarget>,
        message_template: String,
    ) -> AppResult<()> {
        let total = targets.len();
        let mut sent_in_batch: u32 = 0;

        for (idx, target) in targets.into_iter().enumerate() {
            if self.cancel.load(Ordering::SeqCst) {
                break;
            }

            // Always re-read settings so the user can tweak knobs mid-blast.
            let settings = self.settings.snapshot();
            let phone = normalize_phone(&target.phone);
            if phone.is_empty() {
                self.bump_progress(app, idx + 1, total, &target.phone, "invalid", |p| {
                    p.failed += 1
                })
                .await;
                continue;
            }

            // Persist the contact so we can attach logs to it (and use the
            // last_sent_date column for same-day dedup).
            let contact_id = match self.db.upsert_contact(&phone, &target.name).await {
                Ok(id) => id,
                Err(e) => {
                    tracing::error!("upsert_contact failed: {e}");
                    self.bump_progress(app, idx + 1, total, &phone, "db-error", |p| p.failed += 1)
                        .await;
                    continue;
                }
            };

            // ---- smart-filter: skip if already sent today ----
            let today = Local::now().date_naive();
            if self.db.was_sent_today(contact_id, today).await? {
                let _ = self
                    .db
                    .insert_log(contact_id, "skipped", "-", Some("duplicate-today"))
                    .await;
                self.bump_progress(app, idx + 1, total, &phone, "skipped", |p| p.skipped += 1)
                    .await;
                continue;
            }

            // ---- pick API key ----
            let api_key = match self
                .fonnte
                .pick_key(&settings.api_keys, settings.account_mode)
            {
                Ok(k) => k,
                Err(e) => {
                    let _ = self
                        .db
                        .insert_log(contact_id, "failed", "-", Some(&e.to_string()))
                        .await;
                    self.bump_progress(app, idx + 1, total, &phone, "no-key", |p| p.failed += 1)
                        .await;
                    // No keys available — abort early.
                    return Err(e);
                }
            };

            // ---- render + send ----
            let rendered = render_template(&message_template, &target.name, &phone);
            let outcome = match self.fonnte.send(&api_key, &phone, &rendered).await {
                Ok(o) => o,
                Err(e) => {
                    let _ = self
                        .db
                        .insert_log(contact_id, "failed", &api_key, Some(&e.to_string()))
                        .await;
                    self.bump_progress(app, idx + 1, total, &phone, "http-error", |p| {
                        p.failed += 1
                    })
                    .await;
                    continue;
                }
            };

            let status = if outcome.success { "success" } else { "failed" };
            let _ = self
                .db
                .insert_log(contact_id, status, &api_key, Some(&outcome.detail))
                .await;

            if outcome.success {
                self.db.mark_sent_today(contact_id, today).await?;
                self.bump_progress(app, idx + 1, total, &phone, "sent", |p| p.sent += 1)
                    .await;
                sent_in_batch += 1;
            } else {
                self.bump_progress(app, idx + 1, total, &phone, "failed", |p| p.failed += 1)
                    .await;
            }

            // ---- anti-ban delays ----
            // Per-message random delay.
            let per_msg = random_delay(
                settings.anti_ban.min_delay_secs,
                settings.anti_ban.max_delay_secs,
            );
            interruptible_sleep(per_msg, &self.cancel).await;

            // Batch pause.
            if settings.anti_ban.batch_size > 0 && sent_in_batch >= settings.anti_ban.batch_size {
                let pause = Duration::from_secs(settings.anti_ban.batch_pause_secs);
                {
                    let mut s = self.state.lock().await;
                    s.current_status = Some(format!(
                        "batch pause ({}s) after {} sent",
                        pause.as_secs(),
                        sent_in_batch
                    ));
                    emit_progress(app, &s);
                }
                interruptible_sleep(pause, &self.cancel).await;
                sent_in_batch = 0;
            }
        }

        Ok(())
    }

    async fn bump_progress<F>(
        &self,
        app: &AppHandle,
        processed: usize,
        total: usize,
        phone: &str,
        status: &str,
        f: F,
    ) where
        F: FnOnce(&mut BlastProgress),
    {
        let mut s = self.state.lock().await;
        s.total = total;
        s.processed = processed;
        s.current_phone = Some(phone.to_string());
        s.current_status = Some(status.to_string());
        f(&mut s);
        emit_progress(app, &s);
    }
}

fn emit_progress(app: &AppHandle, p: &BlastProgress) {
    let _ = app.emit("blast://progress", p);
}

/// Sleep for `dur`, but wake every 250ms so a cancel request stops promptly.
async fn interruptible_sleep(dur: Duration, cancel: &AtomicBool) {
    let step = Duration::from_millis(250);
    let mut remaining = dur;
    while remaining > Duration::ZERO {
        if cancel.load(Ordering::SeqCst) {
            break;
        }
        let s = remaining.min(step);
        tokio::time::sleep(s).await;
        remaining = remaining.saturating_sub(s);
    }
}

fn random_delay(min_secs: u64, max_secs: u64) -> Duration {
    if max_secs <= min_secs {
        return Duration::from_secs(min_secs);
    }
    let n = rand::thread_rng().gen_range(min_secs..=max_secs);
    Duration::from_secs(n)
}

/// Replace `{name}` and `{phone}` placeholders in the message template.
pub fn render_template(template: &str, name: &str, phone: &str) -> String {
    template.replace("{name}", name).replace("{phone}", phone)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_delay_within_bounds() {
        for _ in 0..50 {
            let d = random_delay(5, 10).as_secs();
            assert!((5..=10).contains(&d));
        }
    }

    #[test]
    fn template_renders_placeholders() {
        let out = render_template("hi {name}, your phone is {phone}", "Dandi", "62812");
        assert_eq!(out, "hi Dandi, your phone is 62812");
    }
}
