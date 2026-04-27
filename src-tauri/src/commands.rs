//! Tauri `#[command]` surface — the bridge between the JS frontend and Rust.

use std::path::PathBuf;
use std::sync::Arc;

use tauri::{AppHandle, State};

use crate::blaster::{BlastProgress, BlastTarget, BlasterEngine};
use crate::db::{Contact, DashboardStats, Db, LogRow};
use crate::error::AppResult;
use crate::import::{import_file, ColumnMap, ImportSummary, ImportedRow};
use crate::settings::{AppSettings, SettingsStore};

/// Bundle of long-lived handles managed by Tauri's state.
pub struct AppState {
    pub db: Db,
    pub settings: SettingsStore,
    pub blaster: BlasterEngine,
}

#[tauri::command]
pub async fn get_settings(state: State<'_, Arc<AppState>>) -> AppResult<AppSettings> {
    Ok(state.settings.snapshot())
}

#[tauri::command]
pub async fn save_settings(
    state: State<'_, Arc<AppState>>,
    settings: AppSettings,
) -> AppResult<AppSettings> {
    state.settings.replace(settings)?;
    Ok(state.settings.snapshot())
}

#[tauri::command]
pub async fn list_contacts(
    state: State<'_, Arc<AppState>>,
    limit: Option<i64>,
) -> AppResult<Vec<Contact>> {
    state.db.list_contacts(limit.unwrap_or(500)).await
}

#[tauri::command]
pub async fn upsert_contact(
    state: State<'_, Arc<AppState>>,
    phone: String,
    name: Option<String>,
) -> AppResult<i64> {
    state
        .db
        .upsert_contact(&phone, &name.unwrap_or_default())
        .await
}

#[tauri::command]
pub async fn delete_all_contacts(state: State<'_, Arc<AppState>>) -> AppResult<u64> {
    state.db.delete_all_contacts().await
}

#[tauri::command]
pub async fn dashboard_stats(state: State<'_, Arc<AppState>>) -> AppResult<DashboardStats> {
    state.db.dashboard_stats().await
}

#[tauri::command]
pub async fn recent_logs(
    state: State<'_, Arc<AppState>>,
    limit: Option<i64>,
) -> AppResult<Vec<LogRow>> {
    state.db.recent_logs(limit.unwrap_or(200)).await
}

#[tauri::command]
pub async fn import_contacts(
    state: State<'_, Arc<AppState>>,
    path: String,
    column_map: Option<ColumnMap>,
) -> AppResult<ImportSummary> {
    let path = PathBuf::from(path);
    let map = column_map.unwrap_or_default();
    let rows: Vec<ImportedRow> = import_file(&path, &map)?;

    let total_rows = rows.len();
    let mut imported = 0usize;
    let mut skipped = 0usize;
    for row in rows {
        if row.phone.trim().is_empty() {
            skipped += 1;
            continue;
        }
        match state.db.upsert_contact(&row.phone, &row.name).await {
            Ok(_) => imported += 1,
            Err(_) => skipped += 1,
        }
    }
    Ok(ImportSummary {
        total_rows,
        imported,
        skipped,
    })
}

#[tauri::command]
pub async fn start_blast(
    state: State<'_, Arc<AppState>>,
    app: AppHandle,
    targets: Vec<BlastTarget>,
    message: String,
) -> AppResult<()> {
    state.blaster.start(app, targets, message).await
}

#[tauri::command]
pub async fn stop_blast(state: State<'_, Arc<AppState>>) -> AppResult<()> {
    state.blaster.request_stop();
    Ok(())
}

#[tauri::command]
pub async fn blast_progress(state: State<'_, Arc<AppState>>) -> AppResult<BlastProgress> {
    Ok(state.blaster.snapshot().await)
}
