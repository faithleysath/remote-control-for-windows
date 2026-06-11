# 贡献指南

这个仓库维护的是一个安全敏感的临时远程协助工具。任何改动都应保持工具可见、临时、可审计，并且只能在运行 `rcw-host.exe` 的人员明确授权下使用。

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
- 保持被控端可见行为：控制台状态、权限显示、操作摘要和关闭窗口即终止控制。
- 不增加持久化、静默运行、自启动、驱动安装、进程注入、键盘记录或 UAC 绕过行为。
- 修改 CLI 参数、协议 payload、安全边界或发布命令时，同步更新文档和测试说明。

## 测试要求

仅文档改动可以通过链接和文本检查验证。代码改动应运行 workspace 检查。Windows 专属改动需要 Windows VM/实机验证，或者在提交说明中明确剩余运行时验证缺口。
