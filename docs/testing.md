# 测试工作流

本文是 `rcw` 仓库内关于“测试”的单一真相源。它定义测试分层、门禁、证据要求和当前脚手架位置，不重复调试主路径，也不替代发布清单。

在本项目里：

- 调试：开发者或 agent 为解决某个 issue 做的定向运行、观察、复现和验证。
- 测试：可重复执行的脚本化或标准化质量检查，用来防回归、做门禁、支撑发版。
- 发布前验证：release 候选上的高成本最终子集，属于测试体系里的最后一层，不等于日常调试。

运行期主路径、Windows VM 投递和交互桌面注意事项见 [debug-workflow.md](debug-workflow.md)。本文只负责回答：

- 该写哪一层测试
- 哪些层适合进 CI
- 哪类改动最低要补什么证据
- 当前仓库测试脚手架应该往哪里加

## 当前测试入口

当前仓库的测试入口约定为：

- Rust `unit / contract`：优先放在对应 crate 的 `#[cfg(test)]` 或后续 `crates/*/tests/`
- Python 测试入口：统一放在 `tests/`，用 `pytest` 驱动
- Linux 本机 + Windows VM smoke：当前放在 `tests/e2e/`
- Windows VM 投递与交互桌面调试脚本：继续复用 `scripts/debug/windows/`

不要再并行发明第二套 Windows smoke 入口，也不要把长期规则只写进 skill。

## 分层总览

| 层级 | 主要职责 | 成本 | 环境 | 自动化优先级 | CI 适配 |
| --- | --- | --- | --- | --- | --- |
| `unit / contract` | 保护纯逻辑、默认值、解析、脱敏、协议结构和工具参数语义 | 低 | Linux / macOS / Windows 本地，CI | 最高 | 默认进入 CI |
| `integration / protocol` | 保护 crate 间约定、消息流、状态流转、协议协作 | 中 | 以 Linux 为主，可扩展专用环境 | 高 | 适合进入 CI 的稳定子集 |
| `smoke e2e` | 证明关键主链路在真实环境里还活着 | 中到高 | Linux 开发机 + `win11-main`，必要时 `SessionId=1` | 高 | 默认不强塞公共 CI |
| `business e2e` | 覆盖完整业务闭环和产品级行为，而不只是链路活性 | 高 | Linux + Windows VM + 真实交互桌面 + 业务准备 | 中 | 通常不进公共 CI |
| `release validation` | 发版前的最终高价值回归子集和产物验证 | 最高 | 与发版环境一致 | 中 | 不进日常 CI |

## 每层默认职责

### `unit / contract`

默认用来防：

- 协议 payload 编解码漂移
- 默认值、边界值或持续时间语义被改坏
- CLI / MCP 参数名、字段名、布尔语义漂移
- 审计脱敏、摘要、路径裁剪、ID 处理退化
- 状态机、任务状态、取消语义、错误码映射回归

这层测试应尽量：

- 快
- 无外部依赖
- 无桌面依赖
- 适合每次 PR 默认跑

### `integration / protocol`

默认用来防：

- controller / server / host 之间的协作约定被改坏
- session、command、transfer、tunnel 的消息序列漂移
- reconnect、stale connection、timeout、cancel 等跨模块语义回归
- 单个 crate 单测都绿，但组合起来已经不再成立

这层测试应尽量：

- 不依赖真实 Windows 交互桌面
- 优先跑协议层、状态层和进程内/本机消息层
- 只把稳定子集放进 CI

### `smoke e2e`

默认用来防：

- “代码能编译，单测全绿，但真实主链路已经死了”
- `SessionId=1` 交互桌面链路断掉
- `rcwctl` 或 `rcwctl mcp` 在真实 host 上打不开最小闭环
- GUI 窗口或 WebView2/CDP 入口已经坏掉

这层不追求业务全覆盖。它只保护关键主路径还通着。当前仓库要求 smoke 尽量短、尽量可复跑、尽量沿用已经验证过的 debug 路径，而不是重造 guest 启动方法。

### `business e2e`

默认用来防：

