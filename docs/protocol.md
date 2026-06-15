# 协议设计

## 传输

当前协议使用 WebSocket 连接承载 JSON 控制消息。

端点：

- `GET /healthz`
- `GET /ws/host`
- `GET /ws/control`

控制消息使用 JSON text frame。文件内容和截图数据使用 WebSocket binary frame 分块传输；对应的 begin/complete/metadata 仍通过 JSON 消息承载。文件上传和下载按块流式读写本地文件，不把文件主体放入 JSON 或 base64 参数。

## 消息通用字段

所有 JSON 消息使用以下基础字段：

```json
{
  "type": "command.request",
  "request_id": "01HY...",
  "session_id": "01HY...",
  "payload": {}
}
```

字段说明：

- `type`：消息类型。
- `request_id`：请求级 ID，用于匹配响应。
- `session_id`：会话 ID，会话建立前可为空。
- `payload`：类型相关内容。

## 主机上线

被控端连接 `/ws/host` 不需要控制端 token。`host.hello` 只用于登记在线主机和能力信息，不授权任何控制操作。

### host.hello

```json
{
  "type": "host.hello",
  "payload": {
    "protocol_version": 6,
    "host_version": "0.1.0",
    "host_id": "host_QbYx...",
    "machine_id": "8A4F-2B7C-91D0",
    "totp_period_seconds": 120,
    "os": "windows",
    "hostname_hash": "..."
  }
}
```

`host_id` 是被控端进程启动时生成的运行期随机长 ID，服务端内部 host/session/request/job 路由都以它为主键。同一进程内 WebSocket 断线重连会复用同一个 `host_id`；进程重启后会生成新的 `host_id`。被控端启动输出和剪贴板连接信息都会包含当前运行期 `host_id`。

`machine_id` 是给人工输入和展示用的短码，当前格式为 `XXXX-XXXX-XXXX`，由稳定机器材料哈希得到；它不再作为服务端内部唯一主键。控制端默认输入短 `machine_id`，服务端用短码查找在线 host；如果同一短码对应多个在线 `host_id`，服务端返回明确错误，不静默覆盖。控制端可以额外传 `host_id` 精确寻址来消除短码冲突；`host_id` 只用于寻址，不是认证秘密。

服务端响应：

```json
{
  "type": "host.hello_ack",
  "payload": {
    "server_time": "2026-06-09T07:30:00Z",
    "heartbeat_interval_ms": 15000
  }
}
```

## 控制端认证

控制端 token 只出现在 `/ws/control` 的会话创建流程中。没有有效控制端 token 的客户端不能发起会话，也不能向被控端发送命令。

### control.open

```json
{
  "type": "control.open",
  "request_id": "01HY...",
  "payload": {
    "protocol_version": 6,
    "control_token": "...",
    "machine_id": "8A4F-2B7C-91D0",
    "host_id": "host_QbYx...",
    "totp": "183942",
    "totp_period_seconds": 120,
    "force_reconnect": false
  }
}
```

服务端先验证 `control_token`，再向主机转发认证请求。
`host_id` 可省略；省略时服务端按短 `machine_id` 查找，短码冲突会返回 `host_busy`。传入 `host_id` 时服务端按它精确查找在线 host，并校验它当前登记的短 `machine_id` 与请求中的 `machine_id` 一致。
`totp_period_seconds` 必须与主机登记值一致；不一致时返回 `invalid_totp_period`，避免控制端和被控端使用不同验证码周期造成误判。
`force_reconnect` 默认为 `false`。设置为 `true` 时，服务端仍会先让被控端验证当前 TOTP；只有验证通过后才会关闭同一 `host_id` 的旧 active session 并创建新 session。

### host.auth_request

```json
{
  "type": "host.auth_request",
  "request_id": "01HY...",
  "payload": {
    "totp": "183942",
    "controller_label": "token:f3a8..."
  }
}
```

### control.open_result

```json
{
  "type": "control.open_result",
  "request_id": "01HY...",
  "payload": {
    "ok": true,
    "session_id": "01HY...",
    "session_token": "...",
    "host_id": "host_QbYx...",
    "machine_id": "8A4F-2B7C-91D0"
  }
}
```

