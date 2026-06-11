# Remote Control for Windows

`remote-control-for-windows` 是一套面向研发远程协助客户 Windows 环境的临时远控工具。它的核心目标不是替代完整商业远控产品，而是让 Codex agent 可以通过命令行介入客户现场，执行必要的诊断、文件传输、截图、鼠标点击和键盘输入。

首版包含三个 Rust 二进制：

- `rcw-server`：公网或内网部署的 WebSocket 中继服务器。
- `rcw-host.exe`：客户 Windows 机器上双击运行的被控端。
- `rcwctl`：研发侧控制端 CLI，供人工或 Codex agent 调用。

## 首版定位

- 被控端只优先支持 Windows。
- 控制端 CLI 和服务器支持 Linux、macOS、Windows。
- 被控端不常驻、不安装服务、不自启动、不隐藏运行。
- 客户双击运行被控端后，控制台持续显示机器 ID、当前 TOTP 和连接状态。
- 被控端启动和 TOTP 刷新时自动复制连接信息到 Windows 剪贴板，方便客户直接通过 QQ 发给研发。
- 被控端运行期间阻止系统休眠和显示器熄屏，关闭被控端后恢复系统默认电源行为。
- 三端都保留操作审计日志，被控端控制台实时显示所有远控操作摘要。
- 被控端不自动 UAC 提权；如客户右键以管理员身份运行，被控端必须高亮显示当前处于管理员权限。
- 研发通过机器 ID、TOTP 和控制端 token 建立一次远控会话。
- TOTP 有效周期默认 120 秒，并允许打包或运行时配置，以适应客户沟通延迟。
- 会话建立后，在会话有效期内不需要重复认证。
- 关闭被控端窗口即断开控制。

## 文档索引

- [产品需求](docs/product-requirements.md)
- [产品设计](docs/product-design.md)
- [技术架构](docs/architecture.md)
- [协议设计](docs/protocol.md)
- [CLI 设计](docs/cli.md)
- [安全与权限边界](docs/security.md)
- [配置与打包](docs/configuration.md)
- [路线图](docs/roadmap.md)
- [实现计划](docs/implementation-plan.md)
- [E2E 测试计划](docs/e2e-test-plan.md)
- [Windows API 清单](docs/windows-apis.md)

## 基本使用形态

客户侧：

```powershell
.\rcw-host.exe
```

窗口持续输出：

```text
Remote Control for Windows Host
Server: wss://remote.example.com
Machine ID: 8K4F-2M7Q
Current TOTP: 183942
Status: waiting for controller
Clipboard: connection info copied
```

研发侧：

```bash
export RCW_SERVER_URL=wss://remote.example.com
export RCW_CONTROL_TOKEN=...

rcwctl open --id 8K4F-2M7Q --totp 183942
rcwctl exec -- pwsh -NoProfile -Command "Get-ComputerInfo"
rcwctl screenshot --output screen.png
rcwctl upload ./tool.exe 'C:\Users\Public\tool.exe'
rcwctl close
```

## 非目标

首版明确不做以下事情：

- 静默控制、隐藏窗口、绕过客户感知。
- 自动 UAC 提权、UAC 绕过、持久化驻留。
- P2P 打洞和复杂 NAT 优化。
- 多租户管理后台、组织权限、计费系统。
- 中央审计数据库和录像回放。

## 当前状态

当前仓库已经创建 Rust workspace，完成 v1 主链路实现，并完成本地与 Windows VM 实机验证：

- `rcw-common`：协议、配置、ID/session token、TOTP、审计 JSONL 和 SHA-256 工具。
- `rcw-server`：`/healthz`、`/ws/host`、`/ws/control`、控制端 token 校验、TOTP 转发认证、内存 session、status/close、命令和 binary frame 中继、WebSocket ping 心跳、基础限流。
- `rcw-host`：被控端连接、断线重连、机器 ID/TOTP 显示、剪贴板刷新、会话认证、流式命令输出、`upload.begin`/`download.begin` 分块文件传输、截图 binary chunk 返回、审计日志和 Win32 API 平台操作入口。
- `rcwctl`：`open/status/exec/upload/download/screenshot/windows/move/click/scroll/type/key/close`，会话文件复用、JSON 输出和控制端审计。

截至 2026-06-11，已验证：

- Linux 本地：`cargo fmt --check`、`cargo test --workspace`、`cargo clippy --workspace -- -D warnings`、本地 server + host + rcwctl smoke。
- Linux 到 Windows MSVC 交叉编译：`RUSTFLAGS='-C target-feature=+crt-static' cargo xwin build -p rcw-host --target x86_64-pc-windows-msvc --release`，产物为 x86-64 Windows console PE，未依赖 `VCRUNTIME140.dll`。
- Windows VM 实机：`open/status/close`、错误 token/TOTP/TOTP period、命令执行、命令超时和子进程清理、上传/下载 SHA-256、窗口枚举、截图、鼠标移动/点击/滚轮、键盘输入/按键、剪贴板内容边界、电源防休眠/熄屏请求、session close 后旧 session 失效、server/host 审计。

仍需补充的验收项：

- 在真实标准用户桌面中启动 `rcw-host.exe`，确认控制台显示 `Privilege: standard user`。当前 Windows VM 中已验证管理员 elevated 显示；自动化尝试创建临时标准用户运行 host 未能稳定启动到可观测桌面。
