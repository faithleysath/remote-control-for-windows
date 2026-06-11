# 发布流程

项目目前还没有自动化发布流水线。发布从干净 checkout 中手动准备。

## 版本策略

workspace 当前版本为 `0.1.0`。在采用更完整的兼容性策略前：

- patch release 不应破坏协议语义或 CLI 行为。
- 破坏 wire format 的改动必须提升协议版本。
- 安全修复应在 `CHANGELOG.md` 中明确说明。

## 发布前清单

1. 确认 `CHANGELOG.md` 已记录本次发布。
2. 确认 `README.md` 和 `docs/` 描述的是即将发布的行为。
3. 运行本地检查：

   ```bash
   cargo fmt --check
   cargo test --workspace
   cargo clippy --workspace -- -D warnings
   ```

4. 运行 Windows host 交叉检查：

   ```bash
   RUSTFLAGS='-C target-feature=+crt-static' \
     cargo xwin clippy -p rcw-host --target x86_64-pc-windows-msvc --release -- -D warnings
   RUSTFLAGS='-C target-feature=+crt-static' \
     cargo xwin build -p rcw-host --target x86_64-pc-windows-msvc --release
   ```

5. 按 [testing.md](testing.md) 运行或刷新 Windows 交互桌面 E2E smoke。
6. 确认发布产物不需要任何 secret 或本地配置文件。

## 构建命令

Linux controller 和 server：

```bash
cargo build --release -p rcwctl
cargo build --release -p rcw-server
```

从 Linux 构建 Windows host：

```bash
rustup target add x86_64-pc-windows-msvc
RUSTFLAGS='-C target-feature=+crt-static' \
  cargo xwin build -p rcw-host --target x86_64-pc-windows-msvc --release
```

预期 host 产物：

```text
target/x86_64-pc-windows-msvc/release/rcw-host.exe
```

## 产物结构

建议发布包结构：

```text
release/
  rcw-host-x86_64-pc-windows-msvc.zip
  rcwctl-x86_64-unknown-linux-gnu.tar.gz
  rcw-server-x86_64-unknown-linux-gnu.tar.gz
  checksums.txt
```

`checksums.txt` 使用 SHA-256，包含所有发布文件。

## Windows Host 验证

发布 `rcw-host.exe` 前确认：

- 文件是 Windows x86-64 PE 可执行文件。
- 静态 CRT 构建在干净 Windows 环境中不依赖 `VCRUNTIME140.dll`。
- host 能从交互桌面启动并连接中继。
- 截图和输入操作在交互桌面中测试过，而不只是 session 0 或非交互服务上下文。

## 发布后

- 给发布 commit 打 tag。
- 将构建命令、校验和和 E2E 证据随 release notes 保存。
- 新发现的运行时缺口应写入 `docs/testing.md` 或 `docs/roadmap.md`，不要只留在聊天记录里。
