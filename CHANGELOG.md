# 变更日志

本文件记录项目的重要变更。

## 未发布

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
