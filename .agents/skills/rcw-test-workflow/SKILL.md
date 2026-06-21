---
name: rcw-test-workflow
description: "Use when testing the remote-control-for-windows repo on this machine: choosing the right test layer, following the repo's testing workflow, and reusing the repo's pytest-based smoke and E2E scaffold instead of inventing ad hoc validation."
---

# rcw-test-workflow

Use this skill for real test work in the `rcw` repository when the task involves:

- deciding whether a change needs `unit / contract`, `integration / protocol`, `smoke e2e`, `business e2e`, or `release validation`
- running or extending the repo's current testing scaffold
- collecting reusable test evidence for an issue, PR, or release candidate

## Scope and boundaries

This skill is a router, not the source of truth for the workflow itself.

The durable facts live in:

- repo doc: `/home/laysath/Projects/remote-control-for-windows/docs/testing.md`
- repo doc: `/home/laysath/Projects/remote-control-for-windows/docs/debug-workflow.md`
- repo tests: `/home/laysath/Projects/remote-control-for-windows/tests/e2e/`
- repo debug scripts: `/home/laysath/Projects/remote-control-for-windows/scripts/debug/windows/`

Read those first. Do not maintain a second copy of the testing rules here.

Environment-specific facts that may need refresh through other skills:

- `win11-main` VM and desktop handoff: `ops-libvirt-vm-platform`
- `ssh win11-main` alias and host facts: `ops-remote-host-ssh`

## Default workflow

1. Confirm the task is really about testing workflow, test evidence, or adding coverage, not just one-off debugging.
2. Read `/home/laysath/Projects/remote-control-for-windows/docs/testing.md`.
3. If the test touches `win11-main`, consult `ops-libvirt-vm-platform` and `ops-remote-host-ssh` first.
4. Pick the narrowest layer that closes the risk:
   - `unit / contract` for pure logic and semantics
   - `integration / protocol` for cross-module or message-flow guarantees
   - `smoke e2e` for short real-environment liveness checks
   - `business e2e` for longer product workflows
   - `release validation` for final high-value candidate checks
5. Reuse the repo scaffold under `tests/e2e/` before inventing a new path.
6. Keep exact evidence paths and state clearly what was run versus what was only inspected.

## Current scaffold routing

### Console smoke

Choose this when the goal is to prove the desktop console host path still works:

- `/home/laysath/Projects/remote-control-for-windows/tests/e2e/test_console_smoke.py`

It covers the current minimum `connect/status/windows/screenshot/exec/disconnect` path.

### MCP smoke

Choose this when the goal is to prove `rcwctl mcp` still speaks stdio MCP correctly:

- `/home/laysath/Projects/remote-control-for-windows/tests/e2e/test_mcp_smoke.py`

It reuses the same interactive console host path and exercises `initialize`, `tools/list`, `connect`, `status`, `exec`, `windows`, `screenshot`, and `disconnect` through the official Python MCP SDK.

### GUI/CDP smoke

Choose this when the goal is to prove `rcw-host-gui.exe` still exposes a usable WebView2/CDP path:

- `/home/laysath/Projects/remote-control-for-windows/tests/e2e/test_gui_cdp_smoke.py`

It validates `/json/version`, `/json/list`, and one page `Runtime.evaluate`.

### GUI control smoke

Choose this when the goal is to prove the control side can really connect to the GUI host path, not just the CDP page:

- `/home/laysath/Projects/remote-control-for-windows/tests/e2e/test_gui_control_smoke.py`

It reuses the GUI/CDP path, reads runtime connection info from `window.__RCW_HOST_GUI_DEBUG__.getConnectionInfo()`, and exercises `connect`, `status`, `exec`, `windows`, `screenshot`, and `disconnect` against the GUI host.

## Hard rules

- Do not use `mcp__rcw_zhang` as evidence for this repo's current code behavior.
- Do not treat `SessionId=0` as valid evidence for Windows desktop behavior.
- Do not add a second guest-launch workflow when the repo already has a validated one.
- Do not report only “tests passed”; include the layer, pytest target, and evidence paths.
- Do not jump straight to VM-level scripts when a crate-level test can close the risk more cheaply.

## Evidence checklist

Before claiming success, ensure the report includes:

- the chosen test layer
- the exact script or command that ran
- the evidence directory or output paths
- any coverage gaps that remain

If a required layer was skipped, say so explicitly instead of smoothing it over.
