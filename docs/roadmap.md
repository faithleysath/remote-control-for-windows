# 路线图

项目已经越过从零实现 v1 的阶段。当前路线图关注在保留已验证远控基线的基础上，提升发布质量、安全检查和操作体验。

## 已完成基线

截至 2026-06-11，早期 v1 基线已完成并验证：

- Rust workspace，包含 `rcw-common`、`rcw-server`、`rcw-host` 和 `rcwctl`。
- 通过 `rcw-server` 实现 host/control WebSocket 中继。
- 使用控制端 token + 机器 ID/TOTP 创建会话。
- 控制端 session 文件复用，并支持显式 close 和服务端 session 失效。
- 远程命令执行，包含输出、退出码、超时和进程清理。
- 上传/下载，包含分块传输和 SHA-256 校验。
- 截图、窗口枚举、鼠标输入和键盘输入。
- host 剪贴板连接信息，并对 token/seed 脱敏。
- host 侧临时 display/system 电源请求。
- host、controller、server 三端 JSONL 审计日志。
- Linux 到 Windows MSVC 的静态 CRT host 交叉构建。
- Windows VM 中的主流程实机 E2E 覆盖。

当前代码还已实现协议 v4 的 host identity/routing、server-owned 后台 exec、CLI/MCP 的 `exec-status` / `exec-cancel`、MCP 文件传输后台任务和取消语义。这些能力需要按 [testing.md](testing.md) 刷新 Windows 实机验证后，才能归入已验证基线。

## 维护优先级

- 闭合剩余的标准用户交互桌面验证缺口。
- 刷新协议 v4 host identity/routing、后台 exec、exec 取消和 MCP 文件传输取消的 Windows 实机 E2E 证据。
- 围绕现有手工 E2E 清单增加可重复的 Windows VM smoke 自动化。
- 改进发布打包和校验和生成。
- 增加 Linux workspace 检查的 CI 覆盖。
- 为未来协议消息变更增加兼容性测试。
- 加强审计脱敏测试。
- 改进常见 Windows 桌面状态的操作者错误提示，例如锁屏、UAC 安全桌面和非交互 session 0。

## 候选能力

以下是可能的后续增强，不代表已承诺发布：

- 一条命令收集诊断包。
- 更好的多显示器选择和坐标报告。
- 窗口相对坐标辅助。
- 在保持当前安全模型的前提下增加 OCR 或 UI 元素发现辅助。
- 控制端操作 transcript，便于会话后复盘。
- 可选的客户侧命令类别允许/拒绝策略。
- 分发 `rcw-host.exe` 的临时一次性下载链接。
- 通过同一中继模型提供端口转发，并保留明确审计和客户可见状态。
- 为需要集中留存的部署增加服务端持久审计存储。

## 明确不进入路线图

项目不应增加：

- 隐藏远控。
- 后台持久化。
- 服务安装或启动项注册。
- 自动 UAC 提权或 UAC 绕过。
- 内核驱动。
- 进程注入。
- 键盘记录。
- 后台录屏。
