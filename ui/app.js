// Tauri JS bridge. Loaded by both index.html (dashboard) and settings.html.
// Detects which page is active by checking for known DOM ids.

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { open } = window.__TAURI__.dialog;

// In-memory queue of contacts to blast (built up before pressing "Start blast").
const queue = [];

document.addEventListener("DOMContentLoaded", async () => {
  if (document.getElementById("api-keys")) {
    await initSettingsPage();
  } else {
    await initDashboard();
  }
});

// -----------------------------------------------------------------------------
// Dashboard page
// -----------------------------------------------------------------------------
async function initDashboard() {
  await refreshStats();
  await refreshLogs();
  await refreshContacts();

  document.getElementById("btn-import").addEventListener("click", onImportClick);
  document.getElementById("btn-add-manual").addEventListener("click", onAddManual);
  document.getElementById("btn-start").addEventListener("click", onStartBlast);
  document.getElementById("btn-stop").addEventListener("click", () => invoke("stop_blast"));
  document.getElementById("btn-clear-contacts").addEventListener("click", async () => {
    if (!confirm("Delete ALL contacts from the local database?")) return;
    await invoke("delete_all_contacts");
    queue.length = 0;
    renderQueue();
    await refreshStats();
  });

  await listen("blast://progress", (e) => onProgress(e.payload));
}

async function onImportClick() {
  const path = await open({
    multiple: false,
    filters: [
      { name: "Spreadsheet", extensions: ["csv", "xlsx", "xls", "tsv"] },
    ],
  });
  if (!path) return;
  try {
    const summary = await invoke("import_contacts", {
      path,
      columnMap: { phone: "phone", name: "name" },
    });
    setStatus(
      `Imported ${summary.imported} / ${summary.total_rows} (skipped ${summary.skipped})`,
    );
    await refreshContacts();
    await refreshStats();
  } catch (err) {
    setStatus(`Import failed: ${err}`, true);
  }
}

function onAddManual() {
  const phone = prompt("Phone (international format, e.g. 6281234567890):");
  if (!phone) return;
  const name = prompt("Name (optional):") || "";
  queue.push({ phone, name });
  renderQueue();
}

async function onStartBlast() {
  const message = document.getElementById("msg-template").value.trim();
  if (!message) {
    setStatus("Message is empty", true);
    return;
  }

  // If the queue is empty, fall back to all stored contacts so users can
  // re-blast everything in the DB.
  let targets = queue.slice();
  if (targets.length === 0) {
    const contacts = await invoke("list_contacts", { limit: 10000 });
    targets = contacts.map((c) => ({ phone: c.phone, name: c.name }));
  }

  if (targets.length === 0) {
    setStatus("No contacts to send to. Import a file or add one manually.", true);
    return;
  }

  try {
    await invoke("start_blast", { targets, message });
    document.getElementById("btn-stop").classList.remove("hidden");
    setStatus(`Started blast for ${targets.length} contacts.`);
  } catch (err) {
    setStatus(`Failed to start: ${err}`, true);
  }
}

function onProgress(p) {
  document.getElementById("stat-sent").textContent = p.sent;
  document.getElementById("stat-skipped").textContent = p.skipped;
  document.getElementById("stat-failed").textContent = p.failed;

  const pct = p.total ? Math.floor((p.processed / p.total) * 100) : 0;
  document.getElementById("progress-bar").style.width = `${pct}%`;
  document.getElementById("progress-label").textContent = `${p.processed} / ${p.total}`;
  document.getElementById("progress-current").textContent =
    p.current_phone ? `${p.current_phone} — ${p.current_status ?? ""}` : "";

  if (!p.running) {
    document.getElementById("btn-stop").classList.add("hidden");
    refreshStats();
    refreshLogs();
  }
}

async function refreshStats() {
  const s = await invoke("dashboard_stats");
  document.getElementById("stat-contacts").textContent = s.total_contacts;
  document.getElementById("stat-sent").textContent = s.total_sent;
  document.getElementById("stat-skipped").textContent = s.total_skipped;
  document.getElementById("stat-failed").textContent = s.total_failed;
}

