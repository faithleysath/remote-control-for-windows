# Windows API 清单

## 目标

本文列出 `rcw-host.exe` v1 需要用到的 Windows 能力、建议 API、实现注意事项和限制。实现时优先使用 Rust `windows` crate 绑定 Win32 API。

## 机器 ID

目标：

- 生成稳定机器 ID。
- 不泄露原始机器标识。

候选来源：

- 注册表 `HKLM\SOFTWARE\Microsoft\Cryptography\MachineGuid`。
- WMI/SMBIOS 的主板 UUID 作为备用。

处理：

1. 读取一个或多个稳定标识。
2. 拼接产品 namespace。
3. 使用 SHA-256 或 BLAKE3。
4. 编码成短 ID，例如 `8K4F-2M7Q`。

限制：

- 不显示、不上传原始 GUID、序列号、用户名、域名。
- 如果读取失败，返回明确错误，不生成每次随机的机器 ID。

## 管理员权限检测

目标：

- 检测当前进程是否 elevated。
- 管理员运行时控制台高亮。

建议 API：

- `OpenProcessToken`
- `GetTokenInformation`
- `TOKEN_ELEVATION`

限制：

- 不调用 `ShellExecute` 的 `runas`。
- 不触发 UAC。
- 不做 UAC 绕过。

## 控制台输出

目标：

- 持续显示 ID、TOTP、连接状态、会话状态、权限状态和审计摘要。
- 管理员权限高亮。

建议 API：

- `GetStdHandle`
- `SetConsoleTextAttribute`

限制：

- 控制台颜色失败不阻断 host。
- 输出必须避免刷出完整命令输出、token、seed、文件内容。

## 剪贴板

目标：

- 启动和每次 TOTP 刷新时复制连接信息。

建议 API：

- `OpenClipboard`
- `EmptyClipboard`
- `SetClipboardData`
- `CloseClipboard`
- `GlobalAlloc`
- `GlobalLock`
- `GlobalUnlock`

剪贴板内容：

- server URL
- machine ID
- current TOTP
- TOTP period

限制：

- 不包含 control token、session token、TOTP seed、原始机器标识。
- 剪贴板被占用时显示 warning，不阻断 host 上线。

## 电源请求

目标：

- `rcw-host.exe` 运行期间阻止系统自动休眠。
- `rcw-host.exe` 运行期间阻止显示器因空闲策略熄屏。
- 退出后恢复 Windows 默认电源行为。

建议 API：

- `SetThreadExecutionState`
- flags:
  - `ES_CONTINUOUS`
  - `ES_SYSTEM_REQUIRED`
  - `ES_DISPLAY_REQUIRED`

启动时：

```text
SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED)
```

退出时：

```text
SetThreadExecutionState(ES_CONTINUOUS)
```

限制：

- 不修改系统电源计划。
- 不写注册表。
- 不安装服务。
- API 调用失败时显示 warning，不阻断 host 上线。
- 该能力只在 host 进程运行期间有效。

## 截图

目标：

- 获取当前交互桌面截图并编码为 PNG。

建议 API：

- `GetDC`
- `CreateCompatibleDC`
- `CreateCompatibleBitmap`
- `BitBlt`
- `GetDIBits`
- `ReleaseDC`
- `DeleteObject`
- `DeleteDC`

多显示器：

- 使用 `EnumDisplayMonitors` 获取显示器信息。
- v1 支持 `--display <index>`，默认主显示器或虚拟屏幕。

DPI：

- 进程启动时可设置 DPI awareness，避免截图坐标和鼠标坐标错位。
- 记录截图尺寸和 display index。

限制：

- 锁屏、UAC 安全桌面、无交互桌面时可能无法截图，必须返回明确错误。
- v1 不做实时视频流。

## 窗口枚举

目标：

- 返回可见窗口列表和基础信息。

建议 API：

- `EnumWindows`
- `IsWindowVisible`
- `GetWindowTextW`
- `GetWindowThreadProcessId`
- `GetWindowRect`
- `GetForegroundWindow`

返回字段：

- handle
- title
- process_id
- rect
- visible
- focused

限制：

- 不保证能读取所有高完整性进程窗口标题。
- 不做控件级 UI Automation。

## 鼠标输入

目标：

- 移动、点击、滚轮。

建议 API：

- `SetCursorPos`
- `SendInput`

坐标：

- v1 使用屏幕绝对坐标。
- 坐标和截图必须使用同一 DPI/显示器口径。

限制：

- 不在锁屏或 UAC 安全桌面上操作。
- 输入失败必须返回错误，不假装成功。

## 键盘输入

目标：

- 输入文本。
- 发送单键和组合键。

建议 API：

- `SendInput`
- Unicode text input 使用 `KEYEVENTF_UNICODE`
- 快捷键映射到 virtual-key code

限制：

- v1 只支持常见键名和组合键。
- 输入目标依赖当前焦点窗口。
- 不实现键盘记录。

## 命令执行

目标：

- 在当前 host 用户权限下执行命令。
- 支持 stdout/stderr、exit code、timeout。

建议 API/策略：

- 当前实现可以使用 `tokio::process::Command`，并在需要时补充 Windows 专属清理逻辑。
- 为 timeout 清理子进程树，建议 Windows Job Object：
  - `CreateJobObjectW`
  - `SetInformationJobObject`
  - `AssignProcessToJobObject`
  - `TerminateJobObject`
- 创建进程时保留 stdout/stderr pipe。

限制：

- 不自动提权。
- 不绕过 UAC。
- timeout 不能只杀父进程。
- 交互式命令不适合通过 `rcwctl exec` 运行。

## 文件系统

目标：

- 上传、下载、校验文件。

建议：

- 使用 Rust 标准库处理文件。
- Windows 路径按 Windows 规则解析。
- 上传临时写入 `.part`，校验成功后 rename。

限制：

- 默认不覆盖已有文件。
- `--overwrite` 才允许覆盖。
- 拒绝空路径、非法路径和无法创建父目录的路径。

## 已知限制

- v1 不保证锁屏后 GUI 能力可用。
- v1 不保证 UAC 安全桌面可控。
- v1 不做控件识别、OCR、UI Automation。
- v1 不做视频流。
- v1 不安装驱动、不注入进程、不常驻。
