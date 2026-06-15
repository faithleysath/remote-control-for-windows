import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./styles.css";

type ListenerStatus =
  | "stopped"
  | "connecting"
  | "connected"
  | "reconnecting"
  | "stopping"
  | "error";

type HostEventKind =
  | "listener_status_changed"
  | "host_connected"
  | "host_disconnected"
  | "auth_requested"
  | "session_opened"
  | "session_closed"
  | "command_started"
  | "command_completed"
  | "command_cancel_requested"
  | "transfer_started"
  | "transfer_progress"
  | "transfer_completed"
  | "tunnel_opened"
  | "tunnel_closed"
  | "error_recorded";

type Tab = "overview" | "session" | "settings";
type NoticeTone = "ok" | "warn" | "bad";

interface HostEvent {
  time: string;
  kind: HostEventKind;
  request_id?: string;
  session_id?: string;
  command?: string;
  status?: string;
  summary?: string;
}

interface HostAuthRequest {
  request_id: string;
  controller_label: string;
  at: string;
  ok: boolean;
}

interface HostSnapshot {
  listener: {
    status: ListenerStatus;
    updated_at: string;
    last_error?: string;
  };
  server_url: string;
  machine_id: string;
  host_id: string;
  totp: {
    current_code: string;
    period_seconds: number;
    remaining_seconds: number;
  };
  power: {
    active: boolean;
    warning?: string;
  };
  audit_path: string;
  session: {
    active_session_id?: string;
    controller_label?: string;
    opened_at?: string;
    last_closed_at?: string;
    last_close_reason?: string;
  };
  auth_requests: HostAuthRequest[];
  commands: unknown[];
  transfers: unknown[];
  tunnels: unknown[];
  recent_errors: Array<{ at: string; summary: string }>;
}

interface HostSettingsView {
  server_url: string;
  totp_period_seconds: number;
  audit_log_path: string;
  auto_listen: boolean;
  config_path: string;
  restart_required: boolean;
}

interface HostSettingsInput {
  server_url: string;
  totp_period_seconds: number;
  audit_log_path: string;
  auto_listen: boolean;
}

interface SettingsFormState {
  server_url: string;
  totp_period_seconds: string;
  audit_log_path: string;
  auto_listen: boolean;
}

interface HostActionOutcome {
  changed: boolean;
  message: string;
  snapshot: HostSnapshot;
}

interface HostCopyOutcome {
  copied: boolean;
  error?: string;
  info: {
    server_url: string;
    machine_id: string;
    host_id: string;
    totp: string;
    totp_period_seconds: number;
    clipboard_text: string;
  };
}

interface AppState {
  snapshot: HostSnapshot | null;
  settings: HostSettingsView | null;
  settingsForm: SettingsFormState;
  settingsDirty: boolean;
  events: HostEvent[];
  loadError: string | null;
  notice: { tone: NoticeTone; text: string } | null;
  loading: boolean;
  busyAction: string | null;
  activeTab: Tab;
}

const MAX_EVENTS = 24;
const app = document.querySelector<HTMLDivElement>("#app");

const state: AppState = {
  snapshot: null,
  settings: null,
  settingsForm: emptySettingsForm(),
  settingsDirty: false,
  events: [],
  loadError: null,
  notice: null,
  loading: true,
  busyAction: null,
  activeTab: "overview",
};

if (!app) {
  throw new Error("missing app root");
}
const root = app;

function emptySettingsForm(): SettingsFormState {
  return {
    server_url: "",
    totp_period_seconds: "120",
    audit_log_path: "",
    auto_listen: true,
  };
}

function formFromSettings(settings: HostSettingsView): SettingsFormState {
  return {
    server_url: settings.server_url,
    totp_period_seconds: String(settings.totp_period_seconds),
    audit_log_path: settings.audit_log_path,
    auto_listen: settings.auto_listen,
  };
}

function escapeHtml(value: unknown): string {
  return String(value ?? "").replace(/[&<>"']/g, (character) => {
    switch (character) {
      case "&":
        return "&amp;";
      case "<":
        return "&lt;";
      case ">":
        return "&gt;";
      case '"':
        return "&quot;";
      default:
        return "&#39;";
    }
  });
}

function formatDate(value?: string): string {
  if (!value) {
    return "Never";
  }
  const parsed = new Date(value);
  return Number.isNaN(parsed.valueOf()) ? value : parsed.toLocaleString();
}