async function refreshContacts() {
  const list = document.getElementById("contacts-list");
  if (!list) return;
  const contacts = await invoke("list_contacts", { limit: 200 });
  if (contacts.length === 0 && queue.length === 0) {
    list.innerHTML =
      '<div class="text-slate-500 text-xs py-4">No contacts yet. Import a file or add one manually.</div>';
    return;
  }
  const rows = contacts
    .map(
      (c) => `
        <div class="py-2 flex items-center justify-between">
          <div>
            <div class="font-medium">${escapeHtml(c.name || "(no name)")}</div>
            <div class="text-xs text-slate-400">${escapeHtml(c.phone)}</div>
          </div>
          <div class="text-xs text-slate-500">${c.last_sent_date ? "sent " + c.last_sent_date : ""}</div>
        </div>`,
    )
    .join("");
  list.innerHTML = rows;
  renderQueue();
}

function renderQueue() {
  if (queue.length === 0) return;
  const list = document.getElementById("contacts-list");
  if (!list) return;
  const queued = queue
    .map(
      (c) => `
        <div class="py-2 flex items-center justify-between bg-emerald-500/5">
          <div>
            <div class="font-medium">${escapeHtml(c.name || "(no name)")}</div>
            <div class="text-xs text-emerald-300">${escapeHtml(c.phone)} · queued</div>
          </div>
        </div>`,
    )
    .join("");
  list.insertAdjacentHTML("afterbegin", queued);
}

async function refreshLogs() {
  const tbody = document.getElementById("logs-body");
  if (!tbody) return;
  const logs = await invoke("recent_logs", { limit: 100 });
  tbody.innerHTML = logs
    .map(
      (l) => `
        <tr>
          <td class="py-1.5 pr-3 text-slate-400">${escapeHtml(l.timestamp)}</td>
          <td class="${statusClass(l.status)}">${escapeHtml(l.status)}</td>
          <td>#${l.contact_id}</td>
          <td class="font-mono text-xs">${escapeHtml(maskKey(l.api_key_used))}</td>
          <td class="text-slate-300">${escapeHtml(l.detail ?? "")}</td>
        </tr>`,
    )
    .join("");
}

function statusClass(s) {
  if (s === "success") return "text-emerald-400";
  if (s === "skipped") return "text-amber-300";
  return "text-rose-300";
}

function maskKey(k) {
  if (!k || k.length <= 6) return k || "";
  return k.slice(0, 3) + "…" + k.slice(-3);
}

function setStatus(msg, isErr = false) {
  const el = document.getElementById("status-line") || document.getElementById("save-status");
  if (!el) return;
  el.textContent = msg;
  el.className = isErr ? "text-xs text-rose-300" : "text-xs text-emerald-300";
}

// -----------------------------------------------------------------------------
// Settings page
// -----------------------------------------------------------------------------
async function initSettingsPage() {
  const s = await invoke("get_settings");
  document.getElementById("api-keys").value = (s.api_keys ?? []).join("\n");
  document.getElementById("account-mode").value = s.account_mode || "round_robin";
  document.getElementById("min-delay").value = s.anti_ban?.min_delay_secs ?? 10;
  document.getElementById("max-delay").value = s.anti_ban?.max_delay_secs ?? 30;
  document.getElementById("batch-size").value = s.anti_ban?.batch_size ?? 50;
  document.getElementById("batch-pause").value = s.anti_ban?.batch_pause_secs ?? 300;

  document.getElementById("btn-save").addEventListener("click", async () => {
    const payload = {
      api_keys: document
        .getElementById("api-keys")
        .value.split(/\r?\n/)
        .map((s) => s.trim())
        .filter(Boolean),
      account_mode: document.getElementById("account-mode").value,
      anti_ban: {
        min_delay_secs: parseInt(document.getElementById("min-delay").value, 10) || 0,
        max_delay_secs: parseInt(document.getElementById("max-delay").value, 10) || 0,
        batch_size: parseInt(document.getElementById("batch-size").value, 10) || 1,
        batch_pause_secs: parseInt(document.getElementById("batch-pause").value, 10) || 0,
      },
    };
    try {
      await invoke("save_settings", { settings: payload });
      setStatus("Saved.");
    } catch (err) {
      setStatus(`Save failed: ${err}`, true);
    }
  });
}

function escapeHtml(s) {
  return String(s)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}
