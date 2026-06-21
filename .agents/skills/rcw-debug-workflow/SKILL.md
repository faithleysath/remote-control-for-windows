---
name: rcw-debug-workflow
description: "Use when debugging the remote-control-for-windows repo on this machine: selecting the validated console, MCP, or GUI path; syncing the repo's Windows debug scripts; and following the repo's debug-workflow document instead of ad hoc commands."
---

# rcw-debug-workflow

Use this skill for real debugging work in the `rcw` repository when the task involves:

- reproducing or validating behavior in `/home/laysath/Projects/remote-control-for-windows`
- choosing between the validated console host, MCP, or GUI/CDP debug path
- syncing the repo's Windows guest debug scripts into `win11-main`
- collecting the expected local evidence files for issue or PR validation

## Scope and boundaries

This skill is a router, not the source of truth for the workflow itself.

The durable facts live in:

- repo doc: `/home/laysath/Projects/remote-control-for-windows/docs/debug-workflow.md`
- repo scripts: `/home/laysath/Projects/remote-control-for-windows/scripts/debug/windows/`

Do not restate those procedures from memory. Read the repo doc, then follow it.

Environment-specific facts that remain outside the repo and should be refreshed through other skills:

- `win11-main` VM and desktop handoff: `ops-libvirt-vm-platform`
- `ssh win11-main` alias and host facts: `ops-remote-host-ssh`

## Default workflow

1. Confirm the task is really about runtime debugging, not unit tests or CI-only work.
2. Read `/home/laysath/Projects/remote-control-for-windows/docs/debug-workflow.md`.
3. If the task touches `win11-main`, consult `ops-libvirt-vm-platform` and `ops-remote-host-ssh` first.
4. Sync the relevant repo script from `scripts/debug/windows/` into `/data/libvirt/work/share/`.
5. Pick the narrowest validated path:
   - console host for `windows` / `screenshot` / input / CLI validation
   - MCP for stdio tool debugging or minimal JSON-RPC confirmation
   - GUI host for Tauri window and WebView2/CDP work
6. Collect the evidence files named in `docs/debug-workflow.md`.
7. In the final report, cite exact file paths and distinguish what was truly run from what was only inspected.

## Path selection

### Console host

Choose this path when the issue is about:

- host connection lifecycle
- `rcwctl connect/status/exec/windows/screenshot`
- keyboard or mouse input
- proving the host is really in `SessionId=1`

Primary script:

- `/home/laysath/Projects/remote-control-for-windows/scripts/debug/windows/launch-rcw-host-interactive.ps1`

### MCP

Choose this path when the issue is about:

- `rcwctl mcp` startup
- tool schemas or tool naming
- minimal stdio JSON-RPC reproduction
- confirming the same desktop-state host also works through MCP

Use the repo doc's parameter names exactly. Do not guess them from memory.

### GUI

Choose this path when the issue is about:

- `rcw-host-gui.exe` startup
- Tauri window lifecycle
- WebView2 CDP exposure
- validating `/json/version`, `/json/list`, and page `Runtime.evaluate`

Primary script:

- `/home/laysath/Projects/remote-control-for-windows/scripts/debug/windows/launch-rcw-host-gui-interactive.ps1`

## Hard rules

- Do not use `mcp__rcw_zhang` as evidence for this repo's current code behavior.
- Do not treat `SessionId=0` as a valid desktop verification path.
- Do not keep reusing stale `host_id` or TOTP values after restarting a host process.
- Do not handwrite long remote PowerShell command strings when a repo script should be synced instead.
- For GUI debugging, remember console host and GUI host share the same Windows global single-instance lock; clear `rcw-host*` first.

## Evidence checklist

Before claiming success, ensure you have the right evidence for the selected path:

- console: `report`, `stdout`, `stderr`, `audit`, and usually a Linux-side screenshot file
- MCP: the exact request sequence plus the meaningful response fields or generated screenshot path
- GUI: `report`, `stdout`, `stderr`, `audit`, CDP HTTP responses, and one successful page WebSocket evaluation

If any of those are missing, report the gap explicitly instead of smoothing it over.