function formatCount(value: number): string {
  return new Intl.NumberFormat().format(value);
}

function statusLabel(status?: ListenerStatus): string {
  if (!status) {
    return "Unknown";
  }
  return status.replaceAll("_", " ");
}

function statusClass(status?: ListenerStatus): string {
  if (status === "connected") {
    return "ok";
  }
  if (status === "connecting" || status === "reconnecting" || status === "stopping") {
    return "warn";
  }
  if (status === "error") {
    return "bad";
  }
  return "idle";
}

function disabledAttr(disabled: boolean): string {
  return disabled ? "disabled" : "";
}

function checkedAttr(checked: boolean): string {
  return checked ? "checked" : "";
}

function busyLabel(action: string, label: string): string {
  return state.busyAction === action ? "Working" : label;
}

function display(value: unknown, fallback = "None"): string {
  const text = String(value ?? "").trim();
  return escapeHtml(text || fallback);
}

function renderMetric(label: string, value: string): string {
  return `
    <div class="metric">
      <span>${escapeHtml(label)}</span>
      <strong>${escapeHtml(value)}</strong>
    </div>
  `;
}

function renderDataRow(label: string, value: unknown, options: { code?: boolean } = {}): string {
  return `
    <div>
      <dt>${escapeHtml(label)}</dt>
      <dd class="${options.code ? "mono" : ""}">${display(value)}</dd>
    </div>
  `;
}

function renderEvent(event: HostEvent): string {
  const summary = event.summary ?? event.status ?? event.kind;
  const meta = [event.command, event.request_id, event.session_id]
    .filter(Boolean)
    .join(" · ");
  return `
    <li class="event-row">
      <span class="event-time">${escapeHtml(formatDate(event.time))}</span>
      <span class="event-kind">${escapeHtml(event.kind.replaceAll("_", " "))}</span>
      <span class="event-summary">${escapeHtml(summary)}</span>
      ${meta ? `<span class="event-meta">${escapeHtml(meta)}</span>` : ""}
    </li>
  `;
}

function renderAuthRequest(request: HostAuthRequest): string {
  return `
    <li class="auth-row">
      <span class="mono">${escapeHtml(request.request_id)}</span>
      <span>${display(request.controller_label)}</span>
      <span>${escapeHtml(formatDate(request.at))}</span>
      <span class="result ${request.ok ? "ok" : "bad"}">${request.ok ? "OK" : "Failed"}</span>
    </li>
  `;
}

function renderTabs(): string {
  const tabs: Array<{ id: Tab; label: string }> = [
    { id: "overview", label: "Overview" },
    { id: "session", label: "Session" },
    { id: "settings", label: "Settings" },
  ];
  return `
    <nav class="tabs" aria-label="Host pages">
      ${tabs
        .map(
          (tab) => `
            <button
              id="tab-${tab.id}"
              class="tab ${state.activeTab === tab.id ? "active" : ""}"
              type="button"
              aria-current="${state.activeTab === tab.id ? "page" : "false"}"
            >
              ${escapeHtml(tab.label)}
            </button>
          `,
        )
        .join("")}
    </nav>
  `;
}

function renderNotice(): string {
  if (state.loadError) {
    return `<div class="banner bad">${escapeHtml(state.loadError)}</div>`;
  }
  if (state.notice) {
    return `<div class="banner ${state.notice.tone}">${escapeHtml(state.notice.text)}</div>`;
  }
  if (state.settings?.restart_required) {
    return `<div class="banner warn">Saved settings are pending listener restart.</div>`;
  }
  const recentError = state.snapshot?.recent_errors.at(-1)?.summary;
  return recentError ? `<div class="banner warn">${escapeHtml(recentError)}</div>` : "";
}