失败时：

```json
{
  "type": "error",
  "request_id": "01HY...",
  "payload": {
    "code": "invalid_totp",
    "message": "TOTP is invalid or expired"
  }
}
```

## 会话命令

### session.status

查询当前会话和主机在线状态。该消息由 `rcwctl status` 使用，不转发到被控端；服务端根据内存中的 host/session 状态直接返回。

```json
{
  "type": "session.status",
  "request_id": "01HY...",
  "session_id": "01HY...",
  "payload": {
    "session_token": "..."
  }
}
```

响应：

```json
{
  "type": "session.status_result",
  "request_id": "01HY...",
  "payload": {
    "ok": true,
    "machine_id": "8A4F-2B7C-91D0",
    "host_online": true,
    "session_active": true
  }
}
```

服务端会刷新有效 session 的最近使用时间。空闲超过服务端保留时间的 session 会被后台清理，并向在线 host 发送 `host.session_closed`。

### session.close / host.session_close

`session.close` 由控制端请求关闭当前 session，服务端验证 `session_token` 后清理 session、request route、exec job 和 tunnel registry，并向 host 发送 `host.session_closed`。

`host.session_close` 由 host 主动请求关闭当前 session，供 GUI host 的“结束当前会话”使用。服务端要求该 session 属于当前 host websocket 的 `host_id` 和 `connection_id`；验证通过后清理 server 侧 session 状态，向 host 发送 `host.session_closed` 和 `host.session_close_result`，并向 controller 发送 `host.session_closed` / `error(cancelled)` 让等待中的控制端尽快退出。

```json
{
  "type": "host.session_close",
  "request_id": "01HY...",
  "session_id": "01HY...",
  "payload": {
    "session_id": "01HY...",
    "reason": "host_close"
  }
}
```

响应：

```json
{
  "type": "host.session_close_result",
  "request_id": "01HY...",
  "session_id": "01HY...",
  "payload": {
    "ok": true,
    "session_id": "01HY..."
  }
}
```

### command.request

```json
{
  "type": "command.request",
  "request_id": "01HY...",
  "session_id": "01HY...",
  "payload": {
    "session_token": "...",
    "command": "exec",
    "audit_label": "collect computer info",
    "args": {
      "program": "pwsh",
      "argv": ["-NoProfile", "-Command", "hostname"],
      "cwd": null,
      "timeout_ms": 30000
    }
  }
}
```

`audit_label` 可选，供控制端或 Codex 为操作提供简短意图说明。被控端控制台和三端日志都应记录该字段，但不得依赖它做权限判断。

`exec.args.timeout_ms` 可选，表示远端进程运行上限；当前 rcwctl CLI 和 MCP 默认都会填入 24 小时。控制端/MCP 的等待响应 timeout 是独立语义，不应写入 `exec.args.timeout_ms`。

同一 active session 内允许多个 `command.request` 并发执行，服务端按 `request_id` 路由文本和 binary 回包。每个 request route 绑定 `session_id`、`host_id` 和当前 host `connection_id`；host 重连后，旧连接的迟到回包不会被转发到新连接的 session。

### command.start

用于创建 server-owned exec job。当前只支持 `command=exec`，不用于 upload/download，因为文件传输需要控制端进程持续读写本地文件。

请求体与 `command.request` 使用同一个 payload；`request_id` 同时也是后续查询和取消使用的 `task_id`：

```json
{
  "type": "command.start",
  "request_id": "01HY...",
  "session_id": "01HY...",
  "payload": {
    "session_token": "...",
    "command": "exec",
    "args": {
      "program": "pwsh",
      "argv": ["-NoProfile", "-Command", "Start-Sleep 30; 'done'"]
    }
  }
}
```

服务端验证 session 后创建内存 job，把消息转成 `command.request` 转发给 host，并返回初始状态：

```json
{
  "type": "command.start_result",
  "request_id": "01HY...",
  "session_id": "01HY...",
  "payload": {
    "task_id": "01HY...",
    "status": "running",
    "request_id": "01HY...",
    "started_at": "2026-06-14T...",
    "stdout": "",
    "stderr": "",
    "stdout_truncated": false,
    "stderr_truncated": false
  }
}
```

