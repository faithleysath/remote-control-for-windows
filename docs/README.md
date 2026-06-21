# 文档索引

这个目录存放长期维护文档。约定是：`README.md` 讲入口，`docs/` 里的文档各自只负责一类稳定信息，不再混放验证记录、发布历史和一次性规划稿。

## 先读

- [项目范围](project-scope.md)：产品模式、目标用户、当前基线能力和明确不做的事。
- [安全模型](security.md)：鉴权、凭据、可见性、审计和长期成立的安全边界。
- [技术架构](architecture.md)：crate 职责、运行拓扑、状态模型和失败处理。

## 契约与接口

- [协议设计](protocol.md)：WebSocket 消息、binary frame、错误码和兼容性规则。
- [CLI 参考](cli.md)：`rcwctl` 与 MCP 的命令、参数、等待窗口和调用约定。

## 运行与发布

- [开发工作流](dev-workflow.md)：issue、worktree、PR、merge 和验证证据的默认协作路径。
- [配置说明](configuration.md)：环境变量、嵌入配置、本地状态路径和部署入口。
- [调试工作流](debug-workflow.md)：本机 + Windows VM 的真实可用调试路径、脚本入口和证据要求。
- [测试工作流](testing.md)：测试分层、门禁、业务覆盖矩阵和当前 `pytest` smoke/E2E 脚手架。
- [发布流程](release.md)：版本策略、发布清单、构建命令、产物和发布后检查。

## 平台与界面

- [Windows 实现说明](windows-apis.md)：`rcw-host` 当前依赖的 Windows 能力、API 和平台限制。
- [Tauri Host GUI](host-gui.md)：GUI 页面范围、command 边界、事件模型和 capability 基线。

## 规划

- [项目范围](project-scope.md)：同时承载当前边界、近期优先级和后续方向。
