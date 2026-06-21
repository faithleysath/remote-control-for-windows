# 调试工作流

本文是 `rcw` 仓库内关于“调试”的单一真相源。它只记录已经在当前开发环境中实跑验证过的主路径，不重复产品范围、发布策略或完整测试矩阵。

“调试”在本项目里指开发者或 agent 为解决某个 issue 做的定向运行、观察、复现和验证；“测试”则是更可重复的脚本化或标准化质量检查。测试分层和 CI 边界见后续测试文档；本文只聚焦真实可用的调试链路。

## 适用范围

- Linux 开发机源码运行 `rcw-server` 和 `rcwctl`
- 本机 `win11-main` Windows VM 运行 `rcw-host.exe` 或 `rcw-host-gui.exe`
- CLI、MCP、console host、GUI host 的最小闭环调试
- 当前仓库已验证的 Windows 交互桌面链路

本文不覆盖：

- 完整 E2E 回归矩阵
- CI 里可以稳定自动化的 smoke / unit 组合
- 发布前完整人工验证清单
- 生产 `mcp__rcw_zhang` 服务的行为

`AGENTS.md` 已明确：当前可见的 `mcp__rcw_zhang` 服务来自独立的生产构建，不能作为本仓库当前代码行为的证据。

## 当前环境基线

这条调试路径基于以下当前已验证环境事实：

- Linux 主机通过 libvirt 运行本地 Windows VM `win11-main`
- 控制 guest 的首选路径是 `ssh win11-main`
- `win11-main` 固定在 libvirt `default` 网络 `192.168.122.107`
- guest 访问 Linux 主机服务使用 `192.168.122.1`
- 宿主共享目录 `/data/libvirt/work/share` 已通过 virtiofs 暴露给 guest，Windows 侧约定盘符为 `Z:`

如果这些环境事实已经漂移，先回到全局环境技能：

- `ops-libvirt-vm-platform`
- `ops-remote-host-ssh`

## 主原则

### 1. 不要把 `SessionId=0` 当成桌面调试路径

从 `ssh win11-main` 里直接启动 `rcw-host.exe` 或 `rcw-host-gui.exe`，进程通常会落在 `SessionId=0`。这一条路径可以证明：

- host 能启动
- host 能连接 server
- CLI/MCP 协议本身能工作

但它不能证明桌面相关能力是可用的。

当前已验证现象：

- `exec` 可以成功
- `windows` 可能返回空数组
- `screenshot` 可能失败并报 `BitBlt failed: 句柄无效。 (0x80070006)`

所以只要目标是验证窗口、截图、输入、GUI 生命周期或 WebView2/CDP，就必须把 host 投递进真实的交互桌面会话。

### 2. 桌面相关调试统一走 `SessionId=1`

当前已验证的可用路径是：

- 在 guest 内注册 `LogonType=Interactive`、`RunLevel=Highest` 的计划任务
- 计划任务先启动 PowerShell runner 脚本
- runner 再在 `SessionId=1` 中拉起 `rcw-host.exe` 或 `rcw-host-gui.exe`

仓库里收编的已验证脚本位于：

- [scripts/debug/windows/launch-session-report.ps1](/home/laysath/Projects/remote-control-for-windows/scripts/debug/windows/launch-session-report.ps1)
- [scripts/debug/windows/launch-rcw-host-interactive.ps1](/home/laysath/Projects/remote-control-for-windows/scripts/debug/windows/launch-rcw-host-interactive.ps1)
- [scripts/debug/windows/launch-rcw-host-gui-interactive.ps1](/home/laysath/Projects/remote-control-for-windows/scripts/debug/windows/launch-rcw-host-gui-interactive.ps1)

### 3. 不要在 SSH 命令行里手搓复杂 PowerShell

已经实踩过的坑包括：

- Bash 抢 `$PID`
- Bash 抢 `$_`
- 多层 quoting 污染
- `pwsh -File ...; other command` 被 `-File` 参数吞掉

因此 guest 侧复杂动作的默认策略是：

1. 先把脚本落仓库
2. 再同步到共享盘 `Z:`
3. 通过 `ssh win11-main "pwsh -File Z:\\...ps1"` 触发

### 4. 调试脚本必须自动留下证据

Windows 侧失败排查成本很高。当前主路径统一要求脚本自动落下：

- report
- stdout
- stderr
- audit

没有这些文件时，不要宣称某条链路“已经跑通”。

## 预检查

开始任何一轮调试前，先确认：