- 业务闭环被改坏，但 smoke 没覆盖到
- 真实 agent / 开发者任务里的多步链路回归
- 输入焦点、文件传输、隧道使用、负向流等长链路问题

这层应按“业务能力”而不是“代码目录”组织，允许成本更高，也允许更长的准备步骤。

### `release validation`

默认用来防：

- 发布产物本身不能用
- GUI / Windows 包在目标环境里缺运行时或起不来
- 发布版本的高价值链路没有被最终确认

这层可以半自动，但必须有明确的 release-blocking 子集和证据产物。

## 默认门禁

下表定义 PR 或 issue 回复里应给出的最低测试证据。更高风险改动可以加码，但不应降到更低层。

| 改动类型 | 最低测试证据 |
| --- | --- |
| 纯文档、注释、索引整理 | 链接/文本自检，必要时说明未跑代码 |
| `rcw-common`、`rcw-server`、`rcwctl` 的纯逻辑改动 | `cargo fmt --check`、`cargo test --workspace`、`cargo clippy --workspace -- -D warnings`，外加相关 `unit / contract` 说明 |
| 协议字段、错误码、超时/等待窗口、session/command/tunnel 语义改动 | 基础 workspace 检查，外加对应 `integration / protocol` 或明确说明缺口 |
| Windows host 非桌面逻辑改动 | 基础 workspace 检查，外加 Windows 交叉构建；如影响真实链路，再补对应 smoke |
| `windows` / `screenshot` / 输入 / 交互桌面相关改动 | 上述检查外加 `uv run pytest tests/e2e/test_console_smoke.py -m smoke` 或等价证据 |
| `rcwctl mcp`、tool schema、stdio 行为改动 | 上述检查外加 `uv run pytest tests/e2e/test_mcp_smoke.py -m smoke` 或等价证据 |
| `rcw-host-gui` / Tauri / WebView2 / CDP 改动 | 上述检查外加 `uv run pytest tests/e2e/test_gui_cdp_smoke.py -m smoke`；如影响控制端连 GUI host，再补 `tests/e2e/test_gui_control_smoke.py` |
| release 候选 | 走本文件的 `release validation` 子集，并在 [release.md](release.md) 对应清单中引用证据 |

如果某层因为环境限制没有跑，提交说明里必须明确写“没跑哪层、为什么没跑、剩余风险是什么”。

## 当前业务覆盖矩阵

这张矩阵是当前仓库的第一版事实表，用来告诉后续工作应该往哪层补，而不是宣称“已经全覆盖”。

| 业务能力 | `unit / contract` | `integration / protocol` | `smoke e2e` | `business e2e` | `release validation` | 当前状态 |
| --- | --- | --- | --- | --- | --- | --- |
| 会话建立 / `status` / 断开 | 已有 | 待加强 | 已接入 `console` / `mcp` smoke | 待补 | 应纳入 | 主路径已有架子 |
| `force reconnect` / `host busy` 语义 | 已有部分 | 待补 | 未单独覆盖 | 待补 | 视改动而定 | 仍以单测为主 |
| `exec` 完整流 | 已有部分 | 待加强 | 已接入 `console` / `mcp` smoke | 待补长链路与取消流 | 应纳入子集 | 主路径已有架子 |
| `windows` / `screenshot` | 无法靠纯单测覆盖 | 不适用 | 已接入 `console` / `mcp` smoke | 待补更多桌面场景 | 应纳入子集 | 主路径已有架子 |
| 键鼠输入 / 焦点验证 | 无法靠纯单测覆盖 | 不适用 | 默认不做强断言 | 待补 | 视发布风险而定 | 需要业务 e2e |
| 上传 / 下载 / 覆盖 / 校验 / 取消 | 已有部分 | 待加强 | 还未接入首批 smoke | 待补 | 应纳入子集 | 下一轮重点 |
| tunnel open / use / status / close | 已有部分 | 待加强 | 还未接入首批 smoke | 待补 | 应纳入子集 | 下一轮重点 |
| MCP tool 生命周期 | 已有部分 | 待加强 | 已接入最小 stdio smoke | 待补完整业务闭环 | 视改动而定 | 主路径已有架子 |
| GUI 启动 / CDP / 页面可评估 | 只有少量设置类单测 | 不适用 | 已接入 GUI smoke | 待补 GUI 业务页闭环 | 应纳入子集 | 主路径已有架子 |
| GUI host 控制端连通性 | 无法靠纯单测覆盖 | 不适用 | 已接入 GUI control smoke | 待补更长链路 | 应纳入子集 | 主路径已有架子 |
| audit 落地与脱敏 | 已有 | 待加强 | smoke 会保留 audit 证据文件 | 待补长链路断言 | 应纳入 | 单测较强，运行时证据较弱 |
| 负向流：`InvalidTotp`、`HostNotFound`、非交互桌面 | 已有部分 | 待补 | 未接入首批 smoke | 待补 | 视发布风险而定 | 后续补 |

