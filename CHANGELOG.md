# 变更日志

本文件记录项目的重要变更。

## 未发布

- GUI 新增 Exec 任务 tab，展示 exec 任务列表、脱敏参数/CWD 摘要、状态、耗时、exit code、stdout/stderr 字节统计和错误摘要，并支持复制 request/session id 与取消 running exec 任务。
- `HostSnapshot` 的 exec 任务可观测字段补齐，但仍不在 host GUI snapshot 中缓存 stdout/stderr 原文；Tauri 权限只新增窄 `host_cancel_exec_task` command。

## 0.1.9 - 2026-06-16

- 推进 GUI Host 转型 epic 的 #12-#18：抽出 `rcw-host-core` 运行时，并保持控制台 `rcw-host` 入口兼容，让 CLI host 和 GUI host 共享连接循环、命令执行、上传下载、隧道、平台 API、单实例锁和审计逻辑。
- 新增 `HostSnapshot` / `HostEvent` 状态模型和事件订阅入口，记录 listener、TOTP、session、auth、command、transfer、tunnel、错误和最近事件历史，供 CLI、GUI 和 audit 共用。
- 暴露 host 生命周期控制 API，支持启动、停止、重启 listener、使用新配置重启、结束当前 session、取消任务和关闭 tunnel；新增 `host.session_close` / `host.session_close_result`，协议版本提升到 v6。
- 升级 host 结构化审计和脱敏策略，覆盖 session、exec、input、screenshot、windows、upload/download 和 tunnel 事件；上传审计保留 controller 传入的 `audit_label`。
- 新增 `rcw-host-gui` Tauri v2 工程和最小安全权限基线，只开放窄 Tauri command 与 `host-event` 事件通道，不启用 shell/fs 插件或任意文件系统 scope。
- 实现 GUI 概览、会话、审计和设置页 MVP，支持复制连接信息、启动/停止/重连 listener、结束会话、保存 GUI 设置、查看/过滤运行期事件时间线、复制 audit path 和显示 audit 文件位置；独立 exec/transfer/tunnel 管理 tab、托盘和安装包仍在后续 #19-#24。
- CI 安装 Tauri Linux 依赖并检查 GUI 前端构建；发布 workflow 增加 `rcw-host-gui` Windows x86-64/arm64 artifact，GUI Windows 构建命令固化为 package script。
- 发布 workflow 的 GUI Windows 构建显式安装 `llvm-rc`，避免 Tauri Windows resource 生成在干净 GitHub runner 上失败；GUI/WebView2 构建不启用 `crt-static`，以匹配 Tauri/WebView2 的 MSVC CRT 链接模型。
- 修复 MCP 本地 tunnel listener 清理，关闭 forward 时释放本地监听端口。

## 0.1.8 - 2026-06-15

- 升级 wire protocol 到 v5，新增 `tunnel.open` / `tunnel.status` / `tunnel.close` 和 `tunnel.stream_*` 控制消息，以及独立 `TunnelData` binary frame。
- 新增 `rcwctl forward` 常驻命令，支持重复声明 `-L listen=target` 正向转发和 `-R listen=target` 反向转发。
- 新增 MCP `tunnel_open`、`tunnel_status`、`tunnel_close` 工具，MCP 进程内持有 listener、stream pump 和 tunnel manager 状态。
- 服务端增加 tunnel/session/stream 路由表、loopback 默认安全校验、并发上限、空闲清理和关闭传播。
- host 和 controller 两端增加 TCP listener/connect、stream pump、EOF/reset 传播和字节计数。
- 在本地 Linux smoke 中验证正向 `-L` 与反向 `-R` TCP echo tunnel；本轮尚未刷新真实 Windows host E2E。

## 0.1.7 - 2026-06-14

- Windows host 启动早期设置 per-monitor DPI awareness，修复 125% 缩放环境下 screenshot 使用逻辑尺寸导致右侧和底部被裁剪的问题。
- 扩展 `keyboard_key` 的导航键映射，支持 `End`、`Home`、`PageUp`、`PageDown` 和 `Insert`，修复 `Control+End` 返回 `unsupported key: end`。
- 在 `win11-data` 1920x1080 / 125% 缩放环境下复测 screenshot、`Control+End` 和 MCP 鼠标坐标靶场；协议版本保持 v4 不变，鼠标操控实现未修改。

## 0.1.6 - 2026-06-14

- 升级 wire protocol 到 v4，引入运行期 `host_id` 和 host `connection_id`，服务端按 `host_id` 路由在线 host、session、request 和后台 exec job。
- 将展示用短 `machine_id` 扩展为 `XXXX-XXXX-XXXX`，并只作为人工输入短码和冲突检测索引使用，不再作为服务端内部唯一主键。
- `rcwctl connect` 和 MCP `connect` 增加可选 `host_id`，短 `machine_id` 冲突时可以用被控端窗口或剪贴板里的当前 Host ID 精确寻址。
- 被控端启动时生成进程运行期 Host ID，不写入磁盘；同一进程内断线重连复用该 Host ID，进程重启后重新生成，避免克隆机器复制持久 Host ID 后互相替换。
- 被控端启动初期增加单实例锁，同一物理机同时只能运行一个 `rcw-host` 实例。
- 保持一个 host 同时只有一个 active session，但同一 session 内允许多个命令按 `request_id` 并行执行，exec/download 的 WebSocket 写锁缩短到单次发送粒度。
- 审计事件补充 `host_id` 字段，服务端在 host 重连、旧连接迟到回包和 session 清理路径上校验 `host_id`/`connection_id`，避免旧连接串包到新会话。

