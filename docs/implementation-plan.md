# 实现计划

## 目标

本文把 v1 文档落成可执行的实现顺序。v1 只实现临时远控闭环，不实现 daemon、P2P、端口转发、一键诊断包、Web 管理后台或常驻服务。

## Workspace

初始结构：

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

`Cargo.toml` 使用 workspace 统一管理依赖版本。首批依赖建议：

- async/runtime：`tokio`
- HTTP/WebSocket：`axum`、`tokio-tungstenite` 或 `axum` WebSocket
- serialization：`serde`、`serde_json`
- CLI：`clap`
- IDs/time：`ulid`、`time`
- logging：`tracing`、`tracing-subscriber`
- paths：`directories`
- hashing：`sha2` 或 `blake3`
- Windows API：`windows`
- TOTP：`totp-rs` 或在 `rcw-common` 内实现一个小型 HOTP/TOTP helper

## rcw-common

实现共享模块：

- `protocol`：所有 wire message、command payload、error code、protocol version。
- `config`：server URL、TOTP period、control token、audit path、embed fallback。
- `audit`：三端 JSONL 审计事件结构和脱敏 helper。
- `ids`：request ID、session ID、session token 生成。
- `totp`：随机 seed、当前验证码、周期验证。
- `transfer`：文件分块 metadata、SHA-256 校验、chunk size 常量。
- `error`：统一错误类型和 CLI/server/host 显示映射。

约束：

- `control_token` 不进入 host config。
- `session token` 只出现在 session 文件和会话消息中。
- 审计事件不得记录完整 token、TOTP seed、文件内容或完整剪贴板文本。

## rcw-server

实现顺序：

1. `GET /healthz`。
2. `/ws/host` 接收 `host.hello`，无需 token 登记在线 host。
3. `/ws/control` 接收 `control.open`，校验 `RCW_CONTROL_TOKEN`。
4. 转发 `host.auth_request`，等待 host 本地 TOTP 验证结果。
5. 创建内存 session，返回 `session_id` 和 `session_token`。
6. 支持 `session.status`，只查服务端内存，不转发给 host。
7. 支持 `command.request` relay，按 request ID 转发 response。
8. 支持 `session.close`，吊销 session 并通知 host。
9. 写服务端审计 JSONL。
10. 加主机登记和认证失败的基础速率限制。

状态结构：

- `hosts: machine_id -> HostConn`
- `sessions: session_id -> SessionState`
- `pending: request_id -> response channel`
- `audit_writer`

session 不设置固定 TTL 和空闲超时。session 只在 host 断开、`session.close`、服务端重启或 session 状态无效时失效。

## rcw-host

实现顺序：

1. 解析配置：server URL、TOTP period、audit path。
2. 生成机器 ID 和本次运行 TOTP seed。
3. 检测管理员权限并在控制台高亮。
4. 连接 `/ws/host` 并发送不含 token 的 `host.hello`。
5. 打印 ID/TOTP/连接状态，启动 TOTP 倒计时刷新。
6. 启动和每次 TOTP 刷新时更新 Windows 剪贴板。
7. 设置进程运行期间的电源请求，阻止系统休眠和显示器熄屏。
8. 本地验证 `host.auth_request` 的 TOTP。
9. 串行执行远控命令。
10. 对每个命令写 host 审计日志并实时打印控制台摘要。
11. 支持关闭窗口即断开 WebSocket，释放电源请求，服务端使 session 失效。

命令执行：

- `exec`：使用 `tokio::process::Command` 或 Windows-specific process wrapper。
- 默认超时 30 秒，支持 payload 覆盖。
- 超时必须清理子进程树，不能只杀父进程。
- stdout/stderr 流式回传，完成后返回 exit code。

文件传输：

- `upload/download` 使用 JSON begin/complete + binary chunk。
- 每次传输必须计算 SHA-256。
- 上传默认不覆盖，除非 `overwrite=true`。
- 首版不做断点续传。

GUI 能力：

- `screenshot` 返回 PNG 数据。
- `windows` 枚举可见窗口和基础 rect/title/pid。
- `mouse.move/click/scroll` 使用屏幕绝对坐标。
- `keyboard.type/key` 映射到 Windows 输入事件。
- 锁屏、UAC 安全桌面、无交互桌面时返回明确错误，不假装成功。

电源请求：

- host 启动后调用 Windows 电源 API 阻止系统休眠和显示器熄屏。
- host 退出时释放电源请求。
- 不修改系统电源计划、注册表或服务。
- 电源请求失败不阻断上线，但必须控制台 warning 并写入 host audit。

## rcwctl

实现顺序：

1. `clap` 定义全局参数和子命令。
2. `open` 从 `--token` 或 `RCW_CONTROL_TOKEN` 读取控制端 token，成功后写 session 文件。
3. session 文件使用用户级应用数据目录，默认不绑定 cwd。
4. 后续每次调用重新读取 session 文件，连接 `/ws/control`，发送本次请求，完成后进程退出。
5. `status` 发送 `session.status`。
6. `close` 发送 `session.close` 并删除本地 session 文件。
7. 实现 `exec/upload/download/screenshot/windows/move/click/scroll/type/key`。
8. 所有命令支持 `--json` 机器可读输出。
9. 每次操作写控制端审计 JSONL。

session 文件不保存控制端 token，只保存：

- server URL
- machine ID
- session ID
- session token
- created_at
- last_used_at

## Binary Frame 格式

首版建议使用一个简单固定头，避免把大块二进制塞进 JSON：

```text
u8    kind        1=upload_chunk, 2=download_chunk, 3=screenshot_chunk
u128  request_id  与 JSON request_id 对应
u32   sequence
u32   total_sequences，未知时为 0
u32   payload_len
bytes payload
```

所有 begin/complete/error 仍使用 JSON。每个 transfer complete 必须带 `sha256`、`size`、`ok`。

## 审计事件

三端统一 JSONL。最小字段：

- `time`
- `side`
- `event`
- `machine_id`
- `session_id`
- `request_id`
- `command`
- `audit_label`
- `result`
- `duration_ms`
- `summary`

host 控制台实时显示 `time command result request_id summary`。日志写入失败不阻断命令，但必须 warning。

## 验证顺序

1. `cargo fmt --check`
2. `cargo clippy --all-targets -- -D warnings`
3. `cargo test --all-targets`
4. server + mock host integration tests
5. local server + Windows host E2E
6. release build smoke
