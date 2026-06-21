# 开发工作流

本文定义 `rcw` 仓库的默认开发工作流。目标不是约束形式感，而是把后续开发收敛到一条低摩擦、可并行、可回溯的路径上：

- 需求、Bug、想法和治理项先沉淀为 issue。
- 开发默认在 `worktree + branch` 中进行，而不是直接在 `main` 上改。
- 合入主线通过 PR 完成，并把验证证据挂到 PR 或关联 issue 上。

调试和测试的运行期事实分别见 [debug-workflow.md](debug-workflow.md) 和 [testing.md](testing.md)。本文只负责回答“任务怎么流转”和“代码怎么进入主线”。

## 1. 基本原则

- `issue` 是任务真相源。
- `main` 是稳定主线，不作为日常开发分支。
- 一个 issue 默认对应一个 branch 和一个 worktree。
- `PR` 是代码评审、验证证据和合入决策的载体。
- 文档、脚本、测试和实现一起演进；不要让流程知识只停留在聊天上下文里。

## 2. 什么时候先开 issue

以下情况默认先开 issue，再开始开发：

- 新功能或产品想法
- Bug
- 协议、安全、发布或兼容性风险项
- 文档治理、开发体验治理、CI/测试治理
- 明确需要后续回溯、统计或发版引用的事项

以下情况可以不单独开 issue，直接在已有 issue 或 PR 中处理：

- 小型拼写修正
- 已有 issue 范围内的顺手修复
- 纯机械性重命名，且没有行为变化

如果拿不准，优先开 issue。这个仓库更怕上下文丢失，不怕多一个结构化记录。

## 3. Issue 结构

issue 至少应包含这些部分：

- 背景
- 目标
- 非目标
- 现状 / 约束
- 验收标准
- 验证方式
- 风险 / 依赖

推荐额外补充：

- 影响范围
- 相关文档或代码路径
- 后续可拆分项

建议标签最少覆盖：

- 类型：`bug`、`enhancement`、`documentation`、`governance`
- 状态：`status:ready`、`status:in-progress`、`status:blocked`
- 区域：`area:server`、`area:control`、`area:host-console`、`area:host-gui`、`area:workflow`、`area:test`

标签体系不要求一次性设计完整，但同一语义不要反复换名字。

## 4. 从 issue 领取任务

推荐领取节奏：

1. 先确认 issue 的目标和验收标准是否足够明确。
2. 如果不明确，先在 issue 评论里补边界，不要直接开写。
3. 明确后，再从该 issue 创建 branch 和 worktree。

默认约定：

- 一个活跃开发任务只对应一个主 issue。
- 一个 PR 默认只解决一个 issue 的一个明确目标。
- 如果做着做着发现问题显著扩散，优先拆新 issue，不要把单个 PR 养成杂烩。

### 复杂任务的父子 issue 规则

如果一个任务同时满足下面任一条件，就不应只用一个平铺 issue 硬扛，而应拆成父子 issue：

- 跨越多个子系统，例如 `server + control + host`
- 同时包含文档、实现、测试、CI 或发布多个维度
- 明显需要分阶段推进，且每个阶段都可以独立验收
- 预计会产生多个 PR，而不是一个 PR 就能安全落地

推荐拆法：

- 父 issue 负责描述总背景、总目标、非目标、总体验收标准和拆分计划
- 子 issue 各自承接一个可独立交付、可独立验证的子目标

父 issue 不应自己再承载大段实现细节；它主要负责：

- 解释为什么要拆
- 链接所有子 issue
- 跟踪整体完成度
- 在所有关键子 issue 完成后再关闭

子 issue 应明确写出：

- 自己解决的是哪一块
- 与父 issue 的关系
- 自己的验收标准和验证方式

推荐在父 issue 中维护一份平铺 checklist，例如：

```text
- [ ] #33 调试工作流
- [ ] #35 测试工作流
- [ ] #36 issue / PR / worktree 治理
```

一个经验规则是：如果你已经开始在 issue 正文里写“第一阶段 / 第二阶段 / 第三阶段”，那通常就该拆父子 issue 了。

## 5. Branch 与 Worktree 规则

默认开发路径是：

1. 在主仓库同步最新 `main`
2. 从 issue 创建分支
3. 为该分支创建独立 worktree
4. 在该 worktree 中开发、调试、测试

### 命名约定

