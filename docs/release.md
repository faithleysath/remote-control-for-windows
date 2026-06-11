# 发布流程

项目使用 GitHub Actions 自动化发布流水线：`.github/workflows/release.yml`。发布 workflow 会在干净 runner 中完成校验、构建、打包、生成 SHA-256，并创建 GitHub Release。

## 版本策略

workspace 当前版本为 `0.1.0`。在采用更完整的兼容性策略前：

- patch release 不应破坏协议语义或 CLI 行为。
- 破坏 wire format 的改动必须提升协议版本。
- 安全修复应在 `CHANGELOG.md` 中明确说明。

## 触发方式

推荐使用 tag 触发：

```bash
git tag v0.1.0
git push origin v0.1.0
```

也可以在 GitHub Actions 页面手动运行 `Release` workflow，并填写 tag，例如 `v0.1.0`。手动运行时 workflow 会以当前 commit 为目标创建 release tag。

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
7. 确认 `CHANGELOG.md` 中的版本号与要推送的 tag 一致。

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

自动发布会生成：

```text
GitHub Release assets:
  rcw-host-x86_64-pc-windows-msvc.zip
  rcwctl-x86_64-unknown-linux-gnu.tar.gz
  rcwctl-x86_64-pc-windows-msvc.zip
  rcw-server-x86_64-unknown-linux-gnu.tar.gz
  checksums.txt
```

`checksums.txt` 使用 SHA-256，包含所有发布包。

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
