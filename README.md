# WhatsApp Blaster (Fonnte) — Desktop

Desktop WhatsApp blaster built with **Rust + Tauri 2**, **SQLite (sqlx)**, and **Tailwind CSS**.
Uses the [Fonnte](https://docs.fonnte.com/) HTTP gateway to send WhatsApp messages.

> ⚠️ Use responsibly. Mass messaging may violate WhatsApp's Terms of Service and your local laws.
> This project is provided for legitimate transactional / opt-in marketing use only.

---

## Features

- **Multi‑account Fonnte** — store many API keys, switch manually or **load‑balance** (round‑robin).
- **Data input**
  - Manual one‑by‑one entry
  - Bulk upload via **Excel (.xlsx)** or **CSV** with column mapping for `Phone Number` and `Name`
- **Smart filtering** — automatically skips numbers already messaged **today** (checked against the
  `contacts.last_sent_date` column in SQLite) so you never double‑send in a day.
- **Anti‑ban configuration**
  - Configurable **random delay** between messages (e.g. 10–30s)
  - **Batch pause** — sleep N minutes after every M messages
- **Real‑time dashboard** — live progress bar, totals for sent / skipped / failed with Tauri events.
- **Settings page** — manage Fonnte API keys, delay window, batch size & batch pause.
- **Webhook (passive contact sync)** — embedded HTTP listener at
  `POST /fonnte-webhook` that auto-saves senders' phone + push-name into the
  `contacts` table whenever Fonnte forwards an incoming message. See
  [Passive contact sync](#passive-contact-sync-fonnte-webhook) below.

## Tech stack

| Layer    | Choice                                           |
|----------|--------------------------------------------------|
| Backend  | Rust, Tokio, Tauri 2                             |
| HTTP     | `reqwest` (Fonnte REST)                          |
| DB       | SQLite via `sqlx` with compile‑safe migrations   |
| Files    | `csv` + `calamine` (XLSX read)                   |
| Frontend | Static HTML + Tailwind CSS (CDN dev / CLI prod)  |

## Project layout

```
whatsapp-bomber/
├── README.md
├── .github/workflows/ci.yml          # cargo fmt / clippy / check
├── ui/                                # Tauri frontend (static)
│   ├── index.html                     # Dashboard
│   ├── settings.html                  # Settings page
│   ├── app.js                         # Tauri IPC + UI logic
│   └── styles.css                     # Tailwind utilities (optional precompiled)
└── src-tauri/
    ├── Cargo.toml
    ├── tauri.conf.json
    ├── build.rs
    ├── capabilities/default.json
    ├── icons/                         # app icons (placeholder)
    ├── migrations/
    │   └── 20250101000000_init.sql    # SQLite schema migration
    └── src/
        ├── main.rs                    # entry: launches Tauri
        ├── lib.rs                     # wires modules + Tauri builder
        ├── error.rs                   # AppError + IPC-friendly conversions
        ├── settings.rs                # SettingsStore (API keys, delays, batching)
        ├── db.rs                      # sqlx pool + repo helpers
        ├── fonnte.rs                  # Fonnte HTTP client w/ key rotation
        ├── import.rs                  # CSV / XLSX importer + column mapping
        ├── blaster.rs                 # async send-loop: delay, batching, dedup
        └── commands.rs                # Tauri #[command] surface (UI <-> Rust)
```

## Database schema

Stored in `${APP_DATA}/whatsapp-bomber/app.db`. Created automatically on first launch by sqlx
migrations in [`src-tauri/migrations`](src-tauri/migrations).

```sql
CREATE TABLE contacts (
  id              INTEGER PRIMARY KEY AUTOINCREMENT,
  phone           TEXT    NOT NULL UNIQUE,
  name            TEXT    NOT NULL DEFAULT '',
  last_sent_date  TEXT,                       -- ISO yyyy-mm-dd, used by smart-filter
  created_at      TEXT    NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE logs (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  contact_id    INTEGER NOT NULL REFERENCES contacts(id) ON DELETE CASCADE,
  status        TEXT    NOT NULL CHECK(status IN ('success','failed','skipped')),
  api_key_used  TEXT    NOT NULL,
  detail        TEXT,
  timestamp     TEXT    NOT NULL DEFAULT (datetime('now'))
);
```

## Running locally

```bash
# 1. Install Rust 1.83+ and Tauri prerequisites for your OS
#    https://tauri.app/start/prerequisites/
#
# 2. Install the Tauri CLI
cargo install tauri-cli --version "^2.0" --locked

# 3. From the repo root:
cargo tauri dev          # launches the desktop app in dev mode
cargo tauri build        # release bundle
```

On first run the app:
1. Creates `app.db` in the OS app‑data dir and applies migrations.
2. Loads settings from `settings.json` (next to `app.db`); you can edit them in‑app on the
   Settings page.

## Configuring Fonnte

Add one or more API keys via **Settings → API Keys**. Keys are persisted to `settings.json`.
The blaster picks a key per message using round‑robin; failed keys are reported in the logs
table along with the response detail returned by Fonnte.

## Passive contact sync (Fonnte webhook)

Fonnte's documented API does **not** expose a "list contacts of connected
device" endpoint, so this app uses Fonnte's webhook instead: every time someone
sends a message to your connected WhatsApp number, Fonnte forwards an event to
your URL — and the app auto-saves the sender's phone + push-name into the
`contacts` table.

**Setup**:

1. **Settings → Webhook** → enable + pick a port (default `8787`) → Save.
   The dashboard's *Webhook* card should flip to `online · :8787`.
2. **Expose the local port to the internet** (Fonnte's servers must reach it):
   ```bash
   # ngrok
   ngrok http 8787
   # cloudflared
   cloudflared tunnel --url http://localhost:8787
   ```
3. Copy the public URL and append `/fonnte-webhook`, e.g.
   `https://abc123.ngrok.io/fonnte-webhook`.
4. In your Fonnte dashboard, paste that URL into **Device → Webhook URL**.
5. Send a test WhatsApp message to your connected device — the contact should
   appear in the dashboard, and the *Events received* counter should tick up.

The endpoint expects Fonnte's documented JSON payload (fields `sender`,
`name`, `member` etc.) — see
[`webhook.rs`](src-tauri/src/webhook.rs) for the exact parser.

## CI

GitHub Actions runs `cargo fmt --check`, `cargo clippy -- -D warnings`, and `cargo check` on
every push / PR — see [`.github/workflows/ci.yml`](.github/workflows/ci.yml).
