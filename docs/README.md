# 文档索引

这个目录保存项目的长期文档。它把稳定的项目契约、操作流程和历史验证记录分开，避免 README 变成过长的混合文档。

## 推荐先读

- [项目范围](project-scope.md)：产品目标、支持工作流、非目标和当前维护基线。
- [技术架构](architecture.md)：crate 职责、运行拓扑、状态模型和失败处理。
- [安全模型](security.md)：鉴权、权限边界、可见性保证、审计脱敏和明确非目标。

## 用户与操作参考

- [CLI 参考](cli.md)：`rcwctl` 命令、全局参数、session 行为和 Codex 调用约定。
- [配置说明](configuration.md)：环境变量、编译期嵌入默认值、审计路径和服务部署说明。
- [测试与验证](testing.md)：本地检查、Windows VM E2E 计划、当前验证证据和剩余验证缺口。
- [v0.1.6 E2E 测试报告](e2e-v0.1.6.md)：协议 v4、host identity/routing、后台 exec、MCP 文件传输和取消语义的实机验证记录。
- [发布流程](release.md)：发布清单、产物、交叉构建命令和验证要求。

## 维护者参考

- [协议设计](protocol.md)：WebSocket 端点、JSON 消息、binary frame 使用方式、命令类型、错误码和兼容性规则。
- [Windows 实现说明](windows-apis.md)：`rcw-host.exe` 使用的 Win32 API 和平台行为。
- [路线图](roadmap.md)：已完成的 v1 基线、维护优先级和明确不做的能力。

## 已移除的历史文档

早期规划文档已经在 v1 基本闭环后合并进上述长期文档。后续改动应直接更新稳定契约，不再新增一次性的从零规划文档。
