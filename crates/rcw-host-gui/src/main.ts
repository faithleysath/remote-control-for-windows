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

interface HostEvent {
  time: string;
  kind: HostEventKind;
  request_id?: string;
  session_id?: string;
  command?: string;
  status?: string;
  summary?: string;
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
  auth_requests: Array<unknown>;
  commands: Array<unknown>;
  transfers: Array<unknown>;
  tunnels: Array<unknown>;
  recent_errors: Array<{ at: string; summary: string }>;
}

interface AppState {
  snapshot: HostSnapshot | null;
  events: HostEvent[];
  loadError: string | null;
  loading: boolean;
}

const MAX_EVENTS = 24;
const app = document.querySelector<HTMLDivElement>("#app");

const state: AppState = {
  snapshot: null,
  events: [],
  loadError: null,
  loading: true,
};

if (!app) {
  throw new Error("missing app root");
}
const root = app;

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
  if (status === "connecting" || status === "reconnecting") {
    return "warn";
  }
  if (status === "error") {
    return "bad";
  }
  return "idle";
}

function renderMetric(label: string, value: string): string {
  return `
    <div class="metric">
      <span>${label}</span>
      <strong>${value}</strong>
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
      <span class="event-time">${formatDate(event.time)}</span>
      <span class="event-kind">${event.kind.replaceAll("_", " ")}</span>
      <span class="event-summary">${summary}</span>
      ${meta ? `<span class="event-meta">${meta}</span>` : ""}
    </li>
  `;
}

function render(): void {
  const snapshot = state.snapshot;
  const status = snapshot?.listener.status;
  const recentError = snapshot?.recent_errors.at(-1)?.summary;
  const activeSession = snapshot?.session.active_session_id ?? "No active session";

  root.innerHTML = `
    <main class="shell">
      <header class="topbar">
        <div>
          <p class="eyebrow">Remote Control Host</p>
          <h1>${snapshot?.machine_id ?? "Starting host"}</h1>
        </div>
        <div class="top-actions">
          <span class="status-pill ${statusClass(status)}">${statusLabel(status)}</span>
          <button id="refresh" type="button" ${state.loading ? "disabled" : ""}>Refresh</button>
        </div>
      </header>

      ${
        state.loadError
          ? `<div class="banner bad">${state.loadError}</div>`
          : recentError
            ? `<div class="banner warn">${recentError}</div>`
            : ""
      }

      <section class="summary-grid" aria-label="Host summary">
        <div class="panel primary">
          <span class="section-label">Connection</span>
          <dl>
            <div><dt>Server</dt><dd>${snapshot?.server_url ?? "Not configured"}</dd></div>
            <div><dt>Host ID</dt><dd>${snapshot?.host_id ?? "Pending"}</dd></div>
            <div><dt>Session</dt><dd>${activeSession}</dd></div>
            <div><dt>Controller</dt><dd>${snapshot?.session.controller_label ?? "None"}</dd></div>
          </dl>
        </div>

        <div class="panel code-panel">
          <span class="section-label">Current TOTP</span>
          <div class="totp">${snapshot?.totp.current_code ?? "------"}</div>
          <p>${snapshot ? `${snapshot.totp.remaining_seconds}s remaining of ${snapshot.totp.period_seconds}s` : "Waiting for host core"}</p>
        </div>

        <div class="panel">
          <span class="section-label">Runtime</span>
          ${renderMetric("Commands", formatCount(snapshot?.commands.length ?? 0))}
          ${renderMetric("Transfers", formatCount(snapshot?.transfers.length ?? 0))}
          ${renderMetric("Tunnels", formatCount(snapshot?.tunnels.length ?? 0))}
          ${renderMetric("Power guard", snapshot?.power.active ? "Active" : "Inactive")}
        </div>

        <div class="panel">
          <span class="section-label">Local audit</span>
          <p class="path">${snapshot?.audit_path ?? "Pending"}</p>
          <p class="muted">Updated ${formatDate(snapshot?.listener.updated_at)}</p>
        </div>
      </section>

      <section class="events" aria-label="Host events">
        <div class="events-header">
          <h2>Event Stream</h2>
          <span>${formatCount(state.events.length)} recent</span>
        </div>
        <ul>
          ${
            state.events.length > 0
              ? state.events.map(renderEvent).join("")
              : `<li class="empty">Waiting for host events</li>`
          }
        </ul>
      </section>
    </main>
  `;

  document.querySelector("#refresh")?.addEventListener("click", () => {
    void refreshSnapshot();
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

async function boot(): Promise<void> {
  render();
  await listen<HostEvent>("host-event", (event) => {
    state.events = [event.payload, ...state.events].slice(0, MAX_EVENTS);
    void refreshSnapshot();
  });
  await refreshSnapshot();
  window.setInterval(() => {
    void refreshSnapshot();
  }, 5_000);
}

void boot();