function renderOverview(snapshot: HostSnapshot | null): string {
  const status = snapshot?.listener.status;
  const isStopped = status === "stopped";
  const isBusy = state.busyAction !== null;
  const activeSession = snapshot?.session.active_session_id ?? "No active session";

  return `
    <section class="summary-grid" aria-label="Host summary">
      <div class="panel primary">
        <div class="panel-heading">
          <span class="section-label">Connection</span>
          <div class="button-row">
            <button id="start-listener" type="button" ${disabledAttr(!snapshot || !isStopped || isBusy)}>
              ${busyLabel("start", "Start")}
            </button>
            <button id="stop-listener" type="button" ${disabledAttr(!snapshot || isStopped || isBusy)}>
              ${busyLabel("stop", "Stop")}
            </button>
            <button id="restart-listener" type="button" ${disabledAttr(!snapshot || isBusy)}>
              ${busyLabel("restart", "Reconnect")}
            </button>
            <button id="copy-connection" type="button" ${disabledAttr(!snapshot || isBusy)}>
              ${busyLabel("copy", "Copy")}
            </button>
          </div>
        </div>
        <dl>
          ${renderDataRow("Server", snapshot?.server_url ?? "Not configured", { code: true })}
          ${renderDataRow("Machine ID", snapshot?.machine_id ?? "Pending", { code: true })}
          ${renderDataRow("Host ID", snapshot?.host_id ?? "Pending", { code: true })}
          ${renderDataRow("Session", activeSession, { code: Boolean(snapshot?.session.active_session_id) })}
          ${renderDataRow("Controller", snapshot?.session.controller_label ?? "None")}
        </dl>
      </div>

      <div class="panel code-panel">
        <span class="section-label">Current TOTP</span>
        <div class="totp">${escapeHtml(snapshot?.totp.current_code ?? "------")}</div>
        <p>
          ${
            snapshot
              ? `${escapeHtml(snapshot.totp.remaining_seconds)}s remaining of ${escapeHtml(snapshot.totp.period_seconds)}s`
              : "Waiting for host core"
          }
        </p>
      </div>

      <div class="panel">
        <span class="section-label">Runtime</span>
        ${renderMetric("Commands", formatCount(snapshot?.commands.length ?? 0))}
        ${renderMetric("Transfers", formatCount(snapshot?.transfers.length ?? 0))}
        ${renderMetric("Tunnels", formatCount(snapshot?.tunnels.length ?? 0))}
        ${renderMetric("Power guard", snapshot?.power.active ? "Active" : "Inactive")}
      </div>

      <div class="panel wide">
        <span class="section-label">Local audit</span>
        <p class="path">${display(snapshot?.audit_path, "Pending")}</p>
        <p class="muted">Listener updated ${escapeHtml(formatDate(snapshot?.listener.updated_at))}</p>
      </div>
    </section>
    ${renderEvents()}
  `;
}

function renderSession(snapshot: HostSnapshot | null): string {
  const session = snapshot?.session;
  const hasActiveSession = Boolean(session?.active_session_id);
  return `
    <section class="two-column">
      <div class="panel">
        <div class="panel-heading">
          <span class="section-label">Current session</span>
          <button
            id="close-session"
            type="button"
            ${disabledAttr(!hasActiveSession || state.busyAction !== null)}
          >
            ${busyLabel("close-session", "End Session")}
          </button>
        </div>
        <dl>
          ${renderDataRow("Controller", session?.controller_label ?? "None")}
          ${renderDataRow("Session ID", session?.active_session_id ?? "No active session", { code: hasActiveSession })}
          ${renderDataRow("Opened", formatDate(session?.opened_at))}
          ${renderDataRow("Last closed", formatDate(session?.last_closed_at))}
          ${renderDataRow("Close reason", session?.last_close_reason ?? "None")}
        </dl>
      </div>

      <div class="panel">
        <span class="section-label">Auth requests</span>
        <ul class="auth-list">
          ${
            snapshot && snapshot.auth_requests.length > 0
              ? snapshot.auth_requests.slice().reverse().map(renderAuthRequest).join("")
              : `<li class="empty">No auth requests</li>`
          }
        </ul>
      </div>
    </section>
    ${renderEvents()}
  `;
}

