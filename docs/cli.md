# CLI 设计

控制端二进制名为 `rcwctl`。它是研发和 Codex 的主要入口。

## 安装

普通用户推荐通过 npm 安装预编译二进制：

```bash
npm install -g rcwctl
rcwctl --version
```

npm 元包会按平台自动安装对应的二进制包，二进制直接随 npm tarball 发布。用户不需要安装 Rust 编译环境，国内 npm 镜像也能直接分发这条链路。

开发者也可以从源码运行：

```bash
cargo run -p rcwctl -- --version
```

## 全局参数

```bash
rcwctl [GLOBAL_OPTIONS] <COMMAND>
```

全局参数：

- `--server <url>`：覆盖服务器地址。
- `--token <token>`：覆盖控制端 token。默认读取 `RCW_CONTROL_TOKEN`。
- `--session <path>`：指定本地会话文件。
- `--json`：输出 JSON。
- `--timeout <duration>`：控制端等待超时。
- `--audit-label <text>`：为本次操作写入一段简短审计说明。
- `-v, --verbose`：输出调试信息。

## 配置优先级

服务器地址：

1. `--server`
2. `RCW_SERVER_URL`
3. 编译期嵌入的 `RCW_EMBED_SERVER_URL`

控制端 token：

1. `--token`
2. `RCW_CONTROL_TOKEN`

控制端 token 不允许编译进默认二进制。

## 本地审计日志

`rcwctl` 每次执行会话操作都必须写入本地审计日志，记录：

- 时间。
- 服务器地址。
- 机器 ID。
- session ID。
- request ID。
- 子命令和参数摘要。
- `--audit-label`。
- 结果和耗时。

日志不记录完整 token、session token、TOTP seed 或文件内容。

## connect

建立会话。

```bash
rcwctl connect --id 8K4F-2M7Q --totp 183942
```

成功后写入本地会话文件。
同时写入控制端审计日志。

JSON 输出：

```json
{
  "ok": true,
  "session_id": "01HY...",
  "machine_id": "8K4F-2M7Q",
  "server": "wss://remote.example.com"
}
```

## status

查看当前会话状态。

```bash
rcwctl status
```

`status` 读取本地会话文件后向服务端发送 `session.status`，返回 session 是否有效、被控端是否在线、目标机器 ID 和服务器地址。

## exec

执行远程命令。

```bash
rcwctl exec -- pwsh -NoProfile -Command "Get-ComputerInfo"
rcwctl exec --timeout 120s -- cmd /c dir C:\
```

规则：

- `--` 后面的内容原样作为远程进程参数。
- 默认工作目录为被控端进程当前目录。
- 默认超时 30 秒。
- 返回远程退出码，但 CLI 本身在传输失败时返回自己的非 0 退出码。
- 审计日志记录命令参数摘要、退出码和耗时；不默认记录完整 stdout/stderr。

## upload

上传文件。

```bash
rcwctl upload ./tool.exe 'C:\Users\Public\tool.exe'
```

可选参数：

- `--overwrite`
- `--sha256 <hex>`

## download

下载文件。

```bash
rcwctl download 'C:\ProgramData\App\logs\app.log' ./app.log
```

## screenshot

截取当前屏幕。

```bash
rcwctl screenshot --output screen.png
```

可选参数：

- `--display <index>`
- `--format png`

当前实现支持 PNG 输出。

## windows

列出窗口。

```bash
rcwctl windows
rcwctl --json windows
```

输出字段：

- `handle`
- `title`
- `process_id`
- `rect`
- `visible`
- `focused`

## mouse

鼠标操作。

```bash
rcwctl mouse-move --x 400 --y 300
rcwctl mouse-click --x 400 --y 300 --button left
rcwctl mouse-scroll --delta -3
```

坐标默认使用屏幕绝对坐标。后续可以扩展为窗口相对坐标。

## keyboard

键盘操作。

```bash
rcwctl keyboard-type "hello world"
rcwctl keyboard-key Ctrl+L
rcwctl keyboard-key Enter
```

按键名称使用跨平台规范名，`rcw-host.exe` 负责映射到 Windows 虚拟键。

## disconnect

关闭当前会话。

```bash
rcwctl disconnect
```

关闭后删除本地会话文件。

## mcp

启动一个 stdio MCP 服务器。

```bash
rcwctl --server ws://127.0.0.1:7800 --token "$RCW_CONTROL_TOKEN" mcp
```

也可以通过 npm 元包交给 MCP 客户端按需拉起：

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

MCP 工具：

- `connect`：用机器 ID 和当前 TOTP 打开远控会话。
- `disconnect`：关闭当前远控会话。
- `status`：查询当前会话和被控端在线状态。
- `exec`：执行远程命令。
- `upload`：流式读取 MCP 服务器本机路径并上传到远端路径。可传 `wait_timeout_ms`，超时后返回后台 `task_id`。
- `download`：下载远端文件并流式写入 MCP 服务器本机路径。可传 `wait_timeout_ms`，超时后返回后台 `task_id`。
- `transfer_status`：查询后台上传或下载任务的运行状态、最终结果或错误。
- `screenshot`：截图并写入 MCP 服务器本机路径。
- `windows`：列出窗口。
- `mouse_move`、`mouse_click`、`mouse_scroll`：鼠标操作。
- `keyboard_type`、`keyboard_key`：键盘输入。

`mcp` 是长期运行进程。它把打开后的 session 和后台传输任务状态保存在本进程内存里，不读取也不写入普通 CLI 使用的本地 session 文件；因此不会污染 `~/.local/share/rcwctl/session.json`、`~/Library/Application Support/rcwctl/session.json` 或 `%APPDATA%\rcwctl\session.json`。MCP 文件工具只接收路径参数，文件主体使用 WebSocket binary frame 流式传输，不使用 base64 参数。`upload` / `download` 默认最多等待 60 秒；如果传输未完成，会返回 `status=running` 和 `task_id`，之后调用 `transfer_status` 查询。`wait_timeout_ms=0` 表示立即转后台。MCP 进程退出后，内存中的 session 信息和任务状态也随之丢失；需要重新调用 `connect`。

## Codex 调用约定

Codex 默认应使用：

```bash
rcwctl --json ...
```

需要 GUI 操作时，应按以下节奏：

1. `screenshot`
2. 本地分析截图
3. `click` 或 `type`
4. 再次 `screenshot` 验证

所有命令都必须避免进入无法退出的交互式程序。需要长时间命令时显式设置 `--timeout`。
必要时使用 `--audit-label` 写明本次操作目的，例如 `--audit-label "collect customer app logs"`。