结论：

- 首批 smoke 只覆盖“主路径活性”，还没有替代完整业务 E2E。
- 文件传输、tunnel、输入焦点和负向流，下一步应进入 `business e2e` 设计。
- 新测试应优先补这张矩阵里的空白，而不是重复已有 smoke。

## 当前 smoke 架子

当前仓库已经收编了第一批可复用 `pytest` smoke 脚手架：

- [tests/e2e/test_console_smoke.py](/home/laysath/Projects/remote-control-for-windows/tests/e2e/test_console_smoke.py)
  - 复用仓库的 interactive console host 启动脚本
  - 采样本轮 `machine_id` / `host_id` / `totp`
  - 跑 `connect/status/windows/screenshot/exec/disconnect`
- [tests/e2e/test_mcp_smoke.py](/home/laysath/Projects/remote-control-for-windows/tests/e2e/test_mcp_smoke.py)
  - 复用同一条 interactive console host 路径
  - 用官方 Python MCP SDK 真实驱动 `rcwctl mcp`
  - 跑 `initialize`、`tools/list`、`connect`、`status`、`exec`、`windows`、`screenshot`、`disconnect`
- [tests/e2e/test_gui_cdp_smoke.py](/home/laysath/Projects/remote-control-for-windows/tests/e2e/test_gui_cdp_smoke.py)
  - 复用仓库的 interactive GUI 启动脚本
  - 建立本地 SSH 端口转发
  - 验证 `/json/version`、`/json/list` 和一次 `Runtime.evaluate`
  - 通过 `window.__RCW_HOST_GUI_DEBUG__.getConnectionInfo()` 校验稳定 GUI 自动化接口
- [tests/e2e/test_gui_control_smoke.py](/home/laysath/Projects/remote-control-for-windows/tests/e2e/test_gui_control_smoke.py)
  - 复用同一条 interactive GUI + CDP 路径
  - 从 `window.__RCW_HOST_GUI_DEBUG__.getConnectionInfo()` 采样本轮 `machine_id` / `host_id` / `totp`
  - 跑 `connect/status/windows/screenshot/exec/disconnect`
- [tests/e2e/harness.py](/home/laysath/Projects/remote-control-for-windows/tests/e2e/harness.py)
  - 共享本机构建、server 启停、guest 脚本渲染与同步、证据复制、运行期值采样
- [tests/e2e/conftest.py](/home/laysath/Projects/remote-control-for-windows/tests/e2e/conftest.py)
  - 提供 `pytest` fixtures、marker 入口和本轮证据目录约定

这些脚手架的设计约束是：

- 复用 [debug-workflow.md](debug-workflow.md) 已验证的 guest 调试主路径
- 保留 `report`、`stdout`、`stderr`、`audit`、`summary.json`
- 不把 `SessionId=0` 当成桌面验证
- 不把生产 `mcp__rcw_zhang` 当成当前仓库代码行为证据
- smoke 结束后自动清理 guest 侧 `rcw-host*` 进程和对应计划任务，避免把单例锁留在 VM 里

## 当前 smoke 环境变量

`pytest` smoke 统一使用以下环境变量：

