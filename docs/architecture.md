# 技术架构

## 总体结构

```text
rcwctl  <--WebSocket-->  rcw-server  <--WebSocket-->  rcw-host.exe
```

所有连接都由客户端主动发起：

- `rcw-host.exe` 主动连接服务器的 `/ws/host`。
- `rcwctl` 主动连接服务器的 `/ws/control`。
- `/ws/host` 不要求 token；`/ws/control` 在建立控制会话时必须校验控制端 token。
- 服务器只做控制端鉴权、状态管理和消息中继。

## Rust Workspace

Workspace structure:

```text
remote-control-for-windows/
  Cargo.toml
  crates/
    rcw-common/
    rcw-server/
    rcw-host/
    rcwctl/
  docs/
```

### rcw-common

共享以下能力：

- 协议消息类型。
- 配置解析。
- 错误类型。
- TOTP 工具。
- 机器 ID 哈希工具。
- 文件分块和哈希工具。

### rcw-server

职责：

- 暴露 HTTP 健康检查。
- 接收被控端 WebSocket；被控端连接和登记不要求 token。
- 接收控制端 WebSocket。
- 校验控制端 token。
- 管理内存中的主机和会话。
- 按 session relay 消息。
- 输出结构化日志。
- 写入服务端审计日志，记录认证、会话、命令中继和断开事件。

Implementation stack:

- `tokio` 作为异步运行时。
- `axum` 提供 HTTP/WebSocket。
- `serde` 和 `serde_json` 定义协议。
- `tracing` 输出日志。

### rcw-host

职责：

- Windows 控制台入口。
- 连接服务器并维持心跳。
- 生成和显示机器 ID/TOTP。
- 启动和 TOTP 刷新时把连接信息写入 Windows 剪贴板。
- 运行期间阻止系统休眠和显示器熄屏，退出时释放电源请求。
- 本地校验 TOTP。
- 检测当前进程是否以管理员权限运行，并在控制台高亮显示。
- 执行远控命令。
- 调用 Windows API 完成截图、鼠标、键盘和窗口枚举。
- 在控制台实时显示操作审计摘要，并写入被控端本地审计日志。

Implementation stack:

- `tokio` 处理网络和命令调度。
- `windows` crate 调用 Win32 API。
- `sha2` 或 `blake3` 处理哈希。
- `totp-rs` 或等价轻量实现处理 TOTP。
- `windows` crate 调用剪贴板 API；剪贴板失败只影响易用性，不阻断主机上线。
- `windows` crate 调用电源管理 API；电源请求失败只显示 warning，不阻断主机上线。

### rcwctl

职责：

- 解析 CLI 参数。
- 管理本地会话文件。
- 与服务器建立控制连接。
- 发起命令并输出结果。
- 支持 JSON 输出。
- 写入控制端本地审计日志。

Implementation stack:

- `clap` 定义子命令。
- `directories` 管理本地状态路径。
- `serde_json` 输出机器可读结果。

## 数据流

### 主机上线

1. 被控端解析服务器地址。
2. 被控端生成机器 ID 和本次运行的 TOTP seed。
3. 被控端连接 `/ws/host`。
4. 被控端发送不含 token 的 `host.hello`。
5. 服务端登记在线主机。
6. 被控端复制连接信息到剪贴板。
7. 被控端设置运行期间阻止系统休眠和显示器熄屏。
8. 被控端进入等待控制状态。

### 会话创建

1. 控制端连接 `/ws/control`。
2. 控制端发送 `control.open`，包含控制 token、机器 ID 和 TOTP。
3. 服务端校验控制 token。
4. 服务端找到在线主机，并转发 `host.auth_request`。
5. 被控端本地验证 TOTP。
6. 服务端创建 session，并返回 session token。
7. 控制端保存 session token 到本地。

### 命令执行

1. 控制端发送 `command.request`。
2. 服务端根据 session 转发给被控端。
3. 被控端执行命令。
4. 被控端返回 `command.output` 和 `command.complete`。
5. 服务端转发结果给控制端。

### 会话状态查询

1. 控制端发送 `session.status`。
2. 服务端校验 session token。
3. 服务端根据内存中的会话表和在线主机表返回 session/host 状态。
4. 该请求不转发给被控端。

## 关键状态

服务端内存状态：

- 在线主机表：`machine_id -> host_connection`。
- 会话表：`session_id -> machine_id, controller, created_at, last_seen`。
- 待处理请求表：`request_id -> response_channel`。
- 审计写入器：按结构化日志记录事件，不参与会话状态恢复。

被控端状态：

- 服务器连接状态。
- 当前 TOTP seed。
- 当前 session。
- 正在执行的请求。
- 当前权限状态：普通用户或管理员权限。
- 最近一次剪贴板复制状态。
- 电源请求状态：是否成功阻止系统休眠和显示器熄屏。
- 本地审计日志路径和最近操作摘要。

控制端本地状态：

- 服务器 URL。
- session ID。
- session token。
- machine ID。
- 创建时间和最近使用时间。
- 本地审计日志路径。

## 并发模型

- The current baseline allows one active session per host at a time.
- 同一 session 内命令默认串行执行，避免鼠标键盘和截图状态混乱。
- File transfer is treated as a long-running command and is not run in parallel
  with other commands in the current baseline.
- 后续可以扩展为命令执行并发、输入类命令串行。

## 失败处理

- 被控端断开：服务端注销主机并使相关 session 失效。
- 控制端断开：session 可以短时间保留，便于 CLI 多子命令复用。
- 服务端重启：所有主机和 session 失效，需要重新连接和认证。
- 命令超时：被控端尝试终止子进程并返回 timeout。
- 文件校验失败：控制端返回失败并提示重试。
- 审计写入失败：命令仍可继续，但三端都必须把本端审计失败作为 warning 暴露；被控端控制台要实时显示本地日志写入失败。

## 部署模型

Recommended single-process deployment:

```text
Internet / VPN
      |
  TLS reverse proxy
      |
  rcw-server
```

可以直接让 `rcw-server` 终止 TLS，也可以由 Nginx/Caddy 终止 TLS 后转发 WebSocket。生产推荐使用 443 端口和合法证书。
