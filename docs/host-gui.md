# Tauri Host GUI

`rcw-host-gui` 是 Windows host 的 Tauri v2 图形界面工程骨架。当前阶段只提供最小窗口、只读 host snapshot 和 Rust 到前端的 `HostEvent` 推送通道；完整页面、托盘、安装包和任务操作入口放到后续 GUI issue。

## 目录

```text
crates/rcw-host-gui/
  package.json        # Vite frontend scripts
  src/                # Web UI
  src-tauri/          # Tauri v2 Rust crate, package name rcw-host-gui
```

Rust workspace 成员是 `crates/rcw-host-gui/src-tauri`。前端通过 `@tauri-apps/api` 调用 Tauri command 并监听事件。

## 开发启动

```bash
cd crates/rcw-host-gui
npm install
RCW_SERVER_URL=ws://127.0.0.1:7800 npm run tauri:dev
```

GUI 复用 `rcw-host-core` 的配置解析：

- `RCW_SERVER_URL` 或编译期 `RCW_EMBED_SERVER_URL` 提供中继服务器地址。
- `RCW_TOTP_PERIOD_SECONDS` 或编译期 `RCW_EMBED_TOTP_PERIOD_SECONDS` 配置 TOTP 周期。
- host 不读取 `RCW_CONTROL_TOKEN`。

如果本地没有启动 `rcw-server`，窗口仍会启动并显示 `reconnecting` 状态，用于验证 snapshot 和事件流。

## Command 边界

当前通过 `tauri_build::AppManifest::commands(&["host_snapshot"])` 只生成一个应用 command permission，并在 `capabilities/default.json` 中显式授权：

- `host_snapshot`：返回 `rcw-host-core::HostSnapshot`，供前端显示机器 ID、host ID、TOTP、listener 状态、session、任务计数和审计路径。

前端没有直接访问协议 socket、文件系统、shell 或底层 Windows API 的入口。后续新增 GUI 操作时，应先在 `rcw-host-core` 暴露明确的控制 API，再通过窄 Tauri command 调用。

## 事件通道

GUI 启动时订阅 `HostService::subscribe_events()`，并通过 Tauri core event `host-event` 推送到前端。前端使用 `@tauri-apps/api/event.listen("host-event", ...)` 接收事件，并在收到事件后刷新 snapshot。

## Tauri 权限基线

`src-tauri/capabilities/default.json` 只绑定 `main` 窗口，并只开放：

- `allow-host-snapshot`
- `core:event:default`

当前不启用 shell 插件，不启用 fs 插件，也不配置任意文件系统 scope。前端 bundle 的 CSP 使用 `default-src 'self'`，只允许本地资源和内联样式。

新增 capability 或插件前必须重新审查：

- 是否暴露了 shell/process 能力。
- 是否暴露了任意 fs 读写 scope。
- 是否绕过 `rcw-host-core` 的审计、脱敏和控制边界。