| 变量 | 默认值 | 含义 |
| --- | --- | --- |
| `RCW_TEST_OUTPUT_ROOT` | `/tmp/rcw-testing/pytest` | 本轮本地证据根目录；每个测试各自建子目录 |
| `RCW_TEST_VM_HOST` | `win11-main` | guest SSH 别名 |
| `RCW_TEST_SHARE_DIR` | `/data/libvirt/work/share` | Linux 到 guest 的共享盘目录 |
| `RCW_TEST_SERVER_URL` | `ws://127.0.0.1:17800` | Linux 侧 `rcwctl` / MCP 使用的 server URL |
| `RCW_TEST_GUEST_SERVER_URL` | `ws://192.168.122.1:17800` | guest 内 host 使用的 server URL |
| `RCW_TEST_CONTROL_TOKEN` | `debug-token-rcw` | 测试控制 token |
| `RCW_TEST_START_SERVER` | `0` | 设为 `1` 时由测试夹具本机启动 `rcw-server` |
| `RCW_TEST_BUILD_HOST` | `0` | 设为 `1` 时交叉构建 `rcw-host.exe` |
| `RCW_TEST_BUILD_GUI` | `0` | 设为 `1` 时构建 `rcw-host-gui.exe` |
| `RCW_TEST_GUI_CDP_PORT` | `9222` | guest 内 WebView2 CDP 端口 |
| `RCW_TEST_LOCAL_CDP_PORT` | `9223` | Linux 本地转发端口 |

脚手架默认追求“快复跑”，所以不强制每次都重编 host，也不默认重起 server。需要更完整自举时再打开对应开关。

## 运行示例

最小 console smoke：

```bash
RCW_TEST_START_SERVER=1 \
RCW_TEST_BUILD_HOST=1 \
uv run pytest tests/e2e/test_console_smoke.py -m smoke
```

最小 MCP smoke：

```bash
RCW_TEST_START_SERVER=1 \
RCW_TEST_BUILD_HOST=1 \
uv run pytest tests/e2e/test_mcp_smoke.py -m smoke
```

最小 GUI/CDP smoke：

```bash
RCW_TEST_BUILD_GUI=1 \
uv run pytest tests/e2e/test_gui_cdp_smoke.py -m smoke
```

最小 GUI control smoke：

```bash
RCW_TEST_START_SERVER=1 \
uv run pytest tests/e2e/test_gui_control_smoke.py -m smoke
```

如果只想复用现成产物和现成 server，把构建/启动开关留在默认值即可。

## 证据要求

测试结果至少应留下：

- 本次跑的是哪一层、哪个脚本
- 证据目录路径
- 关键 `summary.json`
- 如果是 Windows smoke：对应的 guest `report/stdout/stderr/audit`
- 如果是 GUI smoke：`version.json`、`targets.json`、`evaluate.json`
- 如果是 GUI 连接信息验收：`connection-info.json`

PR、issue 评论或 release 记录里至少应写出：

- 跑了哪一层
- 哪些断言通过
- 哪些断言没跑
- 证据路径

不要只写“本地已测通过”。

## 与调试、CI、skill 的边界

- [debug-workflow.md](debug-workflow.md) 负责运行期调试主路径，不负责测试分层。
- 本文负责测试分层、门禁、矩阵和脚手架位置，不负责细讲每条 guest 调试命令。
- `rcw-test-workflow` skill 只负责路由 agent 到本文和当前脚手架，不维护第二份规则。
- 公共 CI 继续只放稳定、低成本的层；不要为了“看起来完整”把高脆弱度 Windows 交互桌面链路强塞进去。

## 后续扩展原则

后续继续补测试时，优先遵守：

1. 先决定是补 `unit / contract`、`integration / protocol`、`smoke e2e` 还是 `business e2e`。
2. 能在 crate 单测里表达的，不要上来就写 VM 级脚本。
3. 要补 Windows smoke，就扩展 `tests/e2e/` 现有 `pytest` 入口或共享 helper，不要新造第二套 guest 启动链路。
4. 要补完整业务回归，就围绕这份覆盖矩阵增量补齐，而不是随手写一次性脚本。
