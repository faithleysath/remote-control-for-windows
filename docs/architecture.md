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

Workspace 结构：

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

实现技术栈：

- `tokio` 作为异步运行时。
- `axum` 提供 HTTP/WebSocket。
- `serde` 和 `serde_json` 定义协议。
- `tracing` 输出日志。

### rcw-host

职责：

- Windows 控制台入口。
- 连接服务器并维持心跳。
- 生成和显示机器 ID/TOTP。
- 首次运行生成并持久化 host ID；运行时只允许同一物理机一个 `rcw-host.exe` 实例。
- 启动和 TOTP 刷新时把连接信息写入 Windows 剪贴板。
- 运行期间阻止系统休眠和显示器熄屏，退出时释放电源请求。
- 本地校验 TOTP。
- 检测当前进程是否以管理员权限运行，并在控制台高亮显示。
- 执行远控命令。
- 调用 Windows API 完成截图、鼠标、键盘和窗口枚举。
- 在控制台实时显示操作审计摘要，并写入被控端本地审计日志。

实现技术栈：

- `tokio` 处理网络和命令调度。
- `windows` crate 调用 Win32 API。
- `sha2` 处理机器 ID、token label 和文件 SHA-256。
- `hmac` + `sha1` 处理当前内置 TOTP/HOTP 逻辑。
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

实现技术栈：

- `clap` 定义子命令。
- `directories` 管理本地状态路径。
- `serde_json` 输出机器可读结果。

## 数据流

### 主机上线

1. 被控端在启动初期获取单实例锁；如果同一物理机已有 `rcw-host.exe` 运行，直接失败并提示。
2. 被控端解析服务器地址。
3. 被控端生成本次进程运行期 `host_id`，并生成展示用短 `machine_id` 和本次运行的 TOTP seed。
4. 被控端连接 `/ws/host`。
5. 被控端发送不含 token 的 `host.hello`，包含 `host_id`、短 `machine_id` 和协议版本。
6. 服务端按 `host_id` 登记在线主机，同时维护短 `machine_id -> host_id` 索引。
7. 被控端复制连接信息到剪贴板。
8. 被控端设置运行期间阻止系统休眠和显示器熄屏。
9. 被控端进入等待控制状态。

### 会话创建

1. 控制端连接 `/ws/control`。
2. 控制端发送 `control.open`，包含控制 token、展示短 machine ID 和 TOTP；短码冲突时可额外携带当前运行期 `host_id` 精确寻址。
3. 服务端校验控制 token。
4. 服务端默认用短 machine ID 查找在线主机；短码唯一时转发 `host.auth_request`，短码重复时返回明确错误。请求携带 `host_id` 时服务端按 `host_id` 精确查找，并校验它当前登记的短 machine ID 与请求一致。
5. 被控端本地验证 TOTP。
6. 服务端创建 session，并返回 session token。
7. 控制端保存 session token 到本地。

### 普通命令执行

1. 控制端发送 `command.request`。
2. 服务端根据 session 转发给被控端。
3. 被控端执行命令。
4. 被控端返回 `command.output` 和 `command.complete`。
5. 服务端转发结果给控制端。

### 后台 exec

1. 控制端发送 `command.start`，当前仅支持 `command=exec`。
2. 服务端验证 session 后创建 server-owned exec job，将消息改写为 `command.request` 并转发给 host。
3. 服务端立即返回 `command.start_result`，其中 `request_id` 同时也是后续查询和取消使用的 `task_id`。
4. host 后续返回的 `command.output`、`command.complete` 或 `error` 写入服务端 job 快照。
5. CLI 或 MCP 后续通过 `command.status` 查询，或通过 `command.cancel` 请求 host 取消。

### 会话状态查询

1. 控制端发送 `session.status`。
2. 服务端校验 session token。
3. 服务端根据内存中的会话表和在线主机表返回 session/host 状态。
4. 该请求不转发给被控端。

### TCP 隧道

1. 控制端发送 `tunnel.open`，声明 `local` 或 `remote` 方向、listen 地址和 target 地址。
2. 服务端验证 session token、loopback/allowlist 边界和 per-session 限额，创建 session 下的 tunnel registry 项。
3. `local` 方向由 controller 本地 listen，accept 后发送 `tunnel.stream_open`，host 连接 target；`remote` 方向由 host 本地 listen，accept 后发送 `tunnel.stream_open`，controller 连接 target。
4. 每条 TCP accept/connect 形成独立 `stream_id`。流量使用 `TunnelData` binary frame 按 `tunnel_id + stream_id` 路由；EOF/reset 用 JSON 控制消息表达。
5. `tunnel.close`、session close、host disconnect、MCP/CLI 退出或 idle timeout 都会回收 tunnel 和 stream 状态。

## 关键状态

服务端内存状态：

