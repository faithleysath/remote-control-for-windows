# Tauri Host GUI

`rcw-host-gui` 是 Windows host 的 Tauri v2 图形界面工程。当前 MVP 提供概览、会话、审计和设置页面，让 host 用户可以在 GUI 内查看连接信息、复制 TOTP、控制监听状态、结束当前会话，并浏览当前运行期的事件时间线。

当前仍不包含独立 exec/transfer/tunnel 任务页、托盘、安装包或后台自启动。

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
npm ci
npm run tauri:dev
```

GUI 复用 `rcw-host-core` 的配置解析：

- `RCW_SERVER_URL` 或编译期 `RCW_EMBED_SERVER_URL` 提供中继服务器地址。
- `RCW_TOTP_PERIOD_SECONDS` 或编译期 `RCW_EMBED_TOTP_PERIOD_SECONDS` 配置 TOTP 周期。
- host 不读取 `RCW_CONTROL_TOKEN`。

如果没有保存过 GUI 设置，且环境变量/编译期配置也没有提供 server URL，GUI 默认使用 `ws://127.0.0.1:7800`，方便本地 smoke。正式分发时应通过设置页、环境变量或编译期配置写入实际 server。

如果本地没有启动 `rcw-server`，窗口仍会启动并显示 `reconnecting` 状态，用于验证 snapshot 和事件流。

## 页面范围

概览页显示：

- listener 状态和更新时间
- server URL
- machine id / host id
- 当前 TOTP 和倒计时
- 当前 session 摘要
- command / transfer / tunnel 计数
- audit log 路径
- Start / Stop / Reconnect / Copy 操作

会话页显示：

- controller label
- session id
- opened / last closed 时间
- close reason
- auth request 历史
- End Session 操作

审计页显示：

- 当前运行期 `HostEvent` 历史，默认保留最近 200 条。
- event category、kind、result、session id、request/task id、command、summary 和耗时。
- 时间正序/倒序切换。
- session id 过滤。
- request id / task id / command / summary 搜索。
- session / exec / transfer / tunnel / input / error / system 类型过滤。
- 每条事件的派生详情 JSON。
- 复制 audit path 或在系统文件管理器中显示 audit 文件位置。

审计页不直接读取历史超大 JSONL 文件；它展示的是 `HostSnapshot.events` 和实时 `host-event` 合并后的当前运行期历史。敏感字段仍由 host-core 的 event/audit 摘要与脱敏策略控制。

设置页显示和保存：

- server URL
- TOTP 周期
- audit log 路径
- 启动后自动监听

设置保存到 Tauri app config 目录下的 `host-gui.json`。保存时的生效规则是：

- listener 已停止：立即重建 stopped runtime，下一次 Start 使用新配置。
- listener 运行中：只保存文件并标记 `restart_required`；用户点击 Reconnect 或 Stop 后再 Start 才应用运行时配置，避免保存设置时自动断开当前会话。
- `auto_listen` 只影响下一次 GUI 启动，不要求重启 listener。

## Command 边界

当前通过 `tauri_build::AppManifest::commands(...)` 为下列应用 command 生成权限，并在 `capabilities/default.json` 中显式授权：

- `host_snapshot`：返回 `rcw-host-core::HostSnapshot`，供前端显示机器 ID、host ID、TOTP、listener 状态、session、任务计数和审计路径。
- `host_settings`：返回 GUI 设置和配置文件路径。
- `host_save_settings`：校验并保存 GUI 设置。
- `host_start_listener`：启动 listener；如果 listener 已停止，会先应用已保存的运行时配置。
- `host_stop_listener`：停止 listener；如果存在 pending runtime 设置，会在停止后应用。
- `host_restart_listener`：使用已保存的运行时配置重启 listener。
- `host_copy_connection_info`：通过 host-core/platform 复制连接信息，并返回同一份文本给前端兜底。
- `host_close_current_session`：请求结束当前 session。
- `host_reveal_audit_location`：用系统文件管理器显示当前 audit 文件位置，不开放通用 shell/fs 调用。

前端没有直接访问协议 socket、文件系统、shell 或底层 Windows API 的入口。后续新增 GUI 操作时，应先在 `rcw-host-core` 暴露明确的控制 API，再通过窄 Tauri command 调用。

## 事件通道

GUI 启动时订阅 `HostService::subscribe_events()`，并通过 Tauri core event `host-event` 推送到前端。前端使用 `@tauri-apps/api/event.listen("host-event", ...)` 接收事件，并在收到事件后刷新 snapshot。

`HostSnapshot` 同时携带最近 200 条运行期事件历史。前端刷新 snapshot 时以该历史为准，实时事件先追加到本地时间线，再由下一次 snapshot 刷新去重和校正顺序。

当 GUI 应用新的 runtime 配置时，`HostService` 会生成新的 host context 和事件源；GUI 会重新订阅事件转发。

## Tauri 权限基线

`src-tauri/capabilities/default.json` 只绑定 `main` 窗口，并只开放：

- `allow-host-snapshot`
- `allow-host-settings`
- `allow-host-save-settings`
- `allow-host-start-listener`
- `allow-host-stop-listener`
- `allow-host-restart-listener`
- `allow-host-copy-connection-info`
- `allow-host-close-current-session`
- `allow-host-reveal-audit-location`
- `core:event:default`

当前不启用 shell 插件，不启用 fs 插件，也不配置任意文件系统 scope。前端 bundle 的 CSP 使用 `default-src 'self'`，只允许本地资源和内联样式。

新增 capability 或插件前必须重新审查：

- 是否暴露了 shell/process 能力。
- 是否暴露了任意 fs 读写 scope。
- 是否绕过 `rcw-host-core` 的审计、脱敏和控制边界。
