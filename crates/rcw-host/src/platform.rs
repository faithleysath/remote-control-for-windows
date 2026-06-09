use std::{path::PathBuf, time::Duration};

use anyhow::{anyhow, Result};
use rcw_common::protocol::WindowInfo;

pub struct PowerGuard {
    active: bool,
}

impl PowerGuard {
    pub fn acquire() -> Result<Self> {
        #[cfg(windows)]
        {
            const ES_CONTINUOUS: u32 = 0x8000_0000;
            const ES_SYSTEM_REQUIRED: u32 = 0x0000_0001;
            const ES_DISPLAY_REQUIRED: u32 = 0x0000_0002;

            #[link(name = "kernel32")]
            extern "system" {
                fn SetThreadExecutionState(es_flags: u32) -> u32;
            }

            let previous = unsafe {
                SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED)
            };
            if previous == 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            return Ok(Self { active: true });
        }

        #[cfg(not(windows))]
        {
            Ok(Self { active: false })
        }
    }

    pub fn active(&self) -> bool {
        self.active
    }
}

impl Drop for PowerGuard {
    fn drop(&mut self) {
        #[cfg(windows)]
        {
            const ES_CONTINUOUS: u32 = 0x8000_0000;

            #[link(name = "kernel32")]
            extern "system" {
                fn SetThreadExecutionState(es_flags: u32) -> u32;
            }

            if self.active {
                let _ = unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
            }
        }
    }
}

pub fn stable_machine_material() -> Result<Vec<u8>> {
    #[cfg(windows)]
    {
        let output = std::process::Command::new("reg.exe")
            .args([
                "query",
                r"HKLM\SOFTWARE\Microsoft\Cryptography",
                "/v",
                "MachineGuid",
            ])
            .output()?;
        if !output.status.success() {
            return Err(anyhow!(
                "failed to query Windows MachineGuid: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let guid = stdout
            .lines()
            .find(|line| line.contains("MachineGuid"))
            .and_then(|line| line.split_whitespace().last())
            .ok_or_else(|| anyhow!("MachineGuid was not present in registry output"))?;
        return Ok(guid.as_bytes().to_vec());
    }

    #[cfg(not(windows))]
    {
        let mut material = Vec::new();
        if let Ok(hostname) = hostname::get() {
            material.extend_from_slice(hostname.to_string_lossy().as_bytes());
        }

        #[cfg(target_os = "linux")]
        if let Ok(machine_id) = std::fs::read("/etc/machine-id") {
            material.extend_from_slice(&machine_id);
        }

        if material.is_empty() {
            return Err(anyhow!("failed to read stable machine material"));
        }
        Ok(material)
    }
}

pub fn default_audit_path() -> PathBuf {
    if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(local_app_data)
            .join("RemoteControlForWindows")
            .join("host-audit.jsonl");
    }
    PathBuf::from("host-audit.jsonl")
}

pub fn is_elevated() -> bool {
    #[cfg(windows)]
    {
        return std::process::Command::new("cmd.exe")
            .args(["/C", "net", "session"])
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false);
    }

    #[cfg(not(windows))]
    {
        false
    }
}

pub fn copy_connection_info(text: &str) -> Result<()> {
    #[cfg(windows)]
    {
        run_powershell(
            "Set-Clipboard -Value $env:RCW_CLIPBOARD_TEXT",
            &[("RCW_CLIPBOARD_TEXT", text)],
        )?;
        return Ok(());
    }

    #[cfg(not(windows))]
    {
        let _ = text;
        Err(anyhow!(
            "clipboard integration is only supported on Windows"
        ))
    }
}

pub fn screenshot_png(display: Option<u32>) -> Result<Vec<u8>> {
    #[cfg(windows)]
    {
        let path = std::env::temp_dir().join(format!(
            "rcw-screenshot-{}-{}.png",
            std::process::id(),
            unix_now()
        ));
        let path_string = path.to_string_lossy().to_string();
        let display_string = display.map(|value| value.to_string()).unwrap_or_default();
        run_powershell(
            r#"
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
$screens = [System.Windows.Forms.Screen]::AllScreens
$display = $env:RCW_DISPLAY_INDEX
if ([string]::IsNullOrWhiteSpace($display)) {
  $screen = [System.Windows.Forms.Screen]::PrimaryScreen
} else {
  $index = [int]$display
  if ($index -lt 0 -or $index -ge $screens.Length) { throw "display index out of range" }
  $screen = $screens[$index]
}
$bounds = $screen.Bounds
$bitmap = New-Object System.Drawing.Bitmap $bounds.Width, $bounds.Height
$graphics = [System.Drawing.Graphics]::FromImage($bitmap)
$graphics.CopyFromScreen($bounds.Location, [System.Drawing.Point]::Empty, $bounds.Size)
$bitmap.Save($env:RCW_SCREENSHOT_PATH, [System.Drawing.Imaging.ImageFormat]::Png)
$graphics.Dispose()
$bitmap.Dispose()
"#,
            &[
                ("RCW_SCREENSHOT_PATH", path_string.as_str()),
                ("RCW_DISPLAY_INDEX", display_string.as_str()),
            ],
        )?;
        let bytes = std::fs::read(&path)?;
        let _ = std::fs::remove_file(path);
        return Ok(bytes);
    }

    #[cfg(not(windows))]
    {
        let _ = display;
        Err(anyhow!(
            "screenshot is only supported on Windows host builds"
        ))
    }
}

pub fn list_windows() -> Result<Vec<WindowInfo>> {
    #[cfg(windows)]
    {
        let output = run_powershell(
            r#"
Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class RcwUser32 {
  public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc enumProc, IntPtr lParam);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr hWnd, StringBuilder text, int count);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint processId);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }
}
"@
$focused = [RcwUser32]::GetForegroundWindow()
$items = New-Object System.Collections.Generic.List[object]
[RcwUser32]::EnumWindows({
  param([IntPtr]$hWnd, [IntPtr]$lParam)
  if ([RcwUser32]::IsWindowVisible($hWnd)) {
    $text = New-Object System.Text.StringBuilder 512
    [void][RcwUser32]::GetWindowText($hWnd, $text, $text.Capacity)
    $title = $text.ToString()
    if (-not [string]::IsNullOrWhiteSpace($title)) {
      $pid = 0
      [void][RcwUser32]::GetWindowThreadProcessId($hWnd, [ref]$pid)
      $rect = New-Object RcwUser32+RECT
      [void][RcwUser32]::GetWindowRect($hWnd, [ref]$rect)
      $items.Add([pscustomobject]@{
        handle = $hWnd.ToString()
        title = $title
        process_id = $pid
        rect = @{ left = $rect.Left; top = $rect.Top; right = $rect.Right; bottom = $rect.Bottom }
        visible = $true
        focused = ($hWnd -eq $focused)
      })
    }
  }
  return $true
}, [IntPtr]::Zero) | Out-Null
@($items) | ConvertTo-Json -Compress
"#,
            &[],
        )?;
        if output.trim().is_empty() {
            return Ok(Vec::new());
        }
        return Ok(serde_json::from_str(output.trim())?);
    }

    #[cfg(not(windows))]
    {
        Err(anyhow!(
            "window enumeration is only supported on Windows host builds"
        ))
    }
}

