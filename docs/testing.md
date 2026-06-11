# Testing

This document is the maintainer-facing verification plan. It replaces the early
E2E planning document now that v1 has a working implementation and a Windows VM
validation baseline.

## Current Validation Record

On 2026-06-11, v1 was tested against the local `/data/windows-vm`
Windows-in-Docker VM. `rcw-host.exe` was cross-built on Linux and copied into
the Windows VM; `rcw-server` and `rcwctl` ran on the Linux host.

Verified:

- Host startup, server connection, machine ID/TOTP display, clipboard update,
  and elevated administrator privilege display.
- `rcwctl open/status/close`.
- Invalid control token, invalid TOTP, and mismatched TOTP period errors.
- Remote Windows command execution.
- Command timeout returning `RequestTimeout` with no residual tested `pwsh`
  process.
- Upload/download SHA-256 equality.
- Visible window enumeration.
- Interactive-desktop screenshot producing a 1280x720 PNG.
- Mouse move/click/scroll and keyboard type/key behavior, verified through
  Notepad screenshots.
- Clipboard content containing only server, machine ID, code, and period, with
  no control token, session token, TOTP seed, or raw machine identifier.
- `powercfg /requests` showing `rcw-host.exe` holding `DISPLAY` and `SYSTEM`
  requests, with session still active after temporarily reducing AC display and
  sleep timeouts.
- Old restored session file returning `SessionExpired` after `rcwctl close`.
- Server and host audit logs containing request-ID-linked events.

Remaining validation gap:

- Start `rcw-host.exe` inside a real standard-user interactive desktop and
  confirm the console displays `Privilege: standard user`. Automated attempts in
  the current VM did not produce a stable observable standard-user desktop host
  process; administrator interactive runs were validated.

Evidence from the 2026-06-11 run was preserved under:

- Linux side: `/tmp/rcw-v1-full.Qm94Dp`
- Windows shared logs: `/data/windows-vm/shared/rcw-v1-*`

## Required Local Checks

Run before code changes are submitted:

```bash
cargo fmt --check
cargo test --workspace
cargo clippy --workspace -- -D warnings
```

For Windows host code or manifest changes:

```bash
RUSTFLAGS='-C target-feature=+crt-static' \
  cargo xwin clippy -p rcw-host --target x86_64-pc-windows-msvc --release -- -D warnings
RUSTFLAGS='-C target-feature=+crt-static' \
  cargo xwin build -p rcw-host --target x86_64-pc-windows-msvc --release
```

## E2E Environment

Minimum real test topology:

- Linux/macOS host running `rcw-server` and `rcwctl`.
- Windows machine or VM running `rcw-host.exe` from an interactive desktop.
- Windows host can make outbound WebSocket connections to `rcw-server`.
- Test operator can read the host console for machine ID, TOTP, privilege state,
  clipboard status, and operation summaries.

Server example:

```bash
export RCW_BIND_ADDR=0.0.0.0:7800
export RCW_CONTROL_TOKEN=test-control-token
export RCW_AUDIT_LOG=$PWD/tmp/server-audit.jsonl
rcw-server
```

Host example:

```powershell
.\rcw-host.exe --server ws://<server-ip>:7800 --totp-period-seconds 120
```

Controller example:

```bash
export RCW_SERVER_URL=ws://<server-ip>:7800
export RCW_CONTROL_TOKEN=test-control-token
rcwctl open --id <machine-id> --totp <totp>
```

## E2E Checklist

Session and auth:

- Host can register without a token.
- Controller cannot open without the correct control token.
- Wrong TOTP fails.
- Mismatched TOTP period fails.
- `status` reports host online and session active after successful open.
- `close` invalidates the session and removes the local session file.

Command execution:

- `exec` returns stdout, stderr, exit code, and duration.
- A long command times out according to controller timeout settings.
- Timed-out child process trees are cleaned up.
- Host console and all audit logs contain the request ID.

File transfer:

- Upload default behavior does not overwrite existing files.
- Upload with `--overwrite` can replace a file.
- Downloaded file SHA-256 matches the source.
- Corrupt or mismatched transfer returns `checksum_mismatch` or an equivalent
  structured error.

GUI operations:

- `screenshot` returns a valid non-empty PNG from an interactive desktop.
- `windows` returns visible windows with handle, title, process ID, rect,
  visible, and focused fields.
- Mouse click lands consistently with screenshot coordinates.
- Text input and common keys work in Notepad or another simple focused app.
- Locked desktop, UAC secure desktop, or non-interactive session errors are
  explicit instead of reported as success.

Security and privacy:

- Clipboard text contains only support-safe connection information.
- Logs do not contain full control tokens, session tokens, TOTP seeds, raw
  machine identifiers, file contents, or default full command output.
- Elevated and standard-user host runs display the correct privilege state.
- The host never triggers UAC by itself.

Power behavior:

- Host process holds display/system power requests while running.
- Host exit releases the request.
- The test does not permanently change Windows power plans.

## Release Gate

Before a release, complete:

- Local checks.
- Windows host cross-build.
- At least one Windows interactive desktop E2E run.
- Audit redaction spot-check.
- SHA-256 checksums for all published artifacts.