## 0.1.5 - 2026-06-14

- 升级 wire protocol 到 v3，新增 `command.start` / `command.status`，让 CLI 和 MCP 的长时间 `exec` 都使用 server-owned 后台任务。
- 增加 `rcwctl exec-status` 和 `rcwctl exec-cancel`，支持短 CLI 进程后续查询或取消后台 exec。
- MCP `exec` 支持 `wait_ms`，未完成时返回 server-owned `task_id`；`exec_status` 和 `exec_cancel` 可复用同一任务。
- 后台 exec 的 stdout/stderr 在 server 侧各最多保留 1 MiB，并通过 truncated 标记暴露截断。
- 取消路径改为等待 server 返回 `command.cancel_result` 后再确认已投递到 host，避免把本地发送成功误当作远端接受。
- MCP 上传/下载后台任务继续保存在 MCP 进程内，和 server-owned exec job 明确分离。
- 修复三端 JSONL 审计在同进程并发写入时可能出现的行粘连问题，保证每条审计事件都是独立可解析 JSON 行。
- 在 `win11-lab` 刷新协议 v3 实机 E2E：覆盖 CLI/MCP 后台 exec、查询、取消、force reconnect、文件传输、截图、窗口枚举、power guard 和 stdout 截断。

## 0.1.4 - 2026-06-12

- 修复 `rcwctl mouse-scroll --delta -1` 这类负数参数解析，鼠标移动和点击坐标也支持负数。
- 控制端短连接完成后主动发送 WebSocket close frame，减少服务端无 close handshake 的正常断开噪音。
- 将常见短连接断开日志从 server `warn` 降级为 `debug`，保留真正异常帧错误的告警。

## 0.1.3 - 2026-06-12

- 修复文件上传/下载的清理路径，降低中断传输后残留临时文件、泄漏句柄或误判成功的风险。
- 加强二进制传输帧解析，返回协议错误而不是依赖 panic，并补齐帧解析边界测试。
- 将 `rcwctl`、`rcw-server` 和 `rcw-host` 的大入口文件拆成按职责划分的模块，降低后续维护成本。
- 引入 `rcwctl` 控制端 client/transport 边界，并显式化上传、下载的请求生命周期。
- 加固 server outbound 状态处理，减少慢连接和关闭路径上的内存与状态一致性风险。
- 将 host 进程输出队列改为有界通道，避免持续输出时无限积压内存。
- 为 Windows FFI 边界增加 RAII 资源管理和 `unsafe` 安全说明。
- 升级 GitHub artifact actions，保持发布流水线使用当前 Node 运行时。
- 将 Rust workspace 和 npm 包版本提升到 `0.1.3`。

## 0.1.2 - 2026-06-12

- 切换 npm 发布到 GitHub Actions trusted publishing，移除 `NPM_TOKEN` 依赖。
- 将下一次发布基线抬到 `0.1.2`。

## 0.1.1 - 2026-06-11

- 修复 npm 首发恢复路径，将 Windows arm64 平台包改为 `rcwctl-windows-arm64` 以避开 registry 对 `rcwctl-win32-arm64` 的误拦截。
- 增强发布 workflow 的恢复能力：GitHub Release 已存在时覆盖上传 assets，npm 包版本已存在时跳过发布。
- 增加 `rcwctl` 元包和按平台拆分的 npm 二进制包，方便镜像环境直接分发预编译控制端 CLI。
- 扩展自动发布流水线，覆盖 Linux/macOS/Windows 的 x86-64 和 arm64 目标。
- 将项目文档重组为长期维护和迭代阶段的结构。
- 增加标准开源项目入口文件：`LICENSE`、`CONTRIBUTING.md`、`SECURITY.md` 和本变更日志。
- 将新增文档入口汉化，统一文档语言。

## 0.1.0 - 2026-06-11

- 实现 v1 中继架构，包括 `rcw-server`、`rcw-host.exe`、`rcwctl` 和共享 crate `rcw-common`。
- 增加控制端 token + 机器 ID/TOTP 的会话创建流程。
- 增加命令执行、上传/下载、截图、窗口枚举、鼠标输入、键盘输入、会话状态和会话关闭流程。
- 增加 host/controller/server 三端 JSONL 审计事件，并通过 request ID 对齐。
- 增加可见的 host 控制台体验，包括机器 ID、TOTP、连接状态、权限状态、剪贴板状态、电源请求状态和操作摘要。
- 增加 Windows 剪贴板连接信息更新，以及 host 进程运行期间的临时防休眠/防熄屏请求。
- 增加 Linux 到 Windows MSVC 的 `rcw-host.exe` 静态 CRT 交叉构建流程。
- 在 Windows VM 中验证 v1 主链路。

已知验证缺口：

- 标准用户交互桌面下的权限显示仍需最终实机确认。管理员 elevated 桌面行为已经验证通过。
