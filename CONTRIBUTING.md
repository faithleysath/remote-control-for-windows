# Contributing

This repository is maintained as a safety-sensitive remote-assistance tool.
Changes should keep the tool visible, temporary, auditable, and explicitly
authorized by the person running `rcw-host.exe`.

## Development Setup

Requirements:

- Rust stable toolchain.
- `cargo-xwin` for Linux-to-Windows MSVC host builds.
- A Windows machine or VM for GUI and privilege validation when touching
  `rcw-host` platform behavior.

Useful commands:

```bash
cargo fmt --check
cargo test --workspace
cargo clippy --workspace -- -D warnings
```

For Windows host changes:

```bash
rustup target add x86_64-pc-windows-msvc
RUSTFLAGS='-C target-feature=+crt-static' \
  cargo xwin clippy -p rcw-host --target x86_64-pc-windows-msvc --release -- -D warnings
RUSTFLAGS='-C target-feature=+crt-static' \
  cargo xwin build -p rcw-host --target x86_64-pc-windows-msvc --release
```

## Change Expectations

- Keep protocol changes backward-compatible unless the protocol version is
  intentionally bumped.
- Do not log full control tokens, session tokens, TOTP seeds, raw machine IDs,
  clipboard contents, or file contents.
- Preserve visible host behavior: console status, privilege display, operation
  summaries, and close-window termination.
- Do not add persistence, stealth, auto-start, driver installation, process
  injection, keylogging, or UAC bypass behavior.
- Update docs and tests when changing CLI flags, protocol payloads, security
  boundaries, or release commands.

## Testing Expectations

Small documentation-only changes can be validated with link/text inspection.
Code changes should run the workspace checks above. Windows-specific changes
need either a Windows VM/manual run or a clear note explaining the remaining
runtime gap.
