# E2E 测试计划

## 目标

证明 v1 的真实远控闭环可用：Windows 被控端连接本机服务端，Linux/macOS 控制端 CLI 能通过服务器完成认证、命令、文件、截图、窗口、鼠标键盘和审计验证。

## 最近实机验证记录

2026-06-11 在本机 `/data/windows-vm` 的 Windows-in-Docker VM 上完成 v1 实机验证。Windows 被控端使用本机 Linux 交叉编译出的 `rcw-host.exe`，控制端和服务端运行在 Linux 主机。

已通过：

- Windows host 启动、连接 server、显示 machine ID/TOTP、复制剪贴板、显示管理员 elevated 状态。
- `rcwctl open/status/close` 正向会话。
- 错误 control token、错误 TOTP、TOTP 周期不一致均返回预期错误。
- Windows 命令执行成功。
- 远端命令超时返回 `RequestTimeout`，并确认被测 `pwsh` 进程无残留。
- 上传/下载文件 SHA-256 一致。
- `windows` 返回可见窗口列表。
- 交互桌面下 `screenshot` 生成 1280x720 PNG。
- `move`、`click`、`scroll`、`type`、`key` 均执行成功，并用 Notepad 截图确认文字实际输入。
- 剪贴板内容只包含服务器、机器 ID、验证码、有效期；未包含 control token、session token、TOTP seed 或原始机器标识。
- `powercfg /requests` 显示 `rcw-host.exe` 持有 `DISPLAY` 和 `SYSTEM` 请求；临时把 AC 显示/睡眠超时设为 60 秒并等待 75 秒后，session 仍 active，命令仍可执行；测试后已恢复原值。
- `rcwctl close` 后，把旧 session 文件放回再请求 `status`，返回 `SessionExpired`。
- server 和 host 审计日志均写入 request ID 相关事件。

未闭合：

- 普通标准用户桌面启动 `rcw-host.exe` 后显示 `Privilege: standard user` 尚未完成实机确认。当前 VM 中管理员交互任务即使不显式请求 highest 仍显示 elevated；临时标准用户的非交互自动化启动未能稳定产生可观测 host 日志。该项需要登录标准用户桌面后手工或通过可交互通道启动 host 再验证。

证据位置：

- Linux 侧测试产物：`/tmp/rcw-v1-full.Qm94Dp`
- Windows 共享目录日志：`/data/windows-vm/shared/rcw-v1-*`

## 测试环境

真实 E2E 环境：

- 本机运行 `rcw-server` 和 `rcwctl`。
- Windows 主机运行 `rcw-host.exe`。
- Windows 主机安装并启用 OpenSSH，便于从本机复制二进制、启动 host、查看日志。
- 本机和 Windows 主机网络可达。
- Windows 主机能主动访问本机 `rcw-server` 地址。

本机服务端示例：

```bash
export RCW_BIND_ADDR=0.0.0.0:7800
export RCW_CONTROL_TOKEN=test-control-token
export RCW_AUDIT_LOG=$PWD/tmp/server-audit.jsonl
rcw-server
```

Windows host 示例：

```powershell
$env:RCW_SERVER_URL = "ws://<local-ip>:7800"
$env:RCW_TOTP_PERIOD_SECONDS = "120"
.\rcw-host.exe
```

控制端示例：

```bash
export RCW_SERVER_URL=ws://<local-ip>:7800
export RCW_CONTROL_TOKEN=test-control-token
rcwctl open --id <machine-id> --totp <totp>
```

## 本地集成测试

在没有真实 Windows 主机前，先用 mock host 验证协议：

- server `/healthz` 返回成功。
- mock host 连接 `/ws/host`，发送 `host.hello`。
- `rcwctl open` 用正确 token/TOTP 建会话。
- 错误 token 返回 `invalid_token`。
- 错误 TOTP 返回 `invalid_totp`。
- TOTP 周期不一致返回 `invalid_totp_period`。
- `rcwctl status` 返回 host online/session active。
- `rcwctl close` 后 session 失效。
- server 重启后旧 session 失效。

## Windows Host Smoke

步骤：

1. 通过 OpenSSH 把 `rcw-host.exe` 复制到 Windows 主机。
2. 启动 `rcw-server`。
3. 在 Windows 主机运行 `rcw-host.exe`。
4. 确认控制台显示 server、machine ID、TOTP、TOTP 剩余时间、clipboard 状态、privilege。
5. 确认控制台显示已请求阻止系统休眠和显示器熄屏，或显示明确 warning。
6. 确认剪贴板内容包含服务器、机器 ID、验证码、有效期。
7. 确认剪贴板内容不包含控制端 token、session token、TOTP seed、原始机器标识。
8. 在本机执行 `rcwctl open`。
9. 执行 `rcwctl status` 确认 session active。

验收：

- host 不需要 token 即可上线。
- control 没有 token 不能 open。
- open 成功后控制端 session 文件不包含控制端 token。

## 电源行为 E2E

步骤：

