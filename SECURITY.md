# Security Policy

Remote Control for Windows is intended for explicit, temporary support sessions.
Security reports should avoid public disclosure until a maintainer has reviewed
the issue and a fix or mitigation is available.

## Supported Versions

The current `main` branch is the active development and maintenance line. No
separate stable release branches are maintained yet.

## Reporting a Vulnerability

Until a public security contact is configured, report vulnerabilities through
the maintainer's existing private project-support channel. Do not open a public
issue for vulnerabilities involving authentication bypass, token exposure,
stealth behavior, persistence, privilege escalation, command execution, file
transfer, or audit-log redaction.

Please include:

- Affected commit or release.
- Reproduction steps.
- Expected and actual behavior.
- Whether a control token, session token, TOTP seed, raw machine identifier, or
  customer data could be exposed.

## Security Boundaries

The project does not accept features that add stealth, persistence, automatic
UAC elevation, UAC bypass, keylogging, driver installation, process injection,
or hidden background control.

See [docs/security.md](docs/security.md) for the full product security model.