```bash
ssh -o BatchMode=yes win11-main 'hostname'
ssh -o BatchMode=yes win11-main 'pwsh -NoProfile -Command "Get-PSDrive Z | Format-List Name,Root,Used,Free"'
virsh -c qemu:///system domstate win11-main
```

如果要验证桌面态行为，再确认 guest 当前有人类桌面会话存在。最小检查可以先跑 session 报告脚本：

```bash
cp -f scripts/debug/windows/launch-session-report.ps1 /data/libvirt/work/share/
ssh -o BatchMode=yes win11-main 'pwsh -NoProfile -ExecutionPolicy Bypass -File Z:\launch-session-report.ps1'
```

成功时，`Z:\session-launch-report.txt` 至少应包含：

- `whoami=...`
- `sessionId=1`

## Linux 侧基线

### 本机启动 server

推荐显式带上调试期 token 和审计文件：

```bash
env RCW_BIND_ADDR=0.0.0.0:17800 \
    RCW_CONTROL_TOKEN=debug-token-rcw \
    RCW_AUDIT_LOG=/tmp/rcw-server-debug-audit.jsonl \
    cargo run -p rcw-server
```

### 构建 console host

```bash
RUSTFLAGS='-C target-feature=+crt-static' \
  cargo xwin build -p rcw-host --target x86_64-pc-windows-msvc --release
```

产物：

```text
target/x86_64-pc-windows-msvc/release/rcw-host.exe
```

### 构建 GUI host

```bash
npm --prefix crates/rcw-host-gui run tauri:build:windows:x64
```

产物：

```text
target/x86_64-pc-windows-msvc/release/rcw-host-gui.exe
```

### 复制产物和脚本到共享盘

当前调试文档默认使用以下 guest 侧文件名：

```bash
cp -f target/x86_64-pc-windows-msvc/release/rcw-host.exe \
  /data/libvirt/work/share/rcw-host-debug.exe
cp -f target/x86_64-pc-windows-msvc/release/rcw-host-gui.exe \
  /data/libvirt/work/share/rcw-host-gui-debug.exe
cp -f scripts/debug/windows/launch-session-report.ps1 \
  /data/libvirt/work/share/
cp -f scripts/debug/windows/launch-rcw-host-interactive.ps1 \
  /data/libvirt/work/share/
cp -f scripts/debug/windows/launch-rcw-host-gui-interactive.ps1 \
  /data/libvirt/work/share/
```

如果脚本或产物文件名发生变化，应同时更新本文和 skill 路由说明。

## Console Host 主路径

### 启动

将脚本同步到 `Z:` 后，在 guest 中触发：

```bash
ssh -o BatchMode=yes win11-main \
  'pwsh -NoProfile -ExecutionPolicy Bypass -File Z:\launch-rcw-host-interactive.ps1'
```

当前脚本约定：

- 任务名：`Codex-Launch-RcwHost-Interactive`
- 计划任务 principal：`jgtty + Interactive + Highest`
- guest 二进制：`Z:\rcw-host-debug.exe`
- server：`ws://192.168.122.1:17800`

### 证据文件

脚本成功运行后，至少会写出：

- `Z:\rcw-host-session1-report.txt`
- `Z:\rcw-host-session1.stdout.log`
- `Z:\rcw-host-session1.stderr.log`
- `Z:\rcw-host-session1-audit.jsonl`

`report` 中至少应能确认：

- `runnerSessionId=1`
- `childSessionId=1`
- `childName=rcw-host-debug.exe`

`stdout` 中应能重新采样运行期值：

- `Machine ID`
- `Host ID`
- `Current TOTP`

注意：`host_id` 和 `totp` 都是运行期值。每次重启 host 后都必须重新采样，不能沿用上一轮值。

### CLI 最小验收

使用新的 `machine_id` / `host_id` / `totp` 后，最小 CLI 闭环是：

```bash
export RCW_SERVER_URL=ws://127.0.0.1:17800
export RCW_CONTROL_TOKEN=debug-token-rcw

rcwctl connect --id <machine-id> --host-id <host-id> --totp <totp>
rcwctl status
rcwctl windows
rcwctl screenshot --output /tmp/rcw-session1-host.png
rcwctl exec -- pwsh -NoProfile -Command "hostname"
```

判定标准：

- `status` 成功
- `windows` 返回真实窗口列表，而不是空数组
- `screenshot` 成功写出 PNG
- `exec` 成功返回结果或明确 task 状态

验证完成后主动断开：

