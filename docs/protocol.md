# 协议设计

## 传输

当前协议使用 WebSocket 连接承载 JSON 控制消息。

端点：

- `GET /healthz`
- `GET /ws/host`
- `GET /ws/control`

控制消息使用 JSON text frame。文件内容和截图数据使用 WebSocket binary frame 分块传输；对应的 begin/complete/metadata 仍通过 JSON 消息承载。

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
    "protocol_version": 1,
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
    "protocol_version": 1,
    "control_token": "...",
    "machine_id": "8K4F-2M7Q",
    "totp": "183942",
    "totp_period_seconds": 120
  }
}
```

服务端先验证 `control_token`，再向主机转发认证请求。
`totp_period_seconds` 必须与主机登记值一致；不一致时返回 `invalid_totp_period`，避免控制端和被控端使用不同验证码周期造成误判。

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
- `upload.begin`、`upload.chunk`、`upload.complete`：上传文件；chunk 数据通过 binary frame 承载，JSON 消息只描述元数据和完成状态。
- `download.begin`、`download.chunk`、`download.complete`：下载文件；chunk 数据通过 binary frame 承载，JSON 消息只描述元数据和完成状态。
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
- `internal_error`

## 版本兼容

所有 hello/open 消息都带 `protocol_version`。当前实现接受协议版本 `1`。后续协议扩展必须保持既有字段含义不变；新行为应通过明确的 capability 字段协商，或有意识地提升协议版本。