- 在线主机表：`host_id -> host_connection(machine_id, connection_id, tx)`。
- 展示短码索引：`machine_id -> host_id set`，只用于控制端输入短码后的查找和冲突检测。
- 会话表：`session_id -> host_id, machine_id, connection_id, controller, created_at, last_seen`。
- 待处理请求表：`request_id -> session_id, host_id, connection_id, response_channel`。
- server-owned exec job 表：`task_id -> host_id, connection_id, session_id, status/stdout/stderr/complete/error`。
- tunnel 表：`tunnel_id -> session_id, host_id, connection_id, direction, listen, target, status, counters, idle timeout`。
- tunnel stream 表：`stream_id -> tunnel_id, session_id, source_side, target_side, EOF/reset state`。
- 审计写入器：按结构化日志记录事件，不参与会话状态恢复。

被控端状态：

- 服务器连接状态。
- 本次进程运行期 host ID。
- 当前 TOTP seed。
- 当前 session。
- 正在执行的请求。
- active tunnel listener 和 stream pump。
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
- active tunnel listener、stream pump 和 MCP tunnel task 句柄。

## 并发模型

- 当前基线中，一个被控端同一时间只允许一个 active session。
- 控制端可以在重新验证当前 TOTP 后使用 force reconnect 替换同一 `host_id` 的旧 session。
- 同一 session 内允许多个命令并发发起和执行；服务端和被控端都按 `request_id` 路由结果。
- 长命令 `exec` 和 `download` 在 host 侧异步执行，发送输出或 binary chunk 时只短暂持有 WebSocket sink 锁，避免把同 session 的其他 request 串行化。
- upload/download 仍依赖控制端进程持续读写本地文件；MCP 的后台 transfer 只在当前 MCP 进程存活期间有效。
- TCP tunnel 由打开它的 CLI/MCP 进程持有本地 listener 和 stream pump；server 只保存短期 registry 并做 WebSocket 中继。
- 输入类命令当前不做全局串行锁；如果后续需要多人协作或复杂 GUI 操作，再单独设计输入 FIFO/lease。

## 失败处理

- 被控端断开：服务端只在断开的 `connection_id` 仍是当前连接时注销主机并使相关 session 失效；旧连接迟到断开不会误删重连后的新连接。
- 控制端断开：session 可以短时间保留，便于 CLI 多子命令复用；后台清理器会回收长期空闲 session、过期 pending open、过期请求路由和空的限流 key。
- exec 后台任务：`command.start` 创建 server-owned exec job；host 负责执行进程，server 保留有限 stdout/stderr、完成状态和错误，短 CLI 或 MCP 都可后续查询或取消。
- 文件传输任务：upload/download 是 client-attached stream；没有控制端 daemon 或 server staging 时，CLI 退出后无法继续读取本地源文件或写入本地目标文件。MCP 只能在本 MCP 进程存活期间后台传输。
- TCP tunnel：CLI `forward` 收到 Ctrl-C 时会发送 `tunnel.close` 并关闭本地 listener；MCP `tunnel_close` 和 MCP shutdown 会关闭对应 manager。异常退出时依赖 server session/tunnel idle cleanup 和 host/controller WebSocket 断开回收。
- MCP 正常退出：控制端尽力发送 `session.close`；崩溃或强杀时依赖 force reconnect 或服务端空闲清理兜底。
- 被控端 Ctrl-C 退出：host 尽力发送 WebSocket close，服务端观察到断开后清理 host 和相关 session。
- 服务端重启：所有主机和 session 失效，需要重新连接和认证。
- 命令超时：`exec.timeout_ms` 到期时，被控端尝试终止子进程树并返回 timeout。
- 命令取消：CLI/MCP `exec_cancel` 和已发到远端的 `transfer_cancel` 通过带 session token 的 `command.cancel` 转发到 host；server 成功验证 session、找到 request route 或 server-owned exec job 并投递到 host socket 后返回 `command.cancel_result`。exec 任务的最终 cancelled/failed 由 host 后续回包写入 server job；transfer 在确认远端取消已投递后由 MCP 进程 abort 本地任务、清理本地临时文件并标记 cancelled。还没发到远端的本地预处理阶段可以直接本地取消。host 收到后杀掉对应远端进程树并释放任务状态。
- 文件校验失败：控制端返回失败并提示重试。
- 审计写入失败：命令仍可继续，但三端都必须把本端审计失败作为 warning 暴露；被控端控制台要实时显示本地日志写入失败。

## 部署模型

推荐单进程部署：

```text
Internet / VPN
      |
  TLS reverse proxy
      |
  rcw-server
```

可以直接让 `rcw-server` 终止 TLS，也可以由 Nginx/Caddy 终止 TLS 后转发 WebSocket。生产推荐使用 443 端口和合法证书。