```bash
rcwctl disconnect
```

### 输入链路注意事项

输入类测试当前已经证明可用，但存在一个非常现实的限制：输入会发往当前焦点窗口，而不是你“以为”的目标窗口。

所以在执行：

```bash
rcwctl keyboard-type "hello from rcw session1"
rcwctl keyboard-key Enter
```

之前，必须先清场并确认焦点窗口。否则只能证明“输入成功”，不能证明“目标窗口收到了输入”。

## MCP 主路径

### 什么时候用手写 JSON-RPC

手写 stdio JSON-RPC 适合：

- 最小排障
- 确认 `rcwctl mcp` 本身能起
- 快速验证 `connect` / `status` / `exec` / `windows` / `screenshot`

不适合：

- 长期回归
- 复杂断言
- 稳定自动化测试

原因是当前 stdio 体验已知有这些特性：

- 输入会回显到 stdout
- 请求和响应混在同一条流
- 成功结果同时出现在 `content[].text` 和 `structuredContent`
- 错误结果也会以文本形式出现，肉眼容易误判

正式测试建议后续用 MCP SDK 写脚本；但最小兜底路径仍应保留。

### 启动 MCP

```bash
RCW_SERVER_URL=ws://127.0.0.1:17800 \
RCW_CONTROL_TOKEN=debug-token-rcw \
cargo run -p rcwctl -- mcp
```

### 最小请求序列

当前已验证过的最小请求顺序是：

1. `initialize`
2. `tools/list`
3. `tools/call(connect)`
4. `tools/call(status)` 或 `tools/call(exec)`
5. `tools/call(windows)` / `tools/call(screenshot)`（需要 host 已经在 `SessionId=1`）

`connect` 的参数字段名是：

```json
{
  "machine_id": "<machine-id>",
  "host_id": "<host-id>",
  "totp": "<totp>"
}
```

一个最小 `connect` 请求示例：

```json
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"connect","arguments":{"machine_id":"<machine-id>","host_id":"<host-id>","totp":"<totp>"}}}
```

一个最小 `exec` 请求示例：

```json
{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"exec","arguments":{"program":"pwsh","argv":["-NoProfile","-Command","hostname"]}}}
```

如果要做桌面态闭环，建议继续补：

```json
{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"windows","arguments":{}}}
{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"screenshot","arguments":{"output_path":"/tmp/rcw-mcp-session1-host.png"}}}
```

## GUI Host 主路径

### 已验证前提

当前 GUI 路径已经实跑确认，但它有两个必须先知道的前提：

1. `rcw-host.exe` 和 `rcw-host-gui.exe` 在 Windows 上共用同一个全局单例锁
2. GUI 调试前必须先清掉所有 `rcw-host*` 进程

对应代码位于：

- [crates/rcw-host-core/src/identity.rs](/home/laysath/Projects/remote-control-for-windows/crates/rcw-host-core/src/identity.rs:67)

单例名是：

```text
Global\RemoteControlForWindowsHost
```

### 启动

将脚本同步到 `Z:` 后，在 guest 中触发：

```bash
ssh -o BatchMode=yes win11-main \
  'pwsh -NoProfile -ExecutionPolicy Bypass -File Z:\launch-rcw-host-gui-interactive.ps1'
```

当前脚本约定：

- 任务名：`Codex-Launch-RcwHostGui-Interactive`
- guest 二进制：`Z:\rcw-host-gui-debug.exe`
- server：`ws://192.168.122.1:17800`
- WebView2 CDP 端口：`9222`

### 证据文件

脚本成功运行后，至少会写出：

- `Z:\rcw-host-gui-session1-report.txt`
- `Z:\rcw-host-gui-session1.stdout.log`
- `Z:\rcw-host-gui-session1.stderr.log`
- `Z:\rcw-host-gui-session1-audit.jsonl`

成功时应至少能在证据中确认：

- `rcw-host-gui-debug.exe` 存活于 `SessionId=1`
- `9222` 已监听
- `stdout` 中能看到连接相关日志
- `stderr` 为空或只有非致命信息

### 配置文件 BOM 注意事项

当前 GUI 路径已经踩过一个真实产品坑：

- 如果 `host-gui.json` 是带 UTF-8 BOM 的 JSON
- `rcw-host-gui` 会在 setup 阶段因为 `serde_json::from_slice` 直接报错

因此当前启动脚本已经显式用无 BOM UTF-8 写配置：