function renderSettings(snapshot: HostSnapshot | null): string {
  const form = state.settingsForm;
  const settings = state.settings;
  return `
    <section class="settings-layout">
      <form id="settings-form" class="panel settings-form">
        <span class="section-label">Settings</span>
        <label>
          <span>Server URL</span>
          <input
            id="settings-server-url"
            name="server_url"
            type="url"
            value="${escapeHtml(form.server_url)}"
            autocomplete="off"
            required
          />
        </label>
        <label>
          <span>TOTP period</span>
          <input
            id="settings-totp-period"
            name="totp_period_seconds"
            type="number"
            min="1"
            step="1"
            value="${escapeHtml(form.totp_period_seconds)}"
            required
          />
        </label>
        <label>
          <span>Audit log path</span>
          <input
            id="settings-audit-log"
            name="audit_log_path"
            type="text"
            value="${escapeHtml(form.audit_log_path)}"
            placeholder="${escapeHtml(snapshot?.audit_path ?? "Default host audit path")}"
            autocomplete="off"
          />
        </label>
        <label class="check-row">
          <input
            id="settings-auto-listen"
            name="auto_listen"
            type="checkbox"
            ${checkedAttr(form.auto_listen)}
          />
          <span>Start listener on launch</span>
        </label>
        <button id="save-settings" type="submit" ${disabledAttr(state.busyAction !== null)}>
          ${busyLabel("save-settings", "Save Settings")}
        </button>
      </form>

      <div class="panel">
        <span class="section-label">Effective runtime</span>
        <dl>
          ${renderDataRow("Server", snapshot?.server_url ?? "Pending", { code: true })}
          ${renderDataRow("TOTP period", snapshot ? `${snapshot.totp.period_seconds}s` : "Pending")}
          ${renderDataRow("Audit path", snapshot?.audit_path ?? "Pending", { code: true })}
          ${renderDataRow("Config file", settings?.config_path ?? "Pending", { code: true })}
          ${renderDataRow("Pending restart", settings?.restart_required ? "Yes" : "No")}
        </dl>
      </div>
    </section>
  `;
}

function renderEvents(): string {
  return `
    <section class="events" aria-label="Host events">
      <div class="events-header">
        <h2>Event Stream</h2>
        <span>${escapeHtml(formatCount(state.events.length))} recent</span>
      </div>
      <ul>
        ${
          state.events.length > 0
            ? state.events.map(renderEvent).join("")
            : `<li class="empty">Waiting for host events</li>`
        }
      </ul>
    </section>
  `;
}

function renderContent(): string {
  if (state.activeTab === "session") {
    return renderSession(state.snapshot);
  }
  if (state.activeTab === "settings") {
    return renderSettings(state.snapshot);
  }
  return renderOverview(state.snapshot);
}

function render(): void {
  const snapshot = state.snapshot;
  const status = snapshot?.listener.status;

  root.innerHTML = `
    <main class="shell">
      <header class="topbar">
        <div>
          <p class="eyebrow">Remote Control Host</p>
          <h1>${display(snapshot?.machine_id, "Starting host")}</h1>
        </div>
        <div class="top-actions">
          <span class="status-pill ${statusClass(status)}">${escapeHtml(statusLabel(status))}</span>
          <button id="refresh" type="button" ${disabledAttr(state.loading || state.busyAction !== null)}>
            ${state.loading ? "Refreshing" : "Refresh"}
          </button>
        </div>
      </header>

      ${renderTabs()}
      ${renderNotice()}
      ${renderContent()}
    </main>
  `;

  bindUi();
}

function bindUi(): void {
  bindClick("#refresh", () => {
    void refreshSnapshot();
  });
  bindClick("#tab-overview", () => {
    switchTab("overview");
  });
  bindClick("#tab-session", () => {
    switchTab("session");
  });
  bindClick("#tab-settings", () => {
    switchTab("settings");
  });
  bindClick("#start-listener", () => {
    void runAction("start", "host_start_listener");
  });
  bindClick("#stop-listener", () => {
    void runAction("stop", "host_stop_listener");
  });
  bindClick("#restart-listener", () => {
    void runAction("restart", "host_restart_listener");
  });
  bindClick("#close-session", () => {
    void runAction("close-session", "host_close_current_session");
  });
  bindClick("#copy-connection", () => {
    void copyConnectionInfo();
  });

  bindSettingsForm();
}

function bindClick(selector: string, handler: () => void): void {
  document.querySelector(selector)?.addEventListener("click", handler);
}

function switchTab(tab: Tab): void {
  state.activeTab = tab;
  state.notice = null;
  render();
}

function bindSettingsForm(): void {
  bindTextInput("#settings-server-url", (value) => {
    state.settingsForm.server_url = value;
  });
  bindTextInput("#settings-totp-period", (value) => {
    state.settingsForm.totp_period_seconds = value;
  });
  bindTextInput("#settings-audit-log", (value) => {
    state.settingsForm.audit_log_path = value;
  });

  const autoListen = document.querySelector<HTMLInputElement>("#settings-auto-listen");
  autoListen?.addEventListener("change", () => {
    state.settingsDirty = true;
    state.settingsForm.auto_listen = autoListen.checked;
  });

  const form = document.querySelector<HTMLFormElement>("#settings-form");
  form?.addEventListener("submit", (event) => {
    event.preventDefault();
    void saveSettings();
  });
}

