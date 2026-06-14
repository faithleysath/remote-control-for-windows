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
    "protocol_version": 3,
    "host_version": "0.1.0",
    "machine_id": "8K4F-2M7Q",
    "totp_period_seconds": 120,
    "os": "windows",
    "hostname_hash": "..."
  }
}
```

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
    "protocol_version": 3,
    "control_token": "...",
    "machine_id": "8K4F-2M7Q",
    "totp": "183942",
    "totp_period_seconds": 120,
    "force_reconnect": false
  }
}
```

服务端先验证 `control_token`，再向主机转发认证请求。
`totp_period_seconds` 必须与主机登记值一致；不一致时返回 `invalid_totp_period`，避免控制端和被控端使用不同验证码周期造成误判。
`force_reconnect` 默认为 `false`。设置为 `true` 时，服务端仍会先让被控端验证当前 TOTP；只有验证通过后才会关闭同一 `machine_id` 的旧 session 并创建新 session。

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
    "machine_id": "8K4F-2M7Q"
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
    "machine_id": "8K4F-2M7Q",
    "host_online": true,
    "session_active": true
  }
}
```

服务端会刷新有效 session 的最近使用时间。空闲超过服务端保留时间的 session 会被后台清理，并向在线 host 发送 `host.session_closed`。

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
  "machine_id": "8K4F-2M7Q",
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
- `upload.begin`、`upload.chunk`、`upload.complete`：上传文件；chunk 数据通过 binary frame 流式承载，JSON 消息只描述元数据和完成状态。
- `download.begin`、`download.chunk`、`download.complete`：下载文件；chunk 数据通过 binary frame 流式承载，JSON 消息只描述元数据和完成状态。
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

所有 hello/open 消息都带 `protocol_version`。当前实现的协议版本是 `3`，server 对 `host.hello` 和 `control.open` 都要求版本精确匹配 `PROTOCOL_VERSION`；不匹配时返回 `error` / `internal_error`。后续协议扩展必须保持既有字段含义不变；新行为应通过明确的 capability 字段协商，或有意识地提升协议版本。
