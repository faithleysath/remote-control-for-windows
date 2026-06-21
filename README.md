# Remote Control for Windows

`remote-control-for-windows` 是一套面向明确授权场景的 Windows 远控产品基线。当前已实现的是可见、可审计的临时协助 / TOTP 模式；后续会在不打破现有安全边界的前提下扩展到显式启用的长期配对和常驻可连接模式。

当前 workspace 版本是 `0.1.11`，协议版本是 `v6`。

## 组件

- `rcw-server`：WebSocket 中继服务器，连接被控端和控制端。
- `rcw-host.exe`：客户或测试人员在 Windows 上运行的可见被控端。
- `rcw-host-gui`：Tauri v2 GUI host 工程，提供概览、会话、Exec、传输、隧道、审计和设置页 MVP。
- `rcwctl`：研发、脚本或 Codex agent 使用的控制端 CLI。
- `rcw-common`：共享协议、ID、TOTP、审计、配置和传输逻辑。

当前基线已经覆盖会话建立、命令执行、文件传输、截图、窗口枚举、鼠标键盘输入、server-owned 后台 exec、TCP tunnel 和 MCP 进程内任务状态；更细的产品边界见 [docs/project-scope.md](docs/project-scope.md)。

## 运行模型

```text
rcwctl  <--WebSocket-->  rcw-server  <--WebSocket-->  rcw-host.exe
```

所有连接都由 host/control 侧主动发起。当前临时协助模式下，被控端上线不需要控制端 token；控制端必须同时持有服务端控制 token 和被控端当前 TOTP，才能打开会话。session token 始终是短期的；MCP 模式把 session、传输任务和 tunnel manager 状态保存在进程内存里，不污染普通 CLI 会话文件。

## 安全边界

- 当前 `rcw-host.exe` 必须保持为可见进程，关闭窗口即结束临时协助模式。
- 产品不做静默控制、自动提权、UAC 绕过、隐藏进程或退出后后台驻留。
- 剪贴板连接信息和默认审计摘要不包含 control token、session token、TOTP seed 或其他长期凭据。

完整边界见 [docs/security.md](docs/security.md)。

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
rcwctl exec --timeout 30s --wait 90s -- pwsh -NoProfile -Command "hostname"
rcwctl exec-status <task_id>
rcwctl exec-cancel <task_id>
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

MCP 模式下先调用 `connect` 打开远控会话，再调用 `exec`、`screenshot`、`windows`、鼠标键盘和文件传输工具。`exec` 默认远端运行上限为 24 小时，单次 tool call 默认等待 90 秒；`upload` / `download` 默认等待 60 秒；未完成时都改为返回可继续查询的任务 ID。更完整的 CLI/MCP 约定见 [docs/cli.md](docs/cli.md)。

## 构建与检查

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

发布产物、目标平台和发布校验见 [docs/release.md](docs/release.md)。

## 文档导航

根目录 `README.md` 负责入口说明；`docs/README.md` 负责文档索引。

- [文档索引](docs/README.md)
- [开发工作流](docs/dev-workflow.md)
- [项目范围](docs/project-scope.md)
- [技术架构](docs/architecture.md)
- [协议设计](docs/protocol.md)
- [CLI 参考](docs/cli.md)
- [调试工作流](docs/debug-workflow.md)
- [测试工作流](docs/testing.md)
- [配置说明](docs/configuration.md)
- [发布流程](docs/release.md)
- [安全模型](docs/security.md)
- [Windows 实现说明](docs/windows-apis.md)

## 贡献

见 [CONTRIBUTING.md](CONTRIBUTING.md)。涉及安全边界的改动必须保持客户可见、显式授权、token 脱敏和操作可审计。

## 许可证

自本次许可证迁移提交起，本项目使用 GPL-3.0-or-later 许可证。见 [LICENSE](LICENSE)。

迁移前已经发布的版本仍按其发布时的 MIT 许可证授权；本次变更不追溯修改既有版本或既有提交的授权条款。
