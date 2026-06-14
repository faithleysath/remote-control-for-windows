# v0.1.7 E2E 修复验证报告

本文记录 `remote-control-for-windows` `0.1.7` 的一次定向实机验证。本轮只验证 host 侧 GUI 修复，不重复 `0.1.6` 已覆盖的完整协议 v4 E2E 矩阵。

## 结论

`0.1.7` 修复了 125% DPI 缩放下 screenshot 被裁剪的问题，并补齐 `keyboard_key` 的常用导航键映射。MCP 鼠标坐标在同一 125% DPI 环境下通过靶场验证，未发现坐标偏移；鼠标操控实现本身没有修改。

协议版本保持 v4，server/control 兼容 `0.1.6`。

## 测试环境

- 日期：2026-06-14
- 版本：`0.1.7`
- Windows VM：`win11-data`，1920x1080，125% 缩放，交互桌面
- server：`zhang` 上的 `rcw-server v0.1.6`
- 控制端：`rcwctl@0.1.6`，Codex MCP `rcw-zhang`
- 验证对象：`0.1.7` host 修复代码候选
- host 测试二进制：`/data/windows-vm/shared/rcw-host-dpi-fix.exe`
- host exe SHA-256：`10ce443ace31a8c6b4e1121fde6643d5bd5a74de9519177c980c586c6ebd7ef6`
- host 内置地址：`ws://106.14.176.184:51234`
- 正式 `0.1.7` zhang 内置 x86-64 host 重新构建 SHA-256：`3bf14723cc2c717e0535f1e7a28a474aa1c41213e552bad970483b9e23a8575e`
- 正式 `0.1.7` zhang 内置 x86-64 host zip SHA-256：`d54d154298d7d2b7dd64caad5e8330834edc0bec0a66030a0fcea058c4fb3354`

测试时只替换 Windows host 二进制，server 和控制端保持 `0.1.6`，用于确认本次改动不依赖协议或控制端同步升级。正式 `0.1.7` zhang 内置包在同一修复代码基础上提升版本号后重新构建，未重复完整实机靶场。

## 验证摘要

### Screenshot DPI 裁剪

已验证：

- 旧 `0.1.6` host 在 1920x1080 / 125% 缩放下的 rcw screenshot 输出 `1536x864` PNG，右侧和底部被裁剪。
- `0.1.7` host 启动早期设置 per-monitor DPI awareness 后，同一环境下 rcw screenshot 输出 `1920x1080` PNG。
- 放置物理右下角红色标记后，`0.1.7` rcw screenshot 可见完整任务栏、右侧桌面和红色标记。

结论：[#6](https://github.com/faithleysath/remote-control-for-windows/issues/6) 已在 `win11-data` 125% DPI 环境下修复。

### Keyboard 导航键

已验证：

- 旧 `0.1.6` host 发送 `keyboard_key("Control+End")` 返回 `CommandFailed: unsupported key: end`。
- `0.1.7` host 发送 `keyboard_key("Control+End")` 返回成功。
- Notepad 文件末尾出现测试标记 `END_MARKER_125_DPI`，确认按键实际作用在目标窗口。

结论：[#1](https://github.com/faithleysath/remote-control-for-windows/issues/1) 中 `End` 导航键不支持的问题已修复。本轮也补齐了 `Insert`、`Home`、`PageUp` 和 `PageDown` 映射；未重新验证 issue 中早先提到但本轮无法复现的连字符变形，因为 `0.1.6` 复测中 `keyboard_type` 已确认连字符输入正常。

### 鼠标坐标靶场

已验证：

- 在 1920x1080 / 125% 缩放下运行 DPI-aware 鼠标靶场。
- MCP 发送的 `mouse_click(x,y)` 与 Windows `MouseDown` 事件中的 client 坐标和 `GetCursorPos` screen 坐标一致。
- 中心、四角内侧、象限点共 7 个样本最大误差 `0px`。
- 补充边缘安全点 `(30,30)`、`(1890,30)`、`(30,900)`、`(1890,900)` 最大误差 `0px`。
- 额外 4 靶点视觉复测由操作者盯屏确认鼠标移动到靶点上。

结论：排除 screenshot DPI 裁剪影响后，MCP 工具传入坐标与 Windows 实际点击坐标在该环境下对齐。鼠标操控逻辑未改动。

## 证据文件

本次验证过程中生成的本机临时证据包括：

- `/tmp/rcw-dpi-fix-screenshot.png`
- `/tmp/rcw-dpi-fix-marker.png`
- `/tmp/rcw-key-after-control-end.png`
- `/tmp/rcw-mouse-target-initial.png`
- `/tmp/rcw-mouse-target-after-clicks.png`
- `/tmp/rcw-mouse-target-edge.png`

这些文件用于本次人工确认，不作为仓库长期测试 fixture。

## 未覆盖项

本次未覆盖：

- 150% DPI 缩放环境。
- 多显示器和非主显示器坐标。
- 鼠标右键、中键以及所有 keyboard key 名称的全量矩阵。
- 标准用户权限下启动 host。

这些是后续增强验证项，不阻塞 `0.1.7` 发布。
