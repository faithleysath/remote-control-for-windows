# Project Scope

Remote Control for Windows provides a temporary, visible remote-assistance path
for authorized Windows support sessions. It is optimized for engineers and
Codex agents that need a scriptable control surface rather than a full
commercial remote-desktop product.

## Core Users

- Customer or tester: runs `rcw-host.exe`, keeps the window open while support is
  active, shares the visible machine ID and TOTP, and can stop control by closing
  the window.
- Engineer or operator: uses `rcwctl` to open a session, run diagnostics,
  transfer files, capture screenshots, and perform basic desktop input.
- Automation agent: calls `rcwctl`, preferably with `--json`, to complete
  bounded support tasks.
- Server operator: deploys `rcw-server`, manages the control token, and reviews
  relay-side logs.

## Supported v1 Workflows

- Host registration over outbound WebSocket.
- Session creation with control token plus machine ID/TOTP.
- Reusable local controller session file for short-lived `rcwctl` invocations.
- Remote command execution with stdout, stderr, exit code, timeout, and cleanup.
- Upload and download with chunking and SHA-256 verification.
- Screenshot capture from an interactive Windows desktop.
- Visible-window enumeration.
- Absolute-coordinate mouse move, click, and scroll.
- Text input and common key/shortcut injection.
- Host clipboard update with safe connection information.
- Temporary prevention of system sleep and display timeout while host is active.
- Host, controller, and server audit logs keyed by request ID.

## Safety Requirements

- `rcw-host.exe` stays visible as a console process.
- Closing the host window terminates control.
- The host does not install, persist, self-start, hide, inject into other
  processes, or register a service.
- The host does not auto-elevate or bypass UAC.
- Elevated host runs are allowed only when the customer or tester explicitly
  starts the process as administrator.
- Clipboard connection info must not include control tokens, session tokens,
  TOTP seeds, raw machine identifiers, or file contents.
- Audit logs must redact sensitive tokens and avoid recording full file content
  or default full command output.

## Current Baseline

The v1 chain is implemented across:

- `rcw-common`: protocol, config, IDs, TOTP, audit helpers, and transfer helpers.
- `rcw-server`: health check, host/control WebSocket endpoints, token checks,
  TOTP-mediated session creation, in-memory sessions, relay routing, close,
  status, ping/heartbeat behavior, rate-limiting basics, and server audit.
- `rcw-host`: Windows host connection, reconnect loop, machine ID/TOTP display,
  clipboard refresh, TOTP auth, command execution, file transfer, screenshot,
  windows/mouse/keyboard operations, audit, privilege display, and power guard.
- `rcwctl`: `open/status/exec/upload/download/screenshot/windows/move/click/
  scroll/type/key/close`, JSON output, session-file reuse, and controller audit.

As of 2026-06-11, this baseline was validated in a Windows VM except for the
remaining standard-user interactive desktop privilege proof documented in
[testing.md](testing.md).

## Non-Goals

The project does not aim to provide:

- Stealth control or hidden operation.
- Background persistence, startup registration, or service installation.
- UAC bypass or automatic privilege elevation.
- Kernel drivers, process injection, or keylogging.
- Real-time video streaming or screen recording.
- Multi-tenant SaaS administration, billing, or organization management.
- Central audit database in v1.
- P2P traversal in the current baseline.
