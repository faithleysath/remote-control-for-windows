# 变更日志

本文件记录项目的重要变更。

## 未发布

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
