# 发布流程

项目使用 GitHub Actions 自动化发布流水线：`.github/workflows/release.yml`。发布 workflow 会在干净 runner 中完成校验、构建、打包、生成 SHA-256，并创建 GitHub Release。

发布流水线还会发布 npm 包 `rcwctl` 以及对应的平台二进制包。npm 元包负责把 `rcwctl` 暴露成一个统一命令，平台包负责把预编译二进制直接放进 npm tarball，这样 npm 镜像也能完整分发。

## 版本策略

workspace 当前版本为 `0.1.3`。在采用更完整的兼容性策略前：

- patch release 不应破坏协议语义或 CLI 行为。
- 破坏 wire format 的改动必须提升协议版本。
- 安全修复应在 `CHANGELOG.md` 中明确说明。

## 触发方式

推荐使用 tag 触发：

```bash
git tag v0.1.3
git push origin v0.1.3
```

也可以在 GitHub Actions 页面手动运行 `Release` workflow，并填写 tag，例如 `v0.1.3`。手动运行时 workflow 会以当前 commit 为目标创建 release tag。

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
8. 确认 `npm/package.json` 和 `npm/packages/*/package.json` 的版本号与 tag 去掉 `v` 后一致。
9. 在 npmjs.com 上为这 7 个 npm 包分别配置 GitHub Actions trusted publisher，工作流文件使用 `.github/workflows/release.yml`，允许 `npm publish`。发布流程不再依赖 `NPM_TOKEN` secret。

## 目标平台

自动发布目标：

- Linux x86-64：`x86_64-unknown-linux-gnu`
- Linux arm64：`aarch64-unknown-linux-gnu`
- macOS x86-64：`x86_64-apple-darwin`
- macOS arm64：`aarch64-apple-darwin`
- Windows x86-64：`x86_64-pc-windows-msvc`
- Windows arm64：`aarch64-pc-windows-msvc`

`rcwctl` 和 `rcw-server` 会发布到上述全部目标。`rcw-host.exe` 只发布 Windows x86-64 和 Windows arm64。

npm 侧会发布一个元包和六个平台包：

- `rcwctl`
- `rcwctl-linux-x64`
- `rcwctl-linux-arm64`
- `rcwctl-darwin-x64`
- `rcwctl-darwin-arm64`
- `rcwctl-win32-x64`
- `rcwctl-windows-arm64`

元包通过 `optionalDependencies` 自动选择当前平台对应的平台包，平台包里直接包含 `rcwctl` 二进制。

## 本地构建命令

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
  rcw-tools-x86_64-unknown-linux-gnu.tar.gz
  rcw-tools-aarch64-unknown-linux-gnu.tar.gz
  rcw-tools-x86_64-apple-darwin.tar.gz
  rcw-tools-aarch64-apple-darwin.tar.gz
  rcw-tools-x86_64-pc-windows-msvc.zip
  rcw-tools-aarch64-pc-windows-msvc.zip
  rcw-host-x86_64-pc-windows-msvc.zip
  rcw-host-aarch64-pc-windows-msvc.zip
  checksums.txt

npm packages:
  rcwctl
  rcwctl-linux-x64
  rcwctl-linux-arm64
  rcwctl-darwin-x64
  rcwctl-darwin-arm64
  rcwctl-win32-x64
  rcwctl-windows-arm64
```

`rcw-tools-*` 包含 `rcwctl` 和 `rcw-server`。`rcw-host-*` 包含 Windows 被控端。`checksums.txt` 使用 SHA-256，包含所有发布包。

`rcwctl` 不再走 `postinstall` 下载 GitHub Release；它只负责把平台包作为依赖暴露给用户。

## npm 发布

发布 workflow 的 `publish-npm` job 在 GitHub Release 创建成功后运行：

1. 使用 Node.js 检查 `npm/package.json` 和平台包版本是否匹配 release tag。
2. 运行 `npm test` 和 `npm run pack:check`。
3. 从 build artifacts 里为每个平台包补入 `rcwctl` 二进制。
4. 执行 `npm publish --access public --registry=https://registry.npmjs.org`，先发平台包，再发元包。

当前 workflow 通过 GitHub Actions trusted publishing 发布：先发平台包，再发元包；npm 会在 OIDC 发布时自动生成 provenance，不需要额外的 `--provenance` flag。

## Windows Host 验证

发布 `rcw-host.exe` 前确认：

- 文件是 Windows x86-64 PE 可执行文件。
- 静态 CRT 构建在干净 Windows 环境中不依赖 `VCRUNTIME140.dll`。
- host 能从交互桌面启动并连接中继。
- 截图和输入操作在交互桌面中测试过，而不只是 session 0 或非交互服务上下文。

## 发布后

- 给发布 commit 打 tag。
- 将构建命令、校验和和 E2E 证据随 release notes 保存。
- 验证 npm 包可安装：

  ```bash
  npm install -g rcwctl --registry=https://registry.npmjs.org
  rcwctl --version
  ```

- 新发现的运行时缺口应写入 `docs/testing.md` 或 `docs/roadmap.md`，不要只留在聊天记录里。