```powershell
[System.IO.File]::WriteAllText(..., [System.Text.UTF8Encoding]::new($false))
```

相关产品 bug 见 Issue `#34`。

### CDP 验收

GUI 调试不能只看“端口开了”。当前已验证的最小闭环是：

1. 从 Linux 开发机转发 guest 的 `9222`
2. 验证 `/json/version`
3. 验证 `/json/list`
4. 至少成功执行一次 page WebSocket `Runtime.evaluate`

建立本地转发：

```bash
ssh -o BatchMode=yes -o ExitOnForwardFailure=yes \
  -N -L 127.0.0.1:9223:127.0.0.1:9222 win11-main
```

验证 CDP HTTP 入口：

```bash
curl -fsS http://127.0.0.1:9223/json/version
curl -fsS http://127.0.0.1:9223/json/list
```

成功时，`/json/list` 应该能看到：

- title: `Remote Control Host`
- url: `http://tauri.localhost/`
- `webSocketDebuggerUrl`

最小 page WebSocket 验收是对该 `webSocketDebuggerUrl` 发一次 `Runtime.evaluate`，确认至少能拿到页面标题、URL 或 `document.readyState`。

如果只验证到“端口已监听”，还不能算 GUI/CDP 链路跑通。

### GUI 自动化接口

为避免从页面文案里硬抠 `machine_id` / `host_id` / `totp`，GUI 现在额外暴露了一个稳定的页面自动化接口：

```js
window.__RCW_HOST_GUI_DEBUG__
```

当前接口版本：

```js
window.__RCW_HOST_GUI_DEBUG__.version === 1
```

当前可调用方法：

- `getConnectionInfo()`
- `getSnapshot()`

其中 `getConnectionInfo()` 返回的就是结构化 JSON，可直接给 CDP 自动化、调试脚本或后续业务 E2E 使用：

```json
{
  "server_url": "ws://192.168.122.1:17800",
  "machine_id": "ABCD-EFGH-IJKL",
  "host_id": "host_xxx",
  "totp": "123456",
  "totp_period_seconds": 120,
  "totp_remaining_seconds": 97,
  "listener_status": "connected",
  "audit_path": "C:\\Users\\...\\audit.jsonl"
}
```

最小 `Runtime.evaluate` 示例：

```json
{"id":2,"method":"Runtime.evaluate","params":{"expression":"(async()=>await window.__RCW_HOST_GUI_DEBUG__.getConnectionInfo())()","awaitPromise":true,"returnByValue":true}}
```

这条接口的定位是“自动化/调试专用稳定入口”。后续如果页面 UI 文案、排版或 DOM 结构变化，调试和测试不应再依赖从网页可见文本里解析连接信息。

## 证据要求

在 issue 或 PR 中引用本地调试结果时，至少保留这些信息中的一部分：

- 本次使用的是 console 还是 GUI 路径
- host 是否处于 `SessionId=1`
- 使用的 `machine_id` / `host_id` / `totp` 是否来自本轮重新采样
- `windows` / `screenshot` / `exec` / `Runtime.evaluate` 哪些通过
- 证据文件路径
- 如有截图，给出 Linux 侧落图路径

不要只写“本地已验证通过”。

## 常见坑

### 1. 复用旧的 `host_id` / `totp`

症状：

- `HostNotFound`
- `InvalidTotp`
- `HostBusy`

原因：

- host 重启后运行期值变了

处理：

- 回到最新的 stdout / report 重新采样

### 2. 把 session 0 误当成桌面态

症状：

- `windows=[]`
- `screenshot` 句柄无效

原因：

- host 并不在交互会话

处理：

- 改用本文的 interactive scheduled task 主路径

### 3. GUI 没起来但没有 stderr

原因通常是：

- 脚本没有重定向 stderr
- 配置文件写入方式不稳

处理：

- 使用仓库脚本，而不是手工临时拼命令

### 4. GUI 和 console host 同时运行

症状：

- GUI 启动失败
- 或 console / GUI 互相抢占

原因：

- 共用全局单例锁

处理：

- GUI 调试前清掉全部 `rcw-host*`

## 与 skill 的边界

本文和 `scripts/debug/windows/*.ps1` 是仓库内长期事实源。

后续的 `rcw-debug-workflow` skill 只负责：

- 入口检查
- 告诉 agent 应该选 console / MCP / GUI 哪条路径
- 指向本文和对应脚本
- 提醒需要调用的全局环境技能

skill 不应复制本文的大段步骤，也不应维护第二份调试事实。
