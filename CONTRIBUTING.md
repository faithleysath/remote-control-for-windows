# 贡献指南

这个仓库维护的是一个安全敏感的 Windows 远控产品。当前已实现基线是可见、可审计的临时协助模式，后续会在不突破安全边界的前提下扩展到常驻可连接和长期配对。任何改动都应保持授权明确、状态可见、操作可审计，并且只能在运行或显式启用了 host 的人员明确授权下使用。

## 开发环境

要求：

- Rust stable 工具链。
- 用于 Linux 到 Windows MSVC 交叉构建的 `cargo-xwin`。
- 修改 `rcw-host` 平台行为时，需要 Windows 机器或 VM 做 GUI 和权限验证。

常用检查命令：

```bash
cargo fmt --check
cargo test --workspace
cargo clippy --workspace -- -D warnings
```

修改 Windows host 侧代码时：

```bash
rustup target add x86_64-pc-windows-msvc
RUSTFLAGS='-C target-feature=+crt-static' \
  cargo xwin clippy -p rcw-host --target x86_64-pc-windows-msvc --release -- -D warnings
RUSTFLAGS='-C target-feature=+crt-static' \
  cargo xwin build -p rcw-host --target x86_64-pc-windows-msvc --release
```

## 改动要求

- 协议改动默认保持向后兼容；确实破坏兼容时必须明确提升协议版本。
- 不记录完整控制端 token、session token、TOTP seed、原始机器 ID、剪贴板内容或文件内容。
- 保持被控端可见行为：当前控制台状态、权限显示、操作摘要和关闭窗口即终止控制；后续若增加常驻模式，也必须保持显式启用、状态可见、可撤销。
- 不增加静默运行、隐藏后台控制、未经显式启用的自启动或持久化、驱动安装、进程注入、键盘记录或 UAC 绕过行为。
- 修改 CLI 参数、协议 payload、安全边界或发布命令时，同步更新文档和测试说明。
- 调试流程、Windows VM 投递路径或 guest 侧验证脚本发生变化时，同步更新 `docs/debug-workflow.md` 和 `scripts/debug/windows/` 下对应脚本。

## 测试要求

测试分层、门禁和当前 `pytest` smoke/E2E 脚手架见 `docs/testing.md`。最低要求如下：

- 仅文档改动：链接和文本检查即可。
- 一般代码改动：至少运行 workspace 检查。
- 协议、等待窗口、错误码、session/command/tunnel 语义改动：除 workspace 检查外，应补相应 `unit / contract` 或 `integration / protocol` 证据。
- Windows 桌面、截图、窗口、输入、CLI/MCP 真实链路改动：除 workspace 检查外，应补本机 + Windows VM smoke，或明确剩余缺口。
- GUI / WebView2 / CDP 改动：除 workspace 检查外，应补 GUI smoke，或明确剩余缺口。

涉及 Windows 交互桌面、截图、窗口枚举、输入、GUI 生命周期或 WebView2/CDP 的改动，不应只以 SSH / `SessionId=0` 路径作为验证依据；默认应参考 `docs/debug-workflow.md` 中已验证的 `SessionId=1` 交互式调试路径。
