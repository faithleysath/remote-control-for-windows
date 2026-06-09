# 产品设计

## 设计原则

- 临时协助优先：客户运行时才可控，关闭窗口即结束。
- CLI 优先：所有关键能力都必须能通过命令行完成。
- 客户可见：被控端始终显示连接和会话状态。
- 最小后台能力：不做托盘、服务、后台驻留和隐身。
- 诊断效率优先：认证后同一会话内不重复打断操作。

## 被控端界面

首版被控端是控制台程序，不做 GUI 框架。这样可以减少 Windows 运行依赖，也便于客户直接双击运行。

示例输出：

```text
Remote Control for Windows Host
Version: 0.1.0
Server: wss://remote.example.com
Privilege: standard user

Machine ID: 8K4F-2M7Q
Current TOTP: 183942
TOTP refreshes in: 103s
Clipboard: connection info copied
Power: sleep/display timeout suppressed while host is running

Connection: connected
Session: none

Keep this window open while support is active.
Close this window to stop remote control.
```

自动复制到剪贴板的文本建议为：

```text
远程协助连接信息
服务器：wss://remote.example.com
机器 ID：8K4F-2M7Q
验证码：183942
验证码有效期：120 秒
```

剪贴板内容只包含客户可主动转发给研发的信息，不包含控制端 token、session token、TOTP seed 或机器原始标识。每次 TOTP 刷新时，被控端应更新剪贴板并在控制台显示复制成功或失败。

会话建立后：

```text
Session: active
Controller: token:f3a8...
Last command: screenshot at 2026-06-09 15:42:17

Audit:
[15:42:17] screenshot ok output=screen.png request=01HY...
[15:42:31] exec started program=pwsh request=01HY...
[15:42:32] exec ok exit=0 request=01HY...
```

如果客户以管理员权限运行被控端，权限状态必须醒目显示，例如使用红色或黄色控制台颜色：

```text
Privilege: ADMINISTRATOR / elevated
```

## 控制端体验

控制端采用一次认证、多次命令的形态。

```bash
rcwctl open --id 8K4F-2M7Q --totp 183942
rcwctl status
rcwctl exec -- pwsh -NoProfile -Command "Get-Service"
rcwctl screenshot --output screen.png
rcwctl close
```

`open` 成功后，`rcwctl` 在本机保存一个会话文件。后续子命令默认读取该会话，不再要求重复传入 ID 和 TOTP。

## Codex 调用体验

Codex 需要稳定、可解析的输出。CLI 应支持：

- `--json` 输出结构化 JSON。
- 非 JSON 模式输出给人看的简短文本。
- 失败时返回非 0 退出码。
- 错误消息包含可操作原因，例如 `session expired`、`host disconnected`、`command timeout`。

示例：

```bash
rcwctl --json exec -- pwsh -NoProfile -Command "hostname"
```

```json
{
  "ok": true,
  "exit_code": 0,
  "stdout": "CLIENT-PC\r\n",
  "stderr": "",
  "duration_ms": 183
}
```

## 会话生命周期

1. 客户启动被控端。
2. 被控端连接服务器并登记机器 ID，同时把连接信息复制到剪贴板。
3. 客户把剪贴板内容通过 QQ 发给研发。
4. 研发执行 `rcwctl open`。
5. 服务端校验控制端 token，并把 TOTP 认证请求转发给被控端。
6. 被控端验证 TOTP，通过后进入 active session。
7. 控制端后续命令复用本地会话。
8. 任一方关闭会话、被控端退出、服务器重启或网络断开后，会话失效。

TOTP 默认 120 秒刷新一次，避免客户在聊天、电话或截图转发过程中刚拿到验证码就过期。打包或运行时可以调整周期，但控制端和被控端必须使用同一周期。

## 操作反馈

被控端必须显示以下状态：

- 服务器连接中、已连接、重连中、断开。
- 当前是否有控制会话。
- 当前是否已阻止系统休眠和显示器熄屏。
- 最近一次远控命令类型和时间。
- 终止控制的方法。

控制端必须显示以下状态：

- 会话 ID。
- 目标机器 ID。
- 被控端在线状态。
- 当前命令是否成功。
- 失败原因和建议下一步。
- 当前操作的 request ID，便于和三端审计日志对齐。

## 审计体验

所有远控操作都必须同时进入三类记录：

- 被控端控制台实时显示：给客户即时可见，不隐藏操作。
- 被控端、控制端本地日志：便于双方事后排查。
- 服务端中继日志：便于研发侧集中定位连接和协议问题。

被控端控制台显示的是操作摘要，不直接刷出大量命令输出或文件内容。命令输出仍由 `rcwctl exec` 返回，并由控制端日志记录摘要和退出状态。

## 首版交互取舍

- 不做实时视频流，只做按需截图。
- 不做复杂坐标识别，先提供原始截图和坐标点击。
- 不做被控端弹窗确认每个动作，因为目标是让 Codex 高效排障。
- 不做图形控制台，避免 UI 框架和打包复杂度。
- 不做自动 UAC 提权；管理员权限只能由客户手动右键运行获得。
