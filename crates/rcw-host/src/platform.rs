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
        return windows_impl::stable_machine_material();
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
        return windows_impl::is_elevated();
    }

    #[cfg(not(windows))]
    {
        false
    }
}

pub fn copy_connection_info(text: &str) -> Result<()> {
    #[cfg(windows)]
    {
        return windows_impl::copy_connection_info(text);
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
        return windows_impl::screenshot_png(display);
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
        return windows_impl::list_windows();
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
        return windows_impl::mouse_move(x, y);
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
        return windows_impl::mouse_click(x, y, button);
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
        return windows_impl::mouse_scroll(delta);
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
        return windows_impl::keyboard_type(text);
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
        return windows_impl::keyboard_key(key);
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
mod windows_impl {
    use std::{ffi::c_void, mem::size_of, ptr::copy_nonoverlapping};

    use anyhow::{anyhow, Result};
    use image::{codecs::png::PngEncoder, ColorType, ImageEncoder};
    use rcw_common::protocol::{RectInfo, WindowInfo};
    use windows::{
        core::{w, PWSTR},
        Win32::{
            Foundation::{CloseHandle, BOOL, HANDLE, HGLOBAL, HWND, LPARAM, RECT},
            Graphics::Gdi::{
                BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC,
                GetDIBits, GetSystemMetrics, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER,
                BI_RGB, DIB_RGB_COLORS, HBITMAP, HGDIOBJ, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
                SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SRCCOPY,
            },
            Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY},
            System::{
                DataExchange::{
                    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData, CF_UNICODETEXT,
                },
                Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE},
                Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ},
                Threading::{GetCurrentProcess, OpenProcessToken},
            },
            UI::{
                Input::KeyboardAndMouse::{
                    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT,
                    KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
                    MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_RIGHTDOWN,
                    MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL, MOUSEINPUT, VIRTUAL_KEY, VK_CONTROL,
                    VK_MENU, VK_SHIFT,
                },
                WindowsAndMessaging::{
                    EnumWindows, GetForegroundWindow, GetWindowRect, GetWindowTextW,
                    GetWindowThreadProcessId, IsWindowVisible, SetCursorPos,
                },
            },
        },
    };

    pub fn stable_machine_material() -> Result<Vec<u8>> {
        let mut bytes = vec![0_u8; 512];
        let mut len = bytes.len() as u32;
        unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                w!("SOFTWARE\\Microsoft\\Cryptography"),
                w!("MachineGuid"),
                RRF_RT_REG_SZ,
                None,
                Some(bytes.as_mut_ptr() as *mut c_void),
                Some(&mut len),
            )
            .ok()?;
        }
        bytes.truncate(len as usize);
        Ok(bytes)
    }

    pub fn is_elevated() -> bool {
        unsafe {
            let mut token = HANDLE::default();
            if !OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).as_bool() {
                return false;
            }
            let mut elevation = TOKEN_ELEVATION::default();
            let mut returned = 0_u32;
            let ok = GetTokenInformation(
                token,
                TokenElevation,
                Some(&mut elevation as *mut _ as *mut c_void),
                size_of::<TOKEN_ELEVATION>() as u32,
                &mut returned,
            )
            .as_bool();
            let _ = CloseHandle(token);
            ok && elevation.TokenIsElevated != 0
        }
    }

    pub fn copy_connection_info(text: &str) -> Result<()> {
        let wide = text
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        unsafe {
            if !OpenClipboard(HWND(0)).as_bool() {
                return Err(anyhow!("OpenClipboard failed"));
            }
            EmptyClipboard();
            let bytes = wide.len() * size_of::<u16>();
            let handle = GlobalAlloc(GMEM_MOVEABLE, bytes);
            if handle.0 == 0 {
                CloseClipboard();
                return Err(anyhow!("GlobalAlloc failed"));
            }
            let ptr = GlobalLock(handle) as *mut u16;
            if ptr.is_null() {
                CloseClipboard();
                return Err(anyhow!("GlobalLock failed"));
            }
            copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
            GlobalUnlock(handle);
            if SetClipboardData(CF_UNICODETEXT, HANDLE(handle.0)).0 == 0 {
                CloseClipboard();
                return Err(anyhow!("SetClipboardData failed"));
            }
            CloseClipboard();
        }
        Ok(())
    }

    pub fn screenshot_png(_display: Option<u32>) -> Result<Vec<u8>> {
        unsafe {
            let left = GetSystemMetrics(SM_XVIRTUALSCREEN);
            let top = GetSystemMetrics(SM_YVIRTUALSCREEN);
            let width = GetSystemMetrics(SM_CXVIRTUALSCREEN);
            let height = GetSystemMetrics(SM_CYVIRTUALSCREEN);
            if width <= 0 || height <= 0 {
                return Err(anyhow!("no interactive desktop dimensions available"));
            }
            let screen = GetDC(HWND(0));
            let memory = CreateCompatibleDC(screen);
            let bitmap = CreateCompatibleBitmap(screen, width, height);
            if bitmap.0 == 0 {
                ReleaseDC(HWND(0), screen);
                DeleteDC(memory);
                return Err(anyhow!("CreateCompatibleBitmap failed"));
            }
            let previous = SelectObject(memory, HGDIOBJ(bitmap.0));
            if !BitBlt(memory, 0, 0, width, height, screen, left, top, SRCCOPY).as_bool() {
                SelectObject(memory, previous);
                DeleteObject(HGDIOBJ(bitmap.0));
                DeleteDC(memory);
                ReleaseDC(HWND(0), screen);
                return Err(anyhow!("BitBlt failed"));
            }
            let stride = (width as usize) * 4;
            let mut bgra = vec![0_u8; stride * height as usize];
            let mut info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    biHeight: -height,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let rows = GetDIBits(
                memory,
                HBITMAP(bitmap.0),
                0,
                height as u32,
                Some(bgra.as_mut_ptr() as *mut c_void),
                &mut info,
                DIB_RGB_COLORS,
            );
            SelectObject(memory, previous);
            DeleteObject(HGDIOBJ(bitmap.0));
            DeleteDC(memory);
            ReleaseDC(HWND(0), screen);
            if rows == 0 {
                return Err(anyhow!("GetDIBits failed"));
            }
            for pixel in bgra.chunks_exact_mut(4) {
                pixel.swap(0, 2);
            }
            let mut png = Vec::new();
            PngEncoder::new(&mut png).write_image(
                &bgra,
                width as u32,
                height as u32,
                ColorType::Rgba8,
            )?;
            Ok(png)
        }
    }

    pub fn list_windows() -> Result<Vec<WindowInfo>> {
        unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
            let items = &mut *(lparam.0 as *mut Vec<WindowInfo>);
            if !IsWindowVisible(hwnd).as_bool() {
                return BOOL(1);
            }
            let mut title = vec![0_u16; 512];
            let len = GetWindowTextW(hwnd, PWSTR(title.as_mut_ptr()), title.len() as i32);
            if len <= 0 {
                return BOOL(1);
            }
            title.truncate(len as usize);
            let mut pid = 0_u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            let mut rect = RECT::default();
            if !GetWindowRect(hwnd, &mut rect).as_bool() {
                return BOOL(1);
            }
            let focused = GetForegroundWindow() == hwnd;
            items.push(WindowInfo {
                handle: format!("{:?}", hwnd.0),
                title: String::from_utf16_lossy(&title),
                process_id: pid,
                rect: RectInfo {
                    left: rect.left,
                    top: rect.top,
                    right: rect.right,
                    bottom: rect.bottom,
                },
                visible: true,
                focused,
            });
            BOOL(1)
        }

        let mut items = Vec::new();
        unsafe {
            EnumWindows(Some(enum_proc), LPARAM(&mut items as *mut _ as isize));
        }
        Ok(items)
    }

    pub fn mouse_move(x: i32, y: i32) -> Result<()> {
        unsafe {
            if !SetCursorPos(x, y).as_bool() {
                return Err(anyhow!("SetCursorPos failed"));
            }
        }
        Ok(())
    }

    pub fn mouse_click(x: i32, y: i32, button: &str) -> Result<()> {
        mouse_move(x, y)?;
        let (down, up) = match button {
            "left" => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
            "right" => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
            "middle" => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP),
            _ => return Err(anyhow!("unsupported mouse button: {button}")),
        };
        send_mouse(down, 0)?;
        send_mouse(up, 0)
    }

    pub fn mouse_scroll(delta: i32) -> Result<()> {
        send_mouse(MOUSEEVENTF_WHEEL, delta * 120)
    }

    pub fn keyboard_type(text: &str) -> Result<()> {
        for code_unit in text.encode_utf16() {
            send_unicode_key(code_unit, false)?;
            send_unicode_key(code_unit, true)?;
        }
        Ok(())
    }

    pub fn keyboard_key(key: &str) -> Result<()> {
        let parts = key.split('+').map(str::trim).collect::<Vec<_>>();
        let key_name = parts.last().copied().unwrap_or(key);
        let modifiers = &parts[..parts.len().saturating_sub(1)];
        for modifier in modifiers {
            send_virtual_key(modifier_key(modifier)?, false)?;
        }
        send_virtual_key(named_key(key_name)?, false)?;
        send_virtual_key(named_key(key_name)?, true)?;
        for modifier in modifiers.iter().rev() {
            send_virtual_key(modifier_key(modifier)?, true)?;
        }
        Ok(())
    }

    fn send_mouse(
        flags: windows::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS,
        data: i32,
    ) -> Result<()> {
        let mut input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    mouseData: data as u32,
                    dwFlags: flags,
                    ..Default::default()
                },
            },
        };
        let sent = unsafe { SendInput(&mut [input], size_of::<INPUT>() as i32) };
        if sent == 0 {
            return Err(anyhow!("SendInput mouse failed"));
        }
        Ok(())
    }

    fn send_unicode_key(code_unit: u16, key_up: bool) -> Result<()> {
        let mut flags = KEYEVENTF_UNICODE;
        if key_up {
            flags |= KEYEVENTF_KEYUP;
        }
        send_keyboard(KEYBDINPUT {
            wScan: code_unit,
            dwFlags: flags,
            ..Default::default()
        })
    }

    fn send_virtual_key(key: VIRTUAL_KEY, key_up: bool) -> Result<()> {
        let mut flags = Default::default();
        if key_up {
            flags |= KEYEVENTF_KEYUP;
        }
        send_keyboard(KEYBDINPUT {
            wVk: key,
            dwFlags: flags,
            ..Default::default()
        })
    }

    fn send_keyboard(ki: KEYBDINPUT) -> Result<()> {
        let mut input = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 { ki },
        };
        let sent = unsafe { SendInput(&mut [input], size_of::<INPUT>() as i32) };
        if sent == 0 {
            return Err(anyhow!("SendInput keyboard failed"));
        }
        Ok(())
    }

    fn modifier_key(name: &str) -> Result<VIRTUAL_KEY> {
        match name.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => Ok(VK_CONTROL),
            "alt" => Ok(VK_MENU),
            "shift" => Ok(VK_SHIFT),
            other => Err(anyhow!("unsupported modifier key: {other}")),
        }
    }

    fn named_key(name: &str) -> Result<VIRTUAL_KEY> {
        let lower = name.to_ascii_lowercase();
        let value = match lower.as_str() {
            "enter" => 0x0D,
            "tab" => 0x09,
            "escape" | "esc" => 0x1B,
            "backspace" => 0x08,
            "delete" | "del" => 0x2E,
            "up" => 0x26,
            "down" => 0x28,
            "left" => 0x25,
            "right" => 0x27,
            one if one.len() == 1 => one.as_bytes()[0].to_ascii_uppercase() as u16,
            other => return Err(anyhow!("unsupported key: {other}")),
        };
        Ok(VIRTUAL_KEY(value))
    }
}
