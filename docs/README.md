# Documentation

This directory is the long-lived project documentation set. It separates stable
project contracts from operational procedures and historical validation notes.

## Start Here

- [Project scope](project-scope.md): product purpose, supported workflows,
  non-goals, and current maintenance baseline.
- [Architecture](architecture.md): crate responsibilities, runtime topology,
  state model, and failure handling.
- [Security model](security.md): authentication, privilege boundaries,
  visibility guarantees, audit redaction, and explicit non-goals.

## User And Operator References

- [CLI reference](cli.md): `rcwctl` commands, global flags, session behavior,
  and Codex-oriented usage rules.
- [Configuration](configuration.md): environment variables, embedded defaults,
  audit paths, and service deployment notes.
- [Testing](testing.md): local checks, Windows VM E2E plan, current validation
  evidence, and remaining proof gaps.
- [Release process](release.md): release checklist, artifacts, cross-build
  commands, and verification expectations.

## Maintainer References

- [Protocol](protocol.md): WebSocket endpoints, JSON messages, binary frame
  use, command types, errors, and compatibility rules.
- [Windows implementation notes](windows-apis.md): Win32 APIs and platform
  behavior used by `rcw-host.exe`.
- [Roadmap](roadmap.md): completed v1 baseline, maintenance priorities, and
  features that are intentionally out of scope.

## Removed Historical Docs

The early planning documents were folded into the documents above after v1
reached a mostly closed loop. Future changes should update the stable contracts
directly instead of adding new one-off planning docs.
