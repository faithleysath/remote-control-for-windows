# v0.1.6 E2E 测试报告

本文记录 `remote-control-for-windows` `0.1.6` 的一次手工端到端验证。该报告用于确认协议 v4 的 host identity/routing、单 host 单 session、同 session 并发、后台 exec、取消和 MCP 文件传输等能力在真实 Windows 交互桌面中可用。

## 结论

`0.1.6` 主链路和本版本新增的关键机制验证通过，可以作为当前可用基线。

未做全参数矩阵穷尽测试。剩余未覆盖项列在本文末尾，后续如有真实用户反馈再按场景修复。

## 测试环境

- 日期：2026-06-14
- 版本：`0.1.6`
- 代码提交：`6aaffa7`
- server：`zhang` 上的 `rcw-server v0.1.6`
- server 地址：`http://106.14.176.184:51234` / `ws://106.14.176.184:51234`
- server 健康检查：`{"ok":true,"protocol_version":4,"service":"rcw-server"}`
- Windows VM：`win11-data`，Windows-in-Docker，交互桌面
- host 包：`rcw-host-zhang-x86_64-pc-windows-msvc-v0.1.6.zip`
- host 内置地址：`ws://106.14.176.184:51234`
- host exe SHA-256：`3855dcbb2fd65adb53b2ddb60561c43bacce8459a8145dd6359e47d358544ad3`
- 控制端：`rcwctl@0.1.6`，Codex MCP `rcw-zhang`

测试时 host 在管理员 elevated 交互桌面中运行。测试结束后已断开会话并停止 host 进程。

## 验证摘要

### Host 启动和身份

已验证：

- `rcw-host.exe` 可直接使用内置 zhang server 地址启动。
- host 控制台显示 `Version: 0.1.6`、server、machine ID、Host ID、TOTP 周期和权限状态。
- host 成功连接 server，server hello acknowledged。
- `host_id` 可由控制台和剪贴板获取，并可用于控制端精确连接。
- host 重启后 `host_id` 变化，符合运行期内存 ID、不持久化的设计。
- 同一 Windows 桌面启动第二个 host 实例失败，错误为 `another rcw-host instance is already running on this machine`。

### 会话和鉴权

已验证：

- MCP `connect` 使用 `machine_id + host_id + totp` 成功建会话。
- CLI `connect --host-id` 成功建会话。
- `status` 返回 `host_online=true` 和 `session_active=true`。
- 错误控制端 token 返回 `InvalidToken`。
- 错误 TOTP 返回 `InvalidTotp`。
- 同一 host 已有 active session 时，普通 connect 返回 `HostBusy`。
- `connect --force` / MCP `force_reconnect=true` 可以替换旧会话。
- 被替换的旧 MCP session 后续 `status` 返回 `SessionExpired`。
- CLI `disconnect` 成功关闭 session 并删除本地 session 文件。
- MCP `disconnect` 成功关闭当前内存 session。

### 命令执行

已验证：

- MCP `exec` 返回 stdout、stderr、exit code、耗时和 request id。
- CLI `exec` 可在 force reconnect 后正常执行。
- 同一 MCP session 内多条 `exec` 并发执行并按 request id 正确完成。
- MCP `exec.wait_ms=0` 返回 server-owned `task_id`。
- MCP `exec_status` 可查询后台任务最终 `completed`。
- CLI `exec --wait 0` 返回 server-owned `task_id`。
- 新 CLI 进程可用 `exec-status` 查询后台任务最终 `completed`。
- MCP `exec_cancel` 可取消长时间任务，最终状态为 `cancelled`。
- `exec` 远端超时返回 `request_timeout`，后续检查未发现同特征残留 PowerShell 进程。

### 文件传输

已验证：

- MCP `upload` 可上传文件到 Windows host。
- `upload overwrite=true` 可覆盖目标文件。
- `upload overwrite=false` 遇到已有目标文件时拒绝覆盖，返回 `InvalidPath`。
- MCP `download` 可下载远端文件。
- 4 MiB 文件 upload/download 后本地 `sha256sum` 和 `cmp` 均一致。
- MCP `upload wait_ms=0` 返回进程内 transfer task id。
- MCP `download wait_ms=0` 返回进程内 transfer task id。
- `transfer_status` 可查询后台 upload/download 最终 `completed`。
- MCP `transfer_cancel` 可取消后台 upload，最终状态为 `cancelled`。

### GUI 和桌面操作

已验证：

- MCP `screenshot` 在 `win11-data` 当前 100% 缩放环境返回有效 PNG，尺寸 `1280x720`，非空。
- MCP `windows` 返回可见窗口列表，包含 focused、handle、title、process_id、rect、visible。
- `keyboard_type`、`keyboard_key` 可向焦点 Notepad 输入文本和换行。
- 截图确认文本实际进入 Notepad 窗口。
- `mouse_move`、`mouse_click`、`mouse_scroll` 均返回成功。

针对两个已知 GUI issue 的复测结论：

- [#1](https://github.com/faithleysath/remote-control-for-windows/issues/1)：`keyboard_key` 发送 `Control+End` 在 `0.1.6` 上可复现失败，返回 `CommandFailed: unsupported key: end`。同一轮复测中，`keyboard_type` 输入 `typed-through hyphen-test alpha-beta gamma-delta` 后截图和文件读回均正确，未复现连字符变形。
- [#6](https://github.com/faithleysath/remote-control-for-windows/issues/6)：`win11-data` 当前交互桌面为 100% 缩放，DPI 探针返回 `LogPixels=96x96`、`Scale=1`、`SystemMetrics=1280x720`、`DesktopRes=1280x720`，rcw screenshot 同为 `1280x720`，因此本轮环境未复现 150% 缩放裁剪。该 issue 仍保留为基于 150% 缩放实机证据的已知缺口。

### 剪贴板、安全和电源

已验证：

- host 启动后复制连接信息到剪贴板。
- 剪贴板包含 server、machine ID、Host ID、TOTP 和有效期。
- 剪贴板不包含控制端 token、session token、TOTP seed 或原始机器标识。
- host 控制台日志记录 request id，可与控制端操作对齐。
- `powercfg /requests` 显示 host 进程持有 DISPLAY 和 SYSTEM 请求。
- host 停止后测试环境中未保留 rcw host 进程。

## 证据文件

本次验证过程中生成的本机临时证据包括：

- `/tmp/rcw-v016-screenshot.png`
- `/tmp/rcw-v016-after-input.png`
- `/tmp/rcw-v016-host.log`
- `/tmp/rcw-v016-large.bin`
- `/tmp/rcw-v016-large-downloaded.bin`

这些文件用于本次人工确认，不作为仓库长期测试 fixture。

## 未覆盖项

本次未覆盖：

- 标准用户权限下启动 host，只验证了管理员 elevated 桌面。
- 锁屏、UAC 安全桌面和非交互 session 0。
- 150% 或其他非 100% DPI 缩放环境下的 screenshot/鼠标坐标一致性。
- TOTP 周期不一致错误。
- download transfer cancel 的单独路径。
- 人为构造 checksum mismatch 或损坏传输。
- 鼠标右键、中键和全部 keyboard key 名称。已知 `Control+End` 在本轮复测中仍不支持。
- server 后台长期 idle cleanup。
- 真实或模拟短 `machine_id` 冲突。

这些是后续增强验证项，不阻塞 `0.1.6` 作为当前可用基线。