pub fn mouse_move(x: i32, y: i32) -> Result<()> {
    #[cfg(windows)]
    {
        run_mouse_script(Some((x, y)), None, None)?;
        return Ok(());
    }

    #[cfg(not(windows))]
    {
        let _ = (x, y);
        Err(anyhow!(
            "mouse input is only supported on Windows host builds"
        ))
    }
}

pub fn mouse_click(x: i32, y: i32, button: &str) -> Result<()> {
    #[cfg(windows)]
    {
        let (down, up) = match button {
            "left" => ("0x0002", "0x0004"),
            "right" => ("0x0008", "0x0010"),
            "middle" => ("0x0020", "0x0040"),
            _ => return Err(anyhow!("unsupported mouse button: {button}")),
        };
        run_mouse_script(Some((x, y)), Some((down, up)), None)?;
        return Ok(());
    }

    #[cfg(not(windows))]
    {
        let _ = (x, y, button);
        Err(anyhow!(
            "mouse input is only supported on Windows host builds"
        ))
    }
}

pub fn mouse_scroll(delta: i32) -> Result<()> {
    #[cfg(windows)]
    {
        run_mouse_script(None, None, Some(delta * 120))?;
        return Ok(());
    }

    #[cfg(not(windows))]
    {
        let _ = delta;
        Err(anyhow!(
            "mouse input is only supported on Windows host builds"
        ))
    }
}

pub fn keyboard_type(text: &str) -> Result<()> {
    #[cfg(windows)]
    {
        run_sendkeys(&escape_sendkeys_text(text))?;
        return Ok(());
    }

    #[cfg(not(windows))]
    {
        let _ = text;
        Err(anyhow!(
            "keyboard input is only supported on Windows host builds"
        ))
    }
}

pub fn keyboard_key(key: &str) -> Result<()> {
    #[cfg(windows)]
    {
        run_sendkeys(&sendkeys_key_expression(key))?;
        return Ok(());
    }

    #[cfg(not(windows))]
    {
        let _ = key;
        Err(anyhow!(
            "keyboard input is only supported on Windows host builds"
        ))
    }
}

pub fn kill_process_tree(pid: u32) -> Result<()> {
    #[cfg(windows)]
    {
        let status = std::process::Command::new("taskkill.exe")
            .args(["/T", "/F", "/PID", &pid.to_string()])
            .status()?;
        if !status.success() {
            return Err(anyhow!("taskkill failed for pid {pid}"));
        }
        return Ok(());
    }

    #[cfg(not(windows))]
    {
        let status = std::process::Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .status()?;
        if !status.success() {
            return Err(anyhow!("kill failed for pid {pid}"));
        }
        Ok(())
    }
}

