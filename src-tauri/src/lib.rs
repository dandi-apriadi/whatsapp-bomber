//! Library entry-point. `main.rs` simply calls [`run`].

mod blaster;
mod commands;
mod db;
mod error;
mod fonnte;
mod import;
mod settings;
mod webhook;

use std::sync::Arc;

use tauri::Manager;

use crate::blaster::BlasterEngine;
use crate::commands::AppState;
use crate::db::Db;
use crate::fonnte::FonnteClient;
use crate::settings::SettingsStore;
use crate::webhook::WebhookManager;

/// Build and run the desktop application.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // tracing — useful both in dev (`RUST_LOG=info cargo tauri dev`) and prod.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .try_init();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .setup(|app| {
            let app_data_dir = app
                .path()
                .app_data_dir()
                .expect("Tauri must provide an app data dir");
            std::fs::create_dir_all(&app_data_dir).ok();

            let db_path = app_data_dir.join("app.db");
            let settings_path = app_data_dir.join("settings.json");

            // Tauri's setup hook is sync; bridge to async via the runtime that
            // Tauri already runs on.
            let db = tauri::async_runtime::block_on(Db::connect(&db_path))
                .expect("failed to open SQLite database");
            let settings = SettingsStore::load(&settings_path).expect("failed to load settings");
            let fonnte = FonnteClient::new();
            let blaster = BlasterEngine::new(db.clone(), settings.clone(), fonnte.clone());
            let webhook = WebhookManager::new(db.clone());

            // Auto-start the webhook on launch if it is enabled in settings.
            let snap = settings.snapshot();
            if snap.webhook.enabled {
                let mgr = webhook.clone();
                let handle = app.handle().clone();
                let port = snap.webhook.port;
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = mgr.start(handle, port).await {
                        tracing::error!("failed to start webhook on launch: {e}");
                    }
                });
            }

            app.manage(Arc::new(AppState {
                db,
                settings,
                blaster,
                webhook,
            }));

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_settings,
            commands::save_settings,
            commands::list_contacts,
            commands::upsert_contact,
            commands::delete_all_contacts,
            commands::dashboard_stats,
            commands::recent_logs,
            commands::import_contacts,
            commands::start_blast,
            commands::stop_blast,
            commands::blast_progress,
            commands::webhook_status,
            commands::restart_webhook,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
