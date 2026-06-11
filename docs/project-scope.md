# 项目范围

Remote Control for Windows 提供一条临时、可见的 Windows 远程协助通道，用于明确授权的支持会话。它优先服务研发和 Codex agent 的脚本化诊断需求，而不是替代完整商业远控产品。

## 核心用户

- 客户或测试人员：运行 `rcw-host.exe`，在支持期间保持窗口打开，分享可见的机器 ID 和 TOTP，并可通过关闭窗口停止控制。
- 研发或运维人员：使用 `rcwctl` 打开会话，执行诊断、传输文件、截图和基础桌面输入。
- 自动化 agent：通过 `rcwctl` 完成有边界的支持任务，默认应使用 `--json`。
- 服务端管理员：部署 `rcw-server`，管理控制端 token，并查看中继侧日志。

## v1 支持的工作流

- 被控端通过出站 WebSocket 登记在线。
- 使用控制端 token + 机器 ID/TOTP 创建会话。
- 控制端本地 session 文件支持短生命周期 `rcwctl` 多次调用复用。
- 远程命令执行，包含 stdout、stderr、退出码、超时和清理。
- 上传和下载，包含分块传输和 SHA-256 校验。
- 从 Windows 交互桌面截图。
- 枚举可见窗口。
- 使用屏幕绝对坐标执行鼠标移动、点击和滚轮。
- 输入文本和常见按键/快捷键。
- 被控端自动更新安全的剪贴板连接信息。
- 被控端运行期间临时阻止系统睡眠和显示器熄屏。
- host、controller、server 三端审计日志通过 request ID 对齐。

## 安全要求

- `rcw-host.exe` 必须保持为可见控制台进程。
- 关闭被控端窗口即终止控制。
- 被控端不安装、不持久化、不自启动、不隐藏、不注入其他进程、不注册服务。
- 被控端不自动提权，也不绕过 UAC。
- 只有客户或测试人员显式以管理员身份启动时，host 才能以 elevated 状态运行。
- 剪贴板连接信息不得包含控制端 token、session token、TOTP seed、原始机器标识或文件内容。
- 审计日志必须脱敏敏感 token，避免记录完整文件内容或默认完整命令输出。

## 当前基线

v1 链路已经在以下 crate 中实现：

- `rcw-common`：协议、配置、ID、TOTP、审计 helper 和传输 helper。
- `rcw-server`：健康检查、host/control WebSocket 端点、token 校验、TOTP 会话创建、内存 session、relay 路由、close、status、ping/heartbeat、基础限流和 server 审计。
- `rcw-host`：Windows 被控端连接、重连循环、机器 ID/TOTP 显示、剪贴板刷新、TOTP 鉴权、命令执行、文件传输、截图、窗口/鼠标/键盘操作、审计、权限显示和电源 guard。
- `rcwctl`：`open/status/exec/upload/download/screenshot/windows/move/click/scroll/type/key/close`、JSON 输出、session 文件复用和 controller 审计。

截至 2026-06-11，该基线已在 Windows VM 中完成主要验证；剩余标准用户交互桌面权限显示验证项见 [testing.md](testing.md)。

## 非目标

项目不提供：

- 静默控制或隐藏运行。
- 后台持久化、启动项注册或服务安装。
- UAC 绕过或自动权限提升。
- 内核驱动、进程注入或键盘记录。
- 实时视频流或后台录屏。
- 多租户 SaaS 管理、计费或组织管理。
- v1 中的中央审计数据库。
- 当前基线中的 P2P 穿透。
