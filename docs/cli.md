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

- `--server <url>`：覆盖服务器地址，默认读取 `RCW_SERVER_URL` 或编译期嵌入值。
- `--token <token>`：覆盖控制端 token，默认读取 `RCW_CONTROL_TOKEN`。
- `--session <path>`：指定本地会话文件。MCP 模式把状态放在进程内存里，不使用这个文件。
- `--json`：输出 JSON。
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
rcwctl connect --id 8A4F-2B7C-91D0 --totp 183942
```

成功后写入本地会话文件。
同时写入控制端审计日志。
`--id` 是被控端显示的短 `machine_id`。如果短码冲突，被控端窗口和剪贴板连接信息还会显示当前运行期 `Host ID`，控制端可以额外传 `--host-id` 精确寻址：

```bash
rcwctl connect --id 8A4F-2B7C-91D0 --host-id host_QbYx... --totp 183942
```

`Host ID` 只用于选择目标在线 host，不替代 TOTP 或控制端 token。
如果服务端仍保留同一被控端的旧会话，而现场用户已经给出新的 TOTP，可以使用 `--force` 在 TOTP 验证通过后替换旧会话：

```bash
rcwctl connect --id 8A4F-2B7C-91D0 --totp 183942 --force
```

JSON 输出：

```json
{
  "ok": true,
  "session_id": "01HY...",
  "machine_id": "8A4F-2B7C-91D0",
  "host_id": "host_QbYx...",
  "server": "wss://remote.example.com"
}
```

## status

查看当前会话状态。

```bash
rcwctl status
```

`status` 读取本地会话文件后向服务端发送 `session.status`，返回 session 是否有效、被控端是否在线和目标机器 ID。

## exec

执行远程命令。

```bash
rcwctl exec -- pwsh -NoProfile -Command "Get-ComputerInfo"
rcwctl exec --timeout 120s -- cmd /c dir C:\
rcwctl exec --wait 0 -- pwsh -NoProfile -Command "Start-Sleep 30; 'done'"
rcwctl exec-status <task_id>
rcwctl exec-cancel <task_id>
```

规则：

- `--` 后面的内容原样作为远程进程参数。
- 默认工作目录为被控端进程当前目录。
- 不传 `exec --timeout` 时，远端进程默认最多运行 24 小时。
- 不传 `exec --wait` 时，CLI 默认等 90 秒；到期仍未完成时返回 server-owned `task_id` 和当前 running 状态。
- `exec --timeout <duration>` 是远端进程运行上限，不影响 CLI 本次等待窗口。
- `exec --wait <duration>` 只限制本次 CLI 调用等待多久，不限制远端进程运行。
- 如果 `--wait` 小于 `--timeout`，CLI 会先返回 running task，远端进程继续运行到完成、失败或远端 timeout。
- 如果 `--wait` 大于 `--timeout`，host 会先按远端 timeout 结束进程，CLI 在等待窗口内看到最终 timeout/error 状态后返回。
- `exec --wait 0` 会在 server 上创建 exec job 后立即返回 `task_id`；后续短 CLI 进程可用 `exec-status` 查询 stdout、stderr、退出码和最终状态，或用 `exec-cancel` 请求 host 杀掉对应进程树。
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

- `connect`：用机器 ID 和当前 TOTP 打开远控会话；可传 `force_reconnect=true` 在 TOTP 验证通过后替换旧会话。
- `disconnect`：关闭当前远控会话。
- `status`：查询当前会话和被控端在线状态。
- `exec`：执行远程命令。可传 `timeout_ms` 限制远端进程运行时长，默认 24 小时；可传 `wait_ms` 限制本次 MCP tool call 等待多久，`0` 表示立即返回 server-owned `task_id`。
- `exec_status`：查询 server-owned exec job 状态、最终 stdout/stderr/exit_code 或错误；stdout 和 stderr 各最多保留 1 MiB，超出时通过 `stdout_truncated` / `stderr_truncated` 标记。
- `exec_cancel`：请求取消 server-owned exec job，并要求被控端杀掉对应远端进程；只有收到 server 的 `command.cancel_result` 后才查询并返回当前 job 状态，最终状态用 `exec_status` 查询。
- `upload`：流式读取 MCP 服务器本机路径并上传到远端路径。可传 `wait_ms`，等待窗口结束后返回 MCP 进程内 `task_id`。
- `download`：下载远端文件并流式写入 MCP 服务器本机路径。可传 `wait_ms`，等待窗口结束后返回 MCP 进程内 `task_id`。
- `transfer_status`：查询 MCP 进程内上传或下载任务的运行状态、最终结果或错误。
- `transfer_cancel`：取消 MCP 进程内上传或下载任务；上传如果还在 MCP 本机 hash/校验阶段会直接本地取消，已经发到远端后会先等待 server 确认取消消息已投递，再中止本地任务并通知 host 丢弃临时上传状态；下载会通知 host 停止远端分块发送，同时中止本地写入并清理本地临时文件。
- `screenshot`：截图并写入 MCP 服务器本机路径。
- `windows`：列出窗口。
- `mouse_move`、`mouse_click`、`mouse_scroll`：鼠标操作。
- `keyboard_type`、`keyboard_key`：键盘输入。

`mcp` 是长期运行进程。它把打开后的 session 和传输任务状态保存在本进程内存里，不读取也不写入普通 CLI 使用的本地 session 文件；因此不会污染 `~/.local/share/rcwctl/session.json`、`~/Library/Application Support/rcwctl/session.json` 或 `%APPDATA%\rcwctl\session.json`。MCP 文件工具只接收路径参数，文件主体使用 WebSocket binary frame 流式传输，不使用 base64 参数。`exec` 默认远端运行上限为 24 小时，默认最多等待 90 秒；如果未完成，会返回 server-owned `task_id`，之后调用 `exec_status` 查询。`upload` / `download` 默认最多等待 60 秒；如果传输未完成，会返回 MCP 进程内 `task_id`，之后调用 `transfer_status` 查询。`wait_ms=0` 表示立即返回任务状态。exec job 可以被后续 CLI 或 MCP 查询；upload/download 依赖当前 MCP 进程继续读写本地文件，不支持无 daemon 的 CLI detached transfer，也不在 MCP 进程退出后保留。已发到远端的取消请求只有在 server 返回 `command.cancel_result` 后，控制端才确认取消已投递；exec 最终状态由 host 的后续回包决定，transfer 的本地进程内状态在确认投递后由 MCP 进程本地标记为 cancelled。如果 server 未确认已发到远端的 transfer 取消或返回 `error`，MCP 会返回错误，并且不会进入本地 abort/清理分支；还没发到远端的本地预处理阶段可以直接本地取消。MCP 进程正常退出时会尽力关闭当前 session；如果进程崩溃或被强杀，内存中的 session 信息和传输任务状态会丢失，后续需要重新调用 `connect`，必要时使用 `force_reconnect=true`。

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

所有命令都必须避免进入无法退出的交互式程序。需要长时间命令时，对 CLI 使用 `exec --timeout` 和 `exec --wait`，对 MCP 使用 `exec.timeout_ms` 和 `exec.wait_ms` 表达远端运行上限与本次调用等待窗口。
必要时使用 `--audit-label` 写明本次操作目的，例如 `--audit-label "collect customer app logs"`。
