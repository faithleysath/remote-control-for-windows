# AGENTS

## Project Identity

- This project is a Windows remote-control CLI and MCP toolchain.
- Short name: `rcw`.
- The repository is the source of truth for current development, debugging, and testing behavior.

## Local Machine Facts

- This machine is an Arch-family Linux server. The current host OS is `CachyOS` (`ID_LIKE=arch`).
- A local Windows VM named `win11-main` runs on this host.
- For remote login or operations related to `win11-main`, consult the `ops-libvirt-vm-platform` and `ops-remote-host-ssh` skills first.
- The intended SSH control path for the guest is `ssh win11-main`.
- The guest IP is fixed at `192.168.122.107` on the libvirt `default` network.
- From inside `win11-main`, host services on this machine are reachable through `192.168.122.1`.

## MCP Boundary

- In the current Codex session, the visible `mcp__rcw_zhang` server is based on a production `rcw` MCP server build.
- That MCP server is pinned to a production version. It does not represent this checkout, and it does not reliably represent the latest production version either.
- During development, debugging, or testing in this repository, do not use `mcp__rcw_zhang` as evidence for current code behavior.