branch 建议统一命名为：

```text
issue/<number>-<slug>
```

例如：

```text
issue/36-workflow-templates
issue/34-host-gui-bom-tolerance
```

worktree 目录建议与 branch 对应，例如：

```text
../remote-control-for-windows-issue-36
```

或使用 Paseo 生成的独立 worktree，但 branch 名仍应沿用上述规则。

### 创建方式

可以直接使用 `git worktree`，也可以使用 Paseo 创建 worktree。两者都可以，仓库层只约束结果，不强绑具体工具：

- 必须有独立 branch
- 必须有独立工作目录
- 不要继续在主工作区直接做功能开发

### 主工作区的职责

主工作区以后主要用于：

- 浏览 backlog
- 创建或整理 issue
- review PR
- release 准备
- 少量只读检查

如果要写代码、跑本地验证或做破坏性实验，请切到对应 worktree。

## 6. PR 规则

所有合入 `main` 的代码变更都应通过 PR。

PR 正文至少应说明：

- 关联 issue
- 改了什么
- 没改什么
- 风险点
- 跑了哪些验证
- 证据路径或 issue/PR 评论链接

推荐 PR 保持这些特征：

- 范围单一
- 提交历史可读
- 验证结论明确
- 剩余缺口不隐藏

不推荐：

- 一个 PR 混合功能、重构、治理和无关格式化
- PR 里只写“已测试”
- 明明还有验证缺口，却在描述里假装已经闭环

## 7. 验证证据要求

PR 或 issue 评论里至少应保留：

- 实际运行的命令
- 实际跑的是哪一层验证
- 关键输出摘要
- 证据文件路径
- 未覆盖项或剩余风险

常见证据入口：

- workspace 检查：`cargo fmt --check`、`cargo test --workspace`、`cargo clippy --workspace -- -D warnings`
- Windows 调试证据：见 [debug-workflow.md](debug-workflow.md)
- 测试分层与 `pytest` 脚手架：见 [testing.md](testing.md)

一句“本地通过”不算证据。

## 8. 合并策略

建议仓库默认使用 `squash merge` 合入 `main`。

理由：

- 一条 PR 对应主线上的一个清晰落点
- 更方便发版时引用 issue / PR
- 更容易保持主线历史干净

如果确实需要保留多提交历史，应在 PR 描述中说明原因，而不是默认所有 PR 都保留碎提交。

## 9. GitHub 仓库设置建议

这些属于仓库设置，不是代码仓库内能自动生效的内容，但应按本文执行：

- 为 `main` 打开 branch protection
- 禁止直接 push 到 `main`
- 要求通过 PR 合入
- 要求必需检查通过后才能合入
- 需要时要求分支在合入前同步最新 `main`

当前 CI 门禁应至少覆盖：

- Rust `fmt`
- Rust `test`
- Rust `clippy`
- 前端构建或对应 smoke

Windows 交互桌面和重型 E2E 不应为了形式完整而强塞进 CI；这些保留在本机 / VM 验证，规则见 [testing.md](testing.md)。

## 10. 与 skills 的分工

仓库文档是规则真相源。

repo-local skills 只负责：

- 帮 agent 选择正确入口文档
- 提醒它遵守本仓库的 branch / worktree / PR 约定
- 引导它去用现有模板，而不是凭空自由发挥

不要让 skill 成为第二份事实来源。规则变化后，应先更新本文，再更新 skill 的路由描述。

## 11. 推荐日常节奏

一个典型任务流应当长这样：

1. 发现需求、Bug 或治理项
2. 先整理成 issue
3. 从 issue 创建 branch 和 worktree
4. 在 worktree 中开发、调试、测试
5. 提交 PR，并附上验证证据
6. review 通过后合入 `main`
7. 关闭 issue，并在关闭时引用对应 PR

如果后续要发版，再从 `main` 上按 release 流程推进，而不是从功能分支直接发。

## 12. 当前阶段的最低落地标准

只要满足下面这些，就算仓库已经切换到新的开发工作流：

- 新需求 / Bug / 治理项默认先开 issue
- 日常开发默认在 worktree + branch 中完成
- 合入主线默认通过 PR
- PR 中有最基本的验证证据
- `main` 不再作为日常直接开发分支

这套规则先求稳定复用，再逐步加自动化，不追求一开始就把所有辅助脚手架做满。
