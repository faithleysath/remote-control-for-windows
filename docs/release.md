# Release Process

This project does not yet have automated release publishing. Releases are
currently prepared manually from a clean checkout.

## Versioning

The workspace currently uses version `0.1.0`. Until a broader compatibility
policy is adopted:

- Patch releases should not change protocol semantics or CLI behavior in a
  breaking way.
- Breaking wire-format changes require a protocol version bump.
- Security fixes should be called out in `CHANGELOG.md`.

## Pre-Release Checklist

1. Confirm `CHANGELOG.md` has an entry for the release.
2. Confirm `README.md` and `docs/` describe the shipped behavior.
3. Run local checks:

   ```bash
   cargo fmt --check
   cargo test --workspace
   cargo clippy --workspace -- -D warnings
   ```

4. Run Windows host cross-checks:

   ```bash
   RUSTFLAGS='-C target-feature=+crt-static' \
     cargo xwin clippy -p rcw-host --target x86_64-pc-windows-msvc --release -- -D warnings
   RUSTFLAGS='-C target-feature=+crt-static' \
     cargo xwin build -p rcw-host --target x86_64-pc-windows-msvc --release
   ```

5. Run or refresh the Windows interactive desktop E2E smoke from
   [testing.md](testing.md).
6. Confirm no release artifact requires secrets or local configuration files.

## Build Commands

Linux controller and server:

```bash
cargo build --release -p rcwctl
cargo build --release -p rcw-server
```

Windows host from Linux:

```bash
rustup target add x86_64-pc-windows-msvc
RUSTFLAGS='-C target-feature=+crt-static' \
  cargo xwin build -p rcw-host --target x86_64-pc-windows-msvc --release
```

Expected host artifact:

```text
target/x86_64-pc-windows-msvc/release/rcw-host.exe
```

## Artifact Layout

Recommended release bundle:

```text
release/
  rcw-host-x86_64-pc-windows-msvc.zip
  rcwctl-x86_64-unknown-linux-gnu.tar.gz
  rcw-server-x86_64-unknown-linux-gnu.tar.gz
  checksums.txt
```

`checksums.txt` should use SHA-256 and include every published file.

## Windows Host Verification

Before publishing `rcw-host.exe`, verify:

- The file is a Windows x86-64 PE executable.
- The static CRT build does not require `VCRUNTIME140.dll` on a clean Windows
  environment.
- The host starts from an interactive desktop and connects to the relay.
- Screenshot and input operations are tested from an interactive desktop, not
  only from session 0 or a non-interactive service context.

## Post-Release

- Tag the release commit.
- Keep build commands, checksums, and E2E evidence with the release notes.
- Move newly found runtime gaps into `docs/testing.md` or `docs/roadmap.md`
  instead of leaving them only in chat history.
