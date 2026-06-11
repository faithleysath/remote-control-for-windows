# 测试与验证

本文档面向维护者，记录验证计划和当前实机验证基线。它取代早期 E2E 规划文档，因为 v1 已经有可运行实现和 Windows VM 验证记录。

## 当前验证记录

2026-06-11，v1 在维护者本机的 Windows-in-Docker VM 中完成实机验证。`rcw-host.exe` 由 Linux 交叉构建后复制进 Windows VM；`rcw-server` 和 `rcwctl` 运行在 Linux 主机。

已验证：

- host 启动、连接 server、显示 machine ID/TOTP、更新剪贴板，并显示管理员 elevated 权限状态。
- `rcwctl open/status/close`。
- 错误控制端 token、错误 TOTP、TOTP 周期不一致均返回预期错误。
- 远程 Windows 命令执行。
- 命令超时返回 `RequestTimeout`，且被测 `pwsh` 进程无残留。
- 上传/下载 SHA-256 一致。
- 可见窗口枚举。
- 交互桌面截图生成 1280x720 PNG。
- 鼠标 move/click/scroll 和键盘 type/key 均执行成功，并通过 Notepad 截图确认。
- 剪贴板内容只包含 server、machine ID、验证码和有效期，不包含 control token、session token、TOTP seed 或原始机器标识。
- `powercfg /requests` 显示 `rcw-host.exe` 持有 `DISPLAY` 和 `SYSTEM` 请求；临时缩短 AC 显示/睡眠超时后，session 仍保持 active。
- `rcwctl close` 后恢复旧 session 文件再请求状态，返回 `SessionExpired`。
- server 和 host 审计日志包含可按 request ID 对齐的事件。

剩余验证缺口：

- 在真实标准用户交互桌面中启动 `rcw-host.exe`，确认控制台显示 `Privilege: standard user`。当前 VM 中的自动化尝试没有稳定产生可观察的标准用户桌面 host 进程；管理员交互桌面运行已经验证。

2026-06-11 的原始验证日志保留在维护者本机，未纳入公开仓库。需要复核时应重新运行本文档的 E2E 清单并保存新的证据。

## 必跑本地检查

代码变更提交前运行：

```bash
cargo fmt --check
cargo test --workspace
cargo clippy --workspace -- -D warnings
```

涉及 Windows host 代码或 manifest 变更时运行：

```bash
RUSTFLAGS='-C target-feature=+crt-static' \
  cargo xwin clippy -p rcw-host --target x86_64-pc-windows-msvc --release -- -D warnings
RUSTFLAGS='-C target-feature=+crt-static' \
  cargo xwin build -p rcw-host --target x86_64-pc-windows-msvc --release
```

## E2E 环境

最小真实测试拓扑：

- Linux/macOS 主机运行 `rcw-server` 和 `rcwctl`。
- Windows 机器或 VM 在交互桌面中运行 `rcw-host.exe`。
- Windows host 能主动通过 WebSocket 访问 `rcw-server`。
- 测试人员能读取 host 控制台中的 machine ID、TOTP、权限状态、剪贴板状态和操作摘要。

服务端示例：

```bash
export RCW_BIND_ADDR=0.0.0.0:7800
export RCW_CONTROL_TOKEN=test-control-token
export RCW_AUDIT_LOG=$PWD/tmp/server-audit.jsonl
rcw-server
```

被控端示例：

```powershell
.\rcw-host.exe --server ws://<server-ip>:7800 --totp-period-seconds 120
```

控制端示例：

```bash
export RCW_SERVER_URL=ws://<server-ip>:7800
export RCW_CONTROL_TOKEN=test-control-token
rcwctl open --id <machine-id> --totp <totp>
```

## E2E 清单

会话和鉴权：

- host 无 token 可登记在线。
- controller 没有正确控制端 token 不能 open。
- 错误 TOTP 失败。
- TOTP 周期不一致失败。
- 成功 open 后，`status` 返回 host online 和 session active。
- `close` 使 session 失效，并删除本地 session 文件。

命令执行：

- `exec` 返回 stdout、stderr、退出码和耗时。
- 长命令按控制端 timeout 设置超时。
- 超时后的子进程树被清理。
- host 控制台和三端审计日志都包含 request ID。

文件传输：

- upload 默认不覆盖已有文件。
- upload 带 `--overwrite` 可以替换文件。
- download 后 SHA-256 与源文件一致。
- 损坏或不匹配的传输返回 `checksum_mismatch` 或等价结构化错误。

GUI 操作：

- `screenshot` 从交互桌面返回有效、非空 PNG。
- `windows` 返回可见窗口，字段包含 handle、title、process ID、rect、visible 和 focused。
- 鼠标点击落点与截图坐标一致。
- 文本输入和常见按键在 Notepad 或其他简单焦点应用中可用。
- 锁屏、UAC 安全桌面或非交互 session 的错误必须明确，不能伪装成功。

安全和隐私：

- 剪贴板文本只包含可安全转发的连接信息。
- 日志不包含完整 control token、session token、TOTP seed、原始机器标识、文件内容或默认完整命令输出。
- elevated 和 standard user host 运行时显示正确权限状态。
- host 不会自行触发 UAC。

电源行为：

- host 运行期间持有 display/system 电源请求。
- host 退出后释放电源请求。
- 测试不会永久修改 Windows 电源计划。

## 发布门槛

发布前完成：

- 本地检查。
- Windows host 交叉构建。
- 至少一次 Windows 交互桌面 E2E。
- 审计脱敏抽查。
- 所有发布产物的 SHA-256 校验和。
