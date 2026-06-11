# Changelog

All notable changes to this project are tracked here.

## Unreleased

- Reorganized project documentation for maintenance and iteration.
- Added standard open-source entry files: `LICENSE`, `CONTRIBUTING.md`,
  `SECURITY.md`, and this changelog.

## 0.1.0 - 2026-06-11

- Implemented v1 relay architecture with `rcw-server`, `rcw-host.exe`,
  `rcwctl`, and shared `rcw-common`.
- Added session creation with control token plus machine ID/TOTP verification.
- Added command execution, upload/download, screenshot, window enumeration,
  mouse input, keyboard input, session status, and session close flows.
- Added host/controller/server JSONL audit events keyed by request ID.
- Added visible host console UX with machine ID, TOTP, connection status,
  privilege state, clipboard status, power request status, and operation
  summaries.
- Added Windows clipboard connection-info updates and temporary sleep/display
  suppression while the host process is running.
- Added Linux-to-Windows MSVC cross-build support for `rcw-host.exe` with static
  CRT.
- Validated the v1 main chain in a Windows VM.

Known validation gap:

- Standard-user interactive desktop privilege display still needs final runtime
  proof. Elevated administrator desktop behavior has been verified.
