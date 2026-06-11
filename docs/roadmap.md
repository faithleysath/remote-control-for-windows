# Roadmap

The project has moved past the initial from-scratch v1 phase. The current
roadmap focuses on preserving the validated remote-control baseline while
improving release quality, safety checks, and operator experience.

## Completed Baseline

Completed and validated as of 2026-06-11:

- Rust workspace with `rcw-common`, `rcw-server`, `rcw-host`, and `rcwctl`.
- Host/control WebSocket relay through `rcw-server`.
- Control token plus machine ID/TOTP session creation.
- Reusable controller session files with explicit close and server-side session
  invalidation.
- Remote command execution with output, exit code, timeout, and process cleanup.
- Upload/download with chunking and SHA-256 verification.
- Screenshot, window enumeration, mouse input, and keyboard input.
- Host clipboard connection info with token/seed redaction.
- Temporary host-side display/system power requests.
- Host, controller, and server JSONL audit logs.
- Linux-to-Windows MSVC host cross-build with static CRT.
- Windows VM practical E2E coverage for the main workflow.

## Maintenance Priorities

- Close the remaining standard-user interactive desktop validation gap.
- Add repeatable Windows VM smoke automation around the existing manual E2E
  checklist.
- Improve release packaging and checksum generation.
- Add CI coverage for Linux workspace checks.
- Add protocol compatibility tests for future message changes.
- Harden audit redaction tests.
- Improve operator-facing error messages for common Windows desktop states such
  as lock screen, UAC secure desktop, and non-interactive session 0.

## Candidate Features

These are plausible future improvements, not committed release promises:

- One-command diagnostic bundle collection.
- Better display selection and coordinate reporting for multi-monitor hosts.
- Window-relative coordinate helpers.
- OCR or UI element discovery helpers that keep the current safety model.
- Controller operation transcripts for post-session review.
- Optional customer-side allow/deny policy for command categories.
- Temporary one-time download links for distributing `rcw-host.exe`.
- Port forwarding through the same relay model, with explicit audit and
  customer-visible status.
- Server-side persistent audit storage for deployments that need centralized
  retention.

## Explicit Non-Roadmap Items

The project should not add:

- Hidden remote control.
- Background persistence.
- Service installation or startup registration.
- Automatic UAC elevation or UAC bypass.
- Kernel drivers.
- Process injection.
- Keylogging.
- Background screen recording.