1. 在 Windows 主机上把显示器关闭和系统睡眠超时临时调短，例如 1 分钟。
2. 启动 `rcw-host.exe`。
3. 等待超过原本显示器关闭和睡眠超时时间。
4. 确认 Windows 主机没有自动睡眠，显示器没有因空闲策略熄屏。
5. 关闭 `rcw-host.exe`。
6. 确认被控端没有永久修改系统电源计划。

验收：

- host 运行期间临时阻止系统休眠和显示器熄屏。
- host 控制台显示电源请求状态。
- host 退出后释放电源请求。
- 该能力不要求管理员权限，不触发 UAC，不安装服务，不修改注册表。

## 命令执行 E2E

命令：

```bash
rcwctl --json exec -- pwsh -NoProfile -Command "hostname"
rcwctl --json exec -- pwsh -NoProfile -Command "$PSVersionTable.PSVersion.ToString()"
rcwctl --json exec --timeout 5s -- pwsh -NoProfile -Command "Start-Sleep -Seconds 30"
```

验收：

- stdout/stderr/exit_code 正确。
- 超时命令返回 `request_timeout` 或等价错误。
- 超时后 Windows 侧没有残留被测子进程树。
- host 控制台实时显示 exec started/ok/timeout。
- 三端审计日志能用 request ID 对齐。

## 文件传输 E2E

上传：

```bash
sha256sum ./fixtures/tool.txt
rcwctl upload ./fixtures/tool.txt 'C:\Users\Public\rcw-tool.txt'
rcwctl exec -- pwsh -NoProfile -Command "Get-FileHash C:\Users\Public\rcw-tool.txt -Algorithm SHA256"
```

下载：

```bash
rcwctl download 'C:\Users\Public\rcw-tool.txt' ./tmp/downloaded-tool.txt
sha256sum ./tmp/downloaded-tool.txt
```

验收：

- 上传默认不覆盖已有文件。
- `--overwrite` 可以覆盖。
- 上传和下载 SHA-256 一致。
- 校验失败返回 `checksum_mismatch`。
- 三端审计记录文件大小和哈希摘要。

## 截图与窗口 E2E

命令：

```bash
rcwctl screenshot --output ./tmp/screen.png
file ./tmp/screen.png
rcwctl --json windows
```

验收：

- `screen.png` 是有效 PNG。
- 截图尺寸合理，非空白。
- `windows` 返回至少一个可见窗口，字段包含 handle/title/process_id/rect/visible/focused。
- 锁屏或无交互桌面时返回明确错误。

## 鼠标键盘 E2E

建议使用 Notepad：

1. 在 Windows 主机打开 Notepad。
2. `rcwctl windows` 找到 Notepad 窗口。
3. `rcwctl screenshot` 确认位置。
4. `rcwctl click --x <x> --y <y> --button left`。
5. `rcwctl type "hello from rcw"`。
6. `rcwctl key Ctrl+S` 或 `rcwctl key Enter` 做基础按键验证。
7. 再截图确认文本出现。

验收：

- 鼠标点击落点与截图一致。
- 文本输入成功。
- 快捷键映射成功。
- host 控制台和三端审计显示 mouse/keyboard 操作。

## 权限与 UAC E2E

普通运行：

- host 控制台显示 `Privilege: standard user`。
- `exec whoami /groups` 或 PowerShell 检查未 elevated。

管理员运行：

- 客户或测试人员手动右键以管理员身份运行 `rcw-host.exe`。
- host 控制台高亮显示 `Privilege: ADMINISTRATOR / elevated`。
- 工具不自动触发 UAC。

验收：

- 普通运行和管理员运行状态显示准确。
- 没有自动 UAC 提权行为。

## 会话生命周期 E2E

场景：

- `rcwctl` 多次短进程调用复用 session。
- `rcwctl close` 后旧 session 不可用。
- 关闭 host 窗口后旧 session 不可用。
- 重启 server 后旧 session 不可用。
- 不设置固定 TTL 和空闲超时，host 和 server 都不断开时 session 可持续复用。

验收：

- `rcwctl` 无 daemon，每次调用重新连接 server。
- host 到 server 的 WebSocket 持续存在。
- session 文件保存在用户级目录，除非 `--session` 覆盖。

## 审计对账

每个 E2E 操作后检查：

- host audit：实际执行或拒绝的操作。
- controller audit：CLI 发起的操作、参数摘要、输出路径、结果。
- server audit：认证、会话、命令中继、断开事件。

验收：

- 三端都包含同一 request ID。
- 不记录完整控制端 token、session token、TOTP seed、文件内容、完整剪贴板内容。
- host 控制台实时显示操作摘要。

## 发布验收

发布前至少执行：

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
cargo build --release -p rcw-server
cargo build --release -p rcwctl
cargo build --release -p rcw-host --target x86_64-pc-windows-msvc
```

如果本机无法交叉构建 Windows MSVC target，则在 Windows builder 上执行 host release build，并记录构建命令和 SHA-256。