server 会记录有限 stdout/stderr、完成 payload 或错误；job 有服务端 TTL，只用于短期恢复和查询，不是永久审计存储。
当前服务端保留 stdout/stderr 各最多 1 MiB，完成后的 job 快照保留约 30 分钟；仍在 running 且超过 request route TTL 的 job 会变为 `failed` / `request_timeout`。

## TCP 隧道

隧道用于在同一 active session 内承载 TCP port forwarding。第一版只支持 TCP，不支持 UDP、P2P、server 裸 TCP 入口、持久化 tunnel 或跨 session 复用。

### tunnel.open

控制端发送：

```json
{
  "type": "tunnel.open",
  "request_id": "01HY...",
  "session_id": "01HY...",
  "payload": {
    "session_token": "...",
    "direction": "local",
    "listen_addr": "127.0.0.1",
    "listen_port": 15432,
    "target_host": "127.0.0.1",
    "target_port": 5432,
    "idle_timeout_ms": 600000,
    "allow_non_loopback_listen": false,
    "allow_non_loopback_target": false
  }
}
```

`direction=local` 表示 controller 本地 listen，host 侧连接 target；`direction=remote` 表示 host 侧 listen，controller 侧连接 target。server 验证 session token、loopback/allowlist 边界和 per-session 限额后分配 `tunnel_id`，再把 `tunnel.open` 转发给 host。

返回：

```json
{
  "type": "tunnel.open_result",
  "request_id": "01HY...",
  "session_id": "01HY...",
  "payload": {
    "ok": true,
    "tunnel": {
      "tunnel_id": "01HY...",
      "session_id": "01HY...",
      "direction": "local",
      "listen_addr": "127.0.0.1",
      "listen_port": 15432,
      "target_host": "127.0.0.1",
      "target_port": 5432,
      "status": "active",
      "opened_at": "2026-06-15T...",
      "last_activity_at": "2026-06-15T...",
      "idle_timeout_ms": 600000,
      "bytes_from_listener": 0,
      "bytes_from_target": 0,
      "active_streams": 0,
      "total_streams": 0
    }
  }
}
```

### tunnel.status / tunnel.close

`tunnel.status` 返回当前 session 下 tunnel 列表，可选 `tunnel_id` 过滤。`tunnel.close` 按 `tunnel_id` 关闭 tunnel，并清理 server registry、host/controller listener 和 stream pump。

### tunnel.stream_open / EOF / reset

每次 TCP accept 都创建一个 `stream_id`，源端发送 `tunnel.stream_open`，目标端连接 target 后返回 `tunnel.stream_open_result`。之后流量使用 WebSocket binary frame 的 `TunnelData` 格式承载，不复用 upload/download 的 file chunk header。半关闭使用 `tunnel.stream_eof`；异常断开使用 `tunnel.stream_reset`。

`TunnelData` binary frame header：

- `kind: u8 = 4`
- `tunnel_id: 16 bytes ULID`
- `stream_id: 16 bytes ULID`
- `payload_len: u32`
- `payload`

server 只按 `tunnel_id + stream_id` 做 WebSocket 中继和状态计数，不暴露裸 TCP 入口，不记录 payload 内容。

### command.status

查询 server-owned exec job：

```json
{
  "type": "command.status",
  "request_id": "01HY...",
  "session_id": "01HY...",
  "payload": {
    "session_token": "...",
    "task_id": "01HY..."
  }
}
```

返回：

```json
{
  "type": "command.status_result",
  "request_id": "01HY...",
  "session_id": "01HY...",
  "payload": {
    "task_id": "01HY...",
    "status": "completed",
    "request_id": "01HY...",
    "started_at": "2026-06-14T...",
    "finished_at": "2026-06-14T...",
    "stdout": "done\n",
    "stderr": "",
    "stdout_truncated": false,
    "stderr_truncated": false,
    "complete": {
      "ok": true,
      "exit_code": 0,
      "duration_ms": 30000
    }
  }
}
```

`status` 可为 `running`、`completed`、`failed` 或 `cancelled`。失败时返回 `error` payload。

### command.cancel

用于取消正在运行的请求。当前用于 server-owned exec job，以及已经发到远端的 MCP 进程内文件传输取消：

