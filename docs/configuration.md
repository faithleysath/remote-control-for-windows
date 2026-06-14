# 配置与打包

## 配置项

### 服务器地址

配置名：

- CLI 参数：`--server`
- 环境变量：`RCW_SERVER_URL`
- 编译期环境变量：`RCW_EMBED_SERVER_URL`

优先级：

1. CLI 参数。
2. 运行时环境变量。
3. 编译期嵌入值。

示例：

```bash
RCW_EMBED_SERVER_URL=wss://remote.example.com cargo build --release -p rcw-host
```

运行时覆盖：

```powershell
$env:RCW_SERVER_URL = "wss://test-remote.example.com"
.\rcw-host.exe
```

### 控制端 token

配置名：

- CLI 参数：`--token`
- 环境变量：`RCW_CONTROL_TOKEN`
- 服务端环境变量：`RCW_CONTROL_TOKEN`

控制端 token 不提供编译期嵌入，避免把敏感凭据固化到可分发二进制。
被控端不读取、不需要、也不应配置控制端 token。

### TOTP 周期

配置名：

- 被控端环境变量：`RCW_TOTP_PERIOD_SECONDS`
- 控制端环境变量：`RCW_TOTP_PERIOD_SECONDS`
- 编译期环境变量：`RCW_EMBED_TOTP_PERIOD_SECONDS`

默认值为 `120` 秒。该默认值刻意比常见 30 秒 TOTP 更长，用于适应客户通过聊天、电话或截图传递验证码时的沟通延迟。

优先级：

1. 运行时环境变量。
2. 编译期嵌入值。
3. 默认值 `120`。

控制端和被控端必须使用相同周期；周期不一致时，`rcwctl connect` 应失败并提示重新检查配置。

### 服务端监听

服务端配置：

- `RCW_BIND_ADDR`：默认 `127.0.0.1:7800`。
- `RCW_CONTROL_TOKEN`：必填。
- `RCW_LOG`：日志级别，默认 `info`。
- `RCW_AUDIT_LOG`：服务端审计日志路径，默认写入当前工作目录下的 `rcw-server-audit.jsonl`。

示例：

```bash
RCW_BIND_ADDR=127.0.0.1:7800 \
RCW_CONTROL_TOKEN=... \
rcw-server
```

## 打包目标

当前发布产物：

- `rcw-host.exe`：Windows x64。
- `rcwctl`：Linux x64、macOS、Windows x64。
- `rcw-server`：Linux x64 优先。

## Windows 被控端构建

当前 Linux 开发机可以通过 `cargo-xwin` 交叉编译 Windows MSVC 目标，不需要在目标 Windows 机器上安装 Rust。

推荐构建命令：

```bash
rustup target add x86_64-pc-windows-msvc
RUSTFLAGS='-C target-feature=+crt-static' \
  cargo xwin build -p rcw-host --target x86_64-pc-windows-msvc --release
```

产物路径：

```text
target/x86_64-pc-windows-msvc/release/rcw-host.exe
```

使用 `crt-static` 可以避免干净 Windows 环境缺少 `VCRUNTIME140.dll` 等 VC++ 运行库。2026-06-14 的 `0.1.6` x86-64 Windows host 产物已在维护者本机的 Windows VM 中完成协议 v4 实机 E2E；`0.1.7` 追加完成 125% DPI host 侧修复验证。验证记录见 [v0.1.6 E2E 测试报告](e2e-v0.1.6.md) 和 [v0.1.7 E2E 修复验证报告](e2e-v0.1.7.md)。

如果在其他机器上无法使用 `cargo-xwin`，可以改在 Windows builder 上执行同等 release 构建，并记录构建命令和 SHA-256。

## 服务端部署

推荐部署：

```text
Caddy/Nginx TLS 443
        |
  127.0.0.1:7800 rcw-server
```

Caddy 示例：

```caddyfile
remote.example.com {
  reverse_proxy 127.0.0.1:7800
}
```

服务端健康检查：

```bash
curl https://remote.example.com/healthz
```

## 本地会话文件

`rcwctl` 默认会话文件路径：

- Linux：`~/.local/share/rcwctl/session.json`
- macOS：`~/Library/Application Support/rcwctl/session.json`
- Windows：`%APPDATA%\rcwctl\session.json`

会话文件内容包含：

- server URL。
- machine ID。
- session ID。
- session token。
- created_at。
- last_used_at。

会话文件权限应尽量限制为当前用户可读写。

## 三端审计日志

默认审计日志路径：

- 被控端 Windows：`%LOCALAPPDATA%\RemoteControlForWindows\host-audit.jsonl`
- 控制端 Linux：`~/.local/share/rcwctl/audit.jsonl`
- 控制端 macOS：`~/Library/Application Support/rcwctl/audit.jsonl`
- 控制端 Windows：`%APPDATA%\rcwctl\audit.jsonl`
- 服务端：`./rcw-server-audit.jsonl` 或 `RCW_AUDIT_LOG`

日志格式为 JSON Lines。每行是一条独立事件，包含 `time`、`side`、`host_id`、`machine_id`、`session_id`、`request_id`、`event`、`result` 等字段。

被控端控制台必须同步显示审计摘要，不依赖日志文件打开成功。即使本地日志写入失败，也要继续把远控操作实时显示在控制台。

被控端 `Host ID` 是进程运行期随机值，不写入磁盘。启动窗口和剪贴板连接信息会显示当前 `Host ID`；进程重启后该值会变化。

## 权限状态显示

被控端启动时检测 Windows 当前进程是否 elevated：

- 普通权限：显示 `Privilege: standard user`。
- 管理员权限：使用醒目控制台颜色显示 `Privilege: ADMINISTRATOR / elevated`。

被控端不主动触发 UAC，不提供自动提权参数。需要管理员权限时，客户必须自行右键选择以管理员身份运行。

## 电源状态

被控端运行期间应请求 Windows 阻止系统休眠和显示器熄屏，以避免远控会话中断。该请求只在 `rcw-host.exe` 进程运行期间有效，不写入系统电源计划，不安装服务，也不修改注册表。

被控端退出后必须释放该请求，让 Windows 恢复默认电源行为。电源请求失败时，被控端继续运行，并在控制台和审计日志中输出 warning。

## 剪贴板连接信息

被控端启动后，以及每次 TOTP 刷新后，应自动写入 Windows 剪贴板：

- 服务器地址。
- 机器 ID。
- 当前 TOTP。
- TOTP 有效期。

剪贴板内容不得包含：

- 控制端 token。
- session token。
- TOTP seed。
- 原始机器标识。

剪贴板写入失败时，被控端继续运行，并在控制台提示客户手动复制连接信息。

## 版本信息

当前 `rcwctl` 和 `rcw-host.exe` 通过 clap 支持：

```bash
--version
```

输出当前 crate 版本号，例如 `rcwctl 0.1.5`。`rcw-server` 当前没有独立 `--version` 参数；服务启动后可通过 `/healthz` 查看服务名和 `protocol_version`。

## 发布包

当前自动发布结构：

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
```

`rcw-tools-*` 包含 `rcwctl` 和 `rcw-server`，`rcw-host-*` 包含 Windows 被控端。`checksums.txt` 使用 SHA-256。
