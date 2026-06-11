# Remote Control for Windows

Remote Control for Windows is a temporary, visible remote-assistance tool for
Windows support sessions. It is designed for engineers and automation agents
that need a command-line control surface for diagnostics, file transfer,
screenshots, and basic GUI input on an explicitly authorized customer machine.

The project is now in maintenance and iteration mode. The v1 remote-control
chain is implemented and has been validated against a real Windows VM; future
work should preserve the current safety model while hardening packaging,
automation, and operator ergonomics.

## Components

- `rcw-server`: WebSocket relay server for hosts and controllers.
- `rcw-host.exe`: visible Windows host process run by the customer or tester.
- `rcwctl`: controller CLI used by engineers, scripts, or Codex agents.
- `rcw-common`: shared protocol, IDs, TOTP, audit, config, and transfer code.

```text
rcwctl  <--WebSocket-->  rcw-server  <--WebSocket-->  rcw-host.exe
```

All connections are outbound from the host/controller side. The host registers
without a control token; a controller must have both the server control token and
the current host TOTP before a session can be opened.

## Current Status

Validated on 2026-06-11:

- Local Rust checks: `cargo fmt --check`, `cargo test --workspace`,
  `cargo clippy --workspace -- -D warnings`.
- Windows host cross-build from Linux with static CRT using `cargo-xwin`.
- Windows VM E2E: session open/status/close, invalid token/TOTP handling,
  command execution, command timeout cleanup, upload/download SHA-256 checks,
  window enumeration, screenshot, mouse movement/click/scroll, keyboard text and
  key input, clipboard safety, sleep/display suppression, expired session
  behavior, and server/host audit logs.

Known validation gap:

- A non-elevated standard-user interactive desktop run still needs final proof
  that `rcw-host.exe` displays `Privilege: standard user`. Elevated
  administrator desktop behavior has already been verified.

## Safety Model

This tool is intentionally not stealth software:

- The Windows host is a visible console process.
- Closing the host window terminates control.
- The host does not install a service, add startup entries, hide itself, or
  persist after exit.
- The host does not auto-elevate or bypass UAC.
- The host console shows the current privilege state and operation summaries.
- Clipboard connection info excludes control tokens, session tokens, TOTP seed,
  and raw machine identifiers.
- Operations are audit-logged on the host, controller, and server surfaces.

See [docs/security.md](docs/security.md) for the full security boundary.

## Quick Start

Run a local relay server:

```bash
export RCW_BIND_ADDR=127.0.0.1:7800
export RCW_CONTROL_TOKEN='replace-with-a-random-token'
cargo run -p rcw-server
```

Run the Windows host:

```powershell
.\rcw-host.exe --server ws://<server-host>:7800
```

Open a controller session and run commands:

```bash
export RCW_SERVER_URL=ws://127.0.0.1:7800
export RCW_CONTROL_TOKEN='replace-with-a-random-token'

cargo run -p rcwctl -- open --id <machine-id> --totp <current-totp>
cargo run -p rcwctl -- status
cargo run -p rcwctl -- exec -- pwsh -NoProfile -Command "hostname"
cargo run -p rcwctl -- screenshot --output screen.png
cargo run -p rcwctl -- close
```

Use `--json` for agent-friendly output:

```bash
cargo run -p rcwctl -- --json exec -- pwsh -NoProfile -Command "hostname"
```

## Build

Local Linux builds:

```bash
cargo build --workspace
cargo test --workspace
```

Cross-build the Windows host from Linux:

```bash
rustup target add x86_64-pc-windows-msvc
RUSTFLAGS='-C target-feature=+crt-static' \
  cargo xwin build -p rcw-host --target x86_64-pc-windows-msvc --release
```

Output:

```text
target/x86_64-pc-windows-msvc/release/rcw-host.exe
```

The static CRT build avoids requiring VC++ runtime installation on a clean
Windows machine.

## Development Checks

Run these before submitting changes:

```bash
cargo fmt --check
cargo test --workspace
cargo clippy --workspace -- -D warnings
```

For host-side Windows changes, also run:

```bash
RUSTFLAGS='-C target-feature=+crt-static' \
  cargo xwin clippy -p rcw-host --target x86_64-pc-windows-msvc --release -- -D warnings
RUSTFLAGS='-C target-feature=+crt-static' \
  cargo xwin build -p rcw-host --target x86_64-pc-windows-msvc --release
```

## Documentation

- [Documentation index](docs/README.md)
- [Project scope](docs/project-scope.md)
- [Architecture](docs/architecture.md)
- [Protocol](docs/protocol.md)
- [CLI reference](docs/cli.md)
- [Configuration](docs/configuration.md)
- [Testing](docs/testing.md)
- [Release process](docs/release.md)
- [Roadmap](docs/roadmap.md)
- [Security model](docs/security.md)
- [Windows implementation notes](docs/windows-apis.md)

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Security-sensitive changes must preserve
customer visibility, explicit authorization, token redaction, and auditability.

## License

This project is licensed under the MIT License. See [LICENSE](LICENSE).