pub async fn sleep_until_next_totp_tick(period_seconds: u64) {
    let now = unix_now();
    let next = ((now / period_seconds) + 1) * period_seconds;
    let wait = next.saturating_sub(now).max(1);
    tokio::time::sleep(Duration::from_secs(wait)).await;
}

pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(windows)]
fn run_powershell(script: &str, envs: &[(&str, &str)]) -> Result<String> {
    let mut command = std::process::Command::new("powershell.exe");
    command.args([
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        script,
    ]);
    for (key, value) in envs {
        command.env(key, value);
    }
    let output = command.output()?;
    if !output.status.success() {
        return Err(anyhow!(
            "powershell failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[cfg(windows)]
fn run_mouse_script(
    position: Option<(i32, i32)>,
    click_flags: Option<(&str, &str)>,
    scroll_delta: Option<i32>,
) -> Result<()> {
    let x = position
        .map(|value| value.0.to_string())
        .unwrap_or_default();
    let y = position
        .map(|value| value.1.to_string())
        .unwrap_or_default();
    let down = click_flags.map(|value| value.0).unwrap_or("");
    let up = click_flags.map(|value| value.1).unwrap_or("");
    let wheel = scroll_delta
        .map(|value| value.to_string())
        .unwrap_or_default();
    run_powershell(
        r#"
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class RcwInput {
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int X, int Y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint flags, uint dx, uint dy, int data, UIntPtr extraInfo);
}
"@
if (-not [string]::IsNullOrWhiteSpace($env:RCW_X)) {
  [RcwInput]::SetCursorPos([int]$env:RCW_X, [int]$env:RCW_Y) | Out-Null
}
if (-not [string]::IsNullOrWhiteSpace($env:RCW_MOUSE_DOWN)) {
  [RcwInput]::mouse_event([Convert]::ToUInt32($env:RCW_MOUSE_DOWN, 16), 0, 0, 0, [UIntPtr]::Zero)
  [RcwInput]::mouse_event([Convert]::ToUInt32($env:RCW_MOUSE_UP, 16), 0, 0, 0, [UIntPtr]::Zero)
}
if (-not [string]::IsNullOrWhiteSpace($env:RCW_WHEEL_DELTA)) {
  [RcwInput]::mouse_event(0x0800, 0, 0, [int]$env:RCW_WHEEL_DELTA, [UIntPtr]::Zero)
}
"#,
        &[
            ("RCW_X", x.as_str()),
            ("RCW_Y", y.as_str()),
            ("RCW_MOUSE_DOWN", down),
            ("RCW_MOUSE_UP", up),
            ("RCW_WHEEL_DELTA", wheel.as_str()),
        ],
    )?;
    Ok(())
}

#[cfg(windows)]
fn run_sendkeys(expression: &str) -> Result<()> {
    run_powershell(
        r#"
Add-Type -AssemblyName System.Windows.Forms
[System.Windows.Forms.SendKeys]::SendWait($env:RCW_SENDKEYS)
"#,
        &[("RCW_SENDKEYS", expression)],
    )?;
    Ok(())
}

#[cfg(windows)]
fn escape_sendkeys_text(text: &str) -> String {
    let mut escaped = String::new();
    for ch in text.chars() {
        if "+^%~(){}[]".contains(ch) {
            escaped.push('{');
            escaped.push(ch);
            escaped.push('}');
        } else {
            escaped.push(ch);
        }
    }
    escaped
}

#[cfg(windows)]
fn sendkeys_key_expression(key: &str) -> String {
    let parts = key.split('+').map(str::trim).collect::<Vec<_>>();
    let mut expression = String::new();
    for modifier in &parts[..parts.len().saturating_sub(1)] {
        match modifier.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => expression.push('^'),
            "alt" => expression.push('%'),
            "shift" => expression.push('+'),
            _ => {}
        }
    }
    let key = parts.last().copied().unwrap_or(key);
    let mapped = match key.to_ascii_lowercase().as_str() {
        "enter" => "{ENTER}".to_owned(),
        "tab" => "{TAB}".to_owned(),
        "escape" | "esc" => "{ESC}".to_owned(),
        "backspace" => "{BACKSPACE}".to_owned(),
        "delete" | "del" => "{DELETE}".to_owned(),
        "up" => "{UP}".to_owned(),
        "down" => "{DOWN}".to_owned(),
        "left" => "{LEFT}".to_owned(),
        "right" => "{RIGHT}".to_owned(),
        other if other.len() == 1 => other.to_owned(),
        other => format!("{{{}}}", other.to_ascii_uppercase()),
    };
    expression.push_str(&mapped);
    expression
}
