# Remote Control for Windows

`remote-control-for-windows` 是一套临时、可见、可审计的 Windows 远程协助工具。它面向授权支持场景，让研发、运维或 Codex agent 可以通过命令行完成诊断、文件传输、截图和基础 GUI 操作。

项目现在已经从 v1 的从零实现阶段进入长期维护和迭代阶段。v1 远控主链路已经实现，并在真实 Windows VM 中完成主要闭环验证；后续工作应在保持当前安全模型的前提下继续强化打包、自动化验证和操作体验。

## 组件

- `rcw-server`：WebSocket 中继服务器，连接被控端和控制端。
- `rcw-host.exe`：客户或测试人员在 Windows 上运行的可见被控端。
- `rcwctl`：研发、脚本或 Codex agent 使用的控制端 CLI。
- `rcw-common`：共享协议、ID、TOTP、审计、配置和传输逻辑。

```text
rcwctl  <--WebSocket-->  rcw-server  <--WebSocket-->  rcw-host.exe
```

所有连接都由 host/control 侧主动发起。被控端上线不需要控制端 token；控制端必须同时持有服务端控制 token 和被控端当前 TOTP，才能打开会话。

## 当前状态

当前代码主链路已经实现，早期基线在 2026-06-11 完成过 Windows VM 实机验证：

- 本地 Rust 检查：`cargo fmt --check`、`cargo test --workspace`、`cargo clippy --workspace -- -D warnings`。
- Linux 上通过 `cargo-xwin` 交叉构建静态 CRT 的 Windows 被控端。
- Windows VM 实机 E2E：会话 `connect/status/disconnect`、错误 token/TOTP/TOTP 周期处理、命令执行、命令超时清理、上传/下载 SHA-256、窗口枚举、截图、鼠标移动/点击/滚轮、键盘文本和按键输入、剪贴板安全边界、防休眠/防熄屏请求、旧 session 失效、server/host 审计日志。

已实现但仍需刷新实机验证的项：

- 协议 v4 的 host identity/routing、`command.start` / `command.status` server-owned exec job。
- CLI/MCP 的后台 exec 查询和取消。
- MCP 进程内 upload/download 后台任务和取消路径。

仍需补齐的早期验证项：

- 在真实标准用户交互桌面中启动 `rcw-host.exe`，最终确认控制台显示 `Privilege: standard user`。管理员 elevated 桌面行为已经验证通过。

## 安全模型

这个工具明确不是静默远控工具：

- Windows 被控端是可见控制台进程。
- 关闭被控端窗口即终止控制。
- 被控端不安装服务、不写入启动项、不隐藏自身、不在退出后驻留。
- 被控端不自动提权，也不绕过 UAC。
- 被控端控制台显示当前权限状态和操作摘要。
- 剪贴板连接信息不包含控制端 token、session token、TOTP seed 或原始机器标识。
- host、controller、server 三端都记录审计日志。

完整安全边界见 [docs/security.md](docs/security.md)。

## 快速开始

安装控制端 CLI：

```bash
npm install -g rcwctl
rcwctl --version
```

启动本地中继服务器：

```bash
export RCW_BIND_ADDR=127.0.0.1:7800
export RCW_CONTROL_TOKEN='replace-with-a-random-token'
cargo run -p rcw-server
```

在 Windows 上启动被控端：

```powershell
.\rcw-host.exe --server ws://<server-host>:7800
```

打开控制端会话并执行命令：

```bash
export RCW_SERVER_URL=ws://127.0.0.1:7800
export RCW_CONTROL_TOKEN='replace-with-a-random-token'

rcwctl connect --id <machine-id> --totp <current-totp>
rcwctl status
rcwctl exec -- pwsh -NoProfile -Command "hostname"
rcwctl screenshot --output screen.png
rcwctl disconnect
```

如果现场短 `machine_id` 冲突，可以让被控端提供启动窗口或剪贴板里的当前 `Host ID`，再精确寻址：

```bash
rcwctl connect --id <machine-id> --host-id <host-id> --totp <current-totp>
```

给 agent 使用时建议开启 JSON 输出：

```bash
rcwctl --json exec -- pwsh -NoProfile -Command "hostname"
```

也可以把控制端作为 stdio MCP 服务器交给 MCP 客户端长期运行：

```json
{
  "mcpServers": {
    "rcw": {
      "command": "npx",
      "args": [
        "-y",
        "rcwctl",
        "--server",
        "ws://127.0.0.1:7800",
        "--token",
        "replace-with-control-token",
        "mcp"
      ]
    }
  }
}
```

MCP 模式下先调用 `connect` 打开远控会话，再调用 `exec`、`screenshot`、`windows`、鼠标键盘和文件传输工具。`exec` 默认远端运行上限为 24 小时，单次 tool call 默认等待 90 秒；未完成时返回 server-owned `task_id`，后续可用 `exec_status` 查询或 `exec_cancel` 取消。agent 发送或接收文件使用 `upload` / `download` 这类路径型工具，让 MCP 服务器自己流式读写本机文件；文件主体走 WebSocket binary frame，不走 base64。`upload` / `download` 默认等待 60 秒，未完成就返回 MCP 进程内 `task_id`，后续用 `transfer_status` 查询。MCP 进程只在内存中保存 session 和后台传输任务状态，不写普通 CLI 的本地 session 文件。

## 构建

本地 Linux 构建：

```bash
cargo build --workspace
cargo test --workspace
```

如果只需要使用控制端，优先通过 npm 安装预编译 `rcwctl`；本地 Rust 构建主要面向开发和发布验证。

从 Linux 交叉构建 Windows 被控端：

```bash
rustup target add x86_64-pc-windows-msvc
RUSTFLAGS='-C target-feature=+crt-static' \
  cargo xwin build -p rcw-host --target x86_64-pc-windows-msvc --release
```

产物路径：

```text
target/x86_64-pc-windows-msvc/release/rcw-host.exe
```

静态 CRT 构建可以避免干净 Windows 环境缺少 VC++ 运行库。

## 开发检查

提交变更前至少运行：

```bash
cargo fmt --check
cargo test --workspace
cargo clippy --workspace -- -D warnings
```

如果改动涉及 Windows host 侧代码，还应运行：

```bash
RUSTFLAGS='-C target-feature=+crt-static' \
  cargo xwin clippy -p rcw-host --target x86_64-pc-windows-msvc --release -- -D warnings
RUSTFLAGS='-C target-feature=+crt-static' \
  cargo xwin build -p rcw-host --target x86_64-pc-windows-msvc --release
```

## 文档

根目录 `README.md` 是项目首页，面向第一次进入仓库的人；`docs/README.md` 是文档目录索引，面向需要深入查阅设计、测试、发布和维护资料的人。两者不是重复正文，职责不同。

- [文档索引](docs/README.md)
- [项目范围](docs/project-scope.md)
- [技术架构](docs/architecture.md)
- [协议设计](docs/protocol.md)
- [CLI 参考](docs/cli.md)
- [配置说明](docs/configuration.md)
- [测试与验证](docs/testing.md)
- [发布流程](docs/release.md)
- [路线图](docs/roadmap.md)
- [安全模型](docs/security.md)
- [Windows 实现说明](docs/windows-apis.md)

## 贡献

见 [CONTRIBUTING.md](CONTRIBUTING.md)。涉及安全边界的改动必须保持客户可见、显式授权、token 脱敏和操作可审计。

## 许可证

本项目使用 MIT 许可证。见 [LICENSE](LICENSE)。