```json
{
  "type": "command.cancel",
  "request_id": "01HY...",
  "session_id": "01HY...",
  "payload": {
    "session_token": "..."
  }
}
```

服务端先用 `session_token` 验证当前 session，再按原 `request_id` 的 request route 或 server-owned exec job 转发给 host。host 收到后会杀掉对应 exec 进程树、停止 download 分块发送，或丢弃对应 upload 临时状态。server 未确认取消或返回 `error` 时，exec 控制端不得把任务伪装成 `cancelled`。MCP transfer 任务由 MCP 进程本地持有；如果远端阶段已经开始，当前实现会先要求 server 成功返回 `command.cancel_result`，再 abort 本地任务并标记为 `cancelled`。尚未发到远端的本地预处理阶段可以由控制端直接本地取消。

服务端成功验证 session、找到 request route 或 server-owned exec job，并把取消消息投递到当前 host socket 后返回：

```json
{
  "type": "command.cancel_result",
  "request_id": "01HY...",
  "session_id": "01HY...",
  "payload": {
    "ok": true
  }
}
```

如果 route 失效、token 过期、host 已断开或转发失败，服务端返回 `error`。

### command.output

用于流式输出：

```json
{
  "type": "command.output",
  "request_id": "01HY...",
  "payload": {
    "stream": "stdout",
    "data": "CLIENT-PC\r\n"
  }
}
```

### command.complete

```json
{
  "type": "command.complete",
  "request_id": "01HY...",
  "payload": {
    "ok": true,
    "exit_code": 0,
    "duration_ms": 183
  }
}
```

## 审计事件

三端都应以结构化格式记录审计事件。服务端和控制端写入本地日志；被控端除写入本地日志外，还必须实时显示到控制台。

通用字段：

```json
{
  "event": "command.complete",
  "time": "2026-06-09T07:42:32Z",
  "host_id": "host_QbYx...",
  "machine_id": "8A4F-2B7C-91D0",
  "session_id": "01HY...",
  "request_id": "01HY...",
  "side": "host",
  "command": "exec",
  "audit_label": "collect computer info",
  "result": "ok"
}
```

三端日志语义：

- host：记录实际执行或拒绝的操作、权限状态、目标路径或窗口摘要。
- controller：记录控制端发起的操作、参数摘要、返回结果和本地输出文件路径。
- server：记录认证、会话创建关闭、消息中继和连接断开，不记录命令输出或文件内容。

## 命令类型

当前命令类型：

- `session.status`：查询会话和主机在线状态。
- `session.close`：关闭会话。
- `exec`：执行终端命令。
- `upload.begin`、`upload.complete`：上传文件；binary frame 里承载 `UploadChunk`，JSON 消息只描述元数据和完成状态。
- `download.begin`、`download.complete`：下载文件；binary frame 里承载 `DownloadChunk`，JSON 消息只描述元数据和完成状态。
- `tunnel.open`、`tunnel.status`、`tunnel.close`：管理 TCP tunnel。
- `tunnel.stream_open`、`tunnel.stream_eof`、`tunnel.stream_reset`：管理 tunnel 内单条 TCP stream。
- `screenshot`：截屏。
- `windows`：列出窗口。
- `mouse.move`：移动光标。
- `mouse.click`：点击。
- `mouse.scroll`：滚动。
- `keyboard.type`：输入文本。
- `keyboard.key`：按键或组合键。

## 错误码

当前错误码：

- `invalid_token`
- `host_not_found`
- `host_busy`
- `invalid_totp`
- `invalid_totp_period`
- `session_expired`
- `host_disconnected`
- `request_timeout`
- `command_failed`
- `unsupported_command`
- `invalid_path`
- `checksum_mismatch`
- `permission_denied`
- `cancelled`
- `internal_error`

## 版本兼容

所有 hello/open 消息都带 `protocol_version`。当前实现的协议版本是 `6`，server 对 `host.hello` 和 `control.open` 都要求版本精确匹配 `PROTOCOL_VERSION`；不匹配时返回 `error` / `internal_error`。当前不做协议范围兼容协商；后续协议扩展必须保持既有字段含义不变，或有意识地提升协议版本。
