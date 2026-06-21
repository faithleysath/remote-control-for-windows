# 配置说明

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

日志格式为 JSON Lines。每行是一条独立事件，包含 `time`、`side`、`event`、`category`、`host_id`、`machine_id`、`session_id`、`request_id`、`task_id`、`command`、`command_kind`、`result`、`duration_ms`、`summary` 等字段。旧字段保持兼容，新增字段都是可选字段，旧版本 JSONL 仍可逐行读取。

被控端 host audit 默认写结构化摘要，用于后续 GUI 时间线和本地 JSONL 共用：

- `category`：`host`、`session`、`exec`、`transfer`、`tunnel`、`input` 或 `error`。
- `controller_label` / `audit_label`：控制端身份说明和调用者提供的审计说明，写入前会折叠换行并截断。
- `args_summary`：命令参数摘要。`exec` 只记录 program basename、argv 数量和 timeout，不记录 argv 明文。
- `path_summary`：文件路径摘要，只记录 basename 或工作目录 basename，不记录完整用户目录。
- `bytes` / `size` / `sha256`：输入长度、传输大小和传输校验哈希；不记录文件内容。
- `started_at` / `finished_at`：长任务或可持续操作的开始/结束时间。
- `error_code` / `error_message`：失败时的错误码和脱敏后的错误消息。

脱敏规则：

- `keyboard.type` 不记录输入全文，只记录字符数和字节数。
- upload/download 只记录远端路径 basename、传输大小和 SHA-256，不记录完整路径或文件内容。
- `token`、`password`、`passwd`、`secret`、`key`、`*_key` 等疑似敏感赋值会写成 `[redacted]`。
- TCP tunnel 只记录 listen/target 端点摘要、tunnel id 和 stream id；不记录 TCP payload。

被控端控制台必须同步显示审计摘要，不依赖日志文件打开成功。即使本地日志写入失败，也要继续把远控操作实时显示在控制台。

被控端 `Host ID` 是进程运行期随机值，不写入磁盘。启动窗口和剪贴板连接信息会显示当前 `Host ID`；进程重启后该值会变化。

## 版本信息

当前 `rcwctl` 和 `rcw-host.exe` 通过 clap 支持：

```bash
--version
```

输出当前 crate 版本号，例如 `rcwctl 0.1.11`。`rcw-server` 当前没有独立 `--version` 参数；服务启动后可通过 `/healthz` 查看服务名和 `protocol_version`。

构建命令、目标平台和发布包结构见 [release.md](release.md)。权限状态、剪贴板和电源请求的 Windows 侧实现约束见 [windows-apis.md](windows-apis.md)。