function bindTextInput(selector: string, update: (value: string) => void): void {
  const input = document.querySelector<HTMLInputElement>(selector);
  input?.addEventListener("input", () => {
    state.settingsDirty = true;
    update(input.value);
  });
}

async function refreshSnapshot(): Promise<void> {
  state.loading = true;
  state.loadError = null;
  render();

  try {
    state.snapshot = await invoke<HostSnapshot>("host_snapshot");
  } catch (error) {
    state.loadError = error instanceof Error ? error.message : String(error);
  } finally {
    state.loading = false;
    render();
  }
}

async function refreshSettings(updateForm: boolean): Promise<void> {
  state.settings = await invoke<HostSettingsView>("host_settings");
  if (updateForm || !state.settingsDirty) {
    state.settingsForm = formFromSettings(state.settings);
    state.settingsDirty = false;
  }
}

async function runAction(action: string, command: string): Promise<void> {
  state.busyAction = action;
  state.loadError = null;
  state.notice = null;
  render();

  try {
    const outcome = await invoke<HostActionOutcome>(command);
    state.snapshot = outcome.snapshot;
    state.notice = { tone: outcome.changed ? "ok" : "warn", text: outcome.message };
    await refreshSettings(false);
  } catch (error) {
    state.notice = { tone: "bad", text: error instanceof Error ? error.message : String(error) };
  } finally {
    state.busyAction = null;
    render();
  }
}

async function copyConnectionInfo(): Promise<void> {
  state.busyAction = "copy";
  state.loadError = null;
  state.notice = null;
  render();

  try {
    const outcome = await invoke<HostCopyOutcome>("host_copy_connection_info");
    let copied = outcome.copied;
    if (!copied) {
      try {
        await navigator.clipboard.writeText(outcome.info.clipboard_text);
        copied = true;
      } catch {
        copied = false;
      }
    }
    state.notice = copied
      ? { tone: "ok", text: "Connection info copied" }
      : {
          tone: "warn",
          text: outcome.error
            ? `Connection info ready; clipboard copy failed: ${outcome.error}`
            : "Connection info ready; clipboard copy failed",
        };
    await refreshSnapshot();
  } catch (error) {
    state.notice = { tone: "bad", text: error instanceof Error ? error.message : String(error) };
  } finally {
    state.busyAction = null;
    render();
  }
}

async function saveSettings(): Promise<void> {
  const period = Number(state.settingsForm.totp_period_seconds);
  if (!Number.isInteger(period) || period <= 0) {
    state.notice = { tone: "bad", text: "TOTP period must be a positive integer" };
    render();
    return;
  }

  const input: HostSettingsInput = {
    server_url: state.settingsForm.server_url,
    totp_period_seconds: period,
    audit_log_path: state.settingsForm.audit_log_path,
    auto_listen: state.settingsForm.auto_listen,
  };

  state.busyAction = "save-settings";
  state.loadError = null;
  state.notice = null;
  render();

  try {
    state.settings = await invoke<HostSettingsView>("host_save_settings", { input });
    state.settingsForm = formFromSettings(state.settings);
    state.settingsDirty = false;
    state.notice = {
      tone: state.settings.restart_required ? "warn" : "ok",
      text: state.settings.restart_required
        ? "Settings saved; restart the listener to apply runtime changes"
        : "Settings saved",
    };
    await refreshSnapshot();
  } catch (error) {
    state.notice = { tone: "bad", text: error instanceof Error ? error.message : String(error) };
  } finally {
    state.busyAction = null;
    render();
  }
}

async function boot(): Promise<void> {
  render();
  await listen<HostEvent>("host-event", (event) => {
    state.events = [event.payload, ...state.events].slice(0, MAX_EVENTS);
    void refreshSnapshot();
  });
  try {
    await refreshSettings(true);
  } catch (error) {
    state.loadError = error instanceof Error ? error.message : String(error);
  }
  await refreshSnapshot();
  window.setInterval(() => {
    void refreshSnapshot();
  }, 5_000);
}

void boot();
