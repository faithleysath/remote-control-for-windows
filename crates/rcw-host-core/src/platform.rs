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

            // SAFETY: SetThreadExecutionState takes only documented flag bits and does not
            // dereference pointers or access Rust-managed memory.
            let previous = unsafe {
                SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED)
            };
            if previous == 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            Ok(Self { active: true })
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
                // SAFETY: Resetting with ES_CONTINUOUS takes no pointers and does not access
                // Rust-managed memory.
                let _ = unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
            }
        }
    }
}

pub fn stable_machine_material() -> Result<Vec<u8>> {
    #[cfg(windows)]
    {
        windows_impl::stable_machine_material()
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
        windows_impl::is_elevated()
    }

    #[cfg(not(windows))]
    {
        false
    }
}

pub fn enable_process_dpi_awareness() {
    #[cfg(windows)]
    {
        windows_impl::enable_process_dpi_awareness();
    }
}

pub fn copy_connection_info(text: &str) -> Result<()> {
    #[cfg(windows)]
    {
        windows_impl::copy_connection_info(text)
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
        windows_impl::screenshot_png(display)
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
        windows_impl::list_windows()
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
        windows_impl::mouse_move(x, y)
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
        windows_impl::mouse_click(x, y, button)
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
        windows_impl::mouse_scroll(delta)
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
        windows_impl::keyboard_type(text)
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
        windows_impl::keyboard_key(key)
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
        Ok(())
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
    use std::{
        ffi::c_void,
        mem::size_of,
        ptr::{copy_nonoverlapping, null_mut},
    };

    use anyhow::{anyhow, Result};
    use image::{codecs::png::PngEncoder, ColorType, ImageEncoder};
    use rcw_common::protocol::{RectInfo, WindowInfo};
    use windows::{
        core::w,
        Win32::{
            Foundation::{CloseHandle, GlobalFree, BOOL, HANDLE, HGLOBAL, HWND, LPARAM, RECT},
            Graphics::Gdi::{
                BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC,
                GetDIBits, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
                DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ, SRCCOPY,
            },
            Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY},
            System::{
                DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData},
                Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE},
                Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ},
                Threading::{GetCurrentProcess, OpenProcessToken},
            },
            UI::{
                HiDpi::{
                    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE,
                    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, DPI_AWARENESS_CONTEXT_SYSTEM_AWARE,
                },
                Input::KeyboardAndMouse::{
                    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT,
                    KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
                    MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_RIGHTDOWN,
                    MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL, MOUSEINPUT, VIRTUAL_KEY, VK_CONTROL,
                    VK_MENU, VK_SHIFT,
                },
                WindowsAndMessaging::{
                    EnumWindows, GetForegroundWindow, GetSystemMetrics, GetWindowRect,
                    GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible, SetCursorPos,
                    SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
                },
            },
        },
    };

    const CF_UNICODETEXT: u32 = 13;

    pub fn enable_process_dpi_awareness() {
        // SAFETY: This process-wide setting takes a documented DPI awareness context and must run
        // before user32 metrics/capture APIs are used. Failure is non-fatal; Windows may reject it
        // if DPI awareness was already fixed by manifest or earlier API use.
        unsafe {
            if SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2).is_ok() {
                return;
            }
            if SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE).is_ok() {
                return;
            }
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_SYSTEM_AWARE);
        }
    }

    fn null_hwnd() -> HWND {
        HWND(null_mut())
    }

    struct HandleGuard(HANDLE);

    impl HandleGuard {
        fn new(handle: HANDLE) -> Self {
            Self(handle)
        }

        fn get(&self) -> HANDLE {
            self.0
        }
    }

    impl Drop for HandleGuard {
        fn drop(&mut self) {
            // SAFETY: HandleGuard is only constructed from successful handle-returning Windows
            // APIs in this module, so CloseHandle receives a live owned handle.
            let _ = unsafe { CloseHandle(self.0) };
        }
    }

    struct ClipboardGuard;

    impl ClipboardGuard {
        fn open() -> Result<Self> {
            // SAFETY: A null owner HWND is allowed by OpenClipboard. The guard closes the
            // clipboard exactly once on every return path.
            unsafe { OpenClipboard(null_hwnd()) }
                .map_err(|err| anyhow!("OpenClipboard failed: {err}"))?;
            Ok(Self)
        }
    }

    impl Drop for ClipboardGuard {
        fn drop(&mut self) {
            // SAFETY: ClipboardGuard exists only after OpenClipboard succeeds.
            let _ = unsafe { CloseClipboard() };
        }
    }

    struct GlobalMemGuard {
        handle: HGLOBAL,
        owned: bool,
    }

    impl GlobalMemGuard {
        fn alloc(bytes: usize) -> Result<Self> {
            // SAFETY: Allocates a movable global memory block. The returned handle is owned by
            // this guard until ownership is explicitly transferred to the clipboard.
            let handle = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes) }
                .map_err(|err| anyhow!("GlobalAlloc failed: {err}"))?;
            if handle.is_invalid() {
                return Err(anyhow!("GlobalAlloc failed"));
            }
            Ok(Self {
                handle,
                owned: true,
            })
        }

        fn handle(&self) -> HGLOBAL {
            self.handle
        }

        fn release_to_clipboard(mut self) -> HGLOBAL {
            self.owned = false;
            self.handle
        }
    }

    impl Drop for GlobalMemGuard {
        fn drop(&mut self) {
            if self.owned {
                // SAFETY: The guard owns this handle unless ownership was transferred to the
                // clipboard by `release_to_clipboard`.
                let _ = unsafe { GlobalFree(self.handle) };
            }
        }
    }

    struct GlobalLockGuard {
        handle: HGLOBAL,
        ptr: *mut u16,
    }

    impl GlobalLockGuard {
        fn lock(handle: HGLOBAL) -> Result<Self> {
            // SAFETY: The handle comes from GlobalAlloc and remains live while the guard exists.
            let ptr = unsafe { GlobalLock(handle) } as *mut u16;
            if ptr.is_null() {
                return Err(anyhow!("GlobalLock failed"));
            }
            Ok(Self { handle, ptr })
        }

        fn as_ptr(&self) -> *mut u16 {
            self.ptr
        }
    }

    impl Drop for GlobalLockGuard {
        fn drop(&mut self) {
            // SAFETY: The guard is constructed only after GlobalLock succeeds for this handle.
            let _ = unsafe { GlobalUnlock(self.handle) };
        }
    }

    struct ScreenDc {
        hwnd: HWND,
        hdc: HDC,
    }

    impl ScreenDc {
        fn acquire(hwnd: HWND) -> Result<Self> {
            // SAFETY: GetDC accepts a null HWND to obtain the screen DC. ScreenDc releases the
            // exact HWND/HDC pair in Drop.
            let hdc = unsafe { GetDC(hwnd) };
            if hdc.is_invalid() {
                return Err(anyhow!("GetDC failed"));
            }
            Ok(Self { hwnd, hdc })
        }

        fn hdc(&self) -> HDC {
            self.hdc
        }
    }

    impl Drop for ScreenDc {
        fn drop(&mut self) {
            // SAFETY: The HWND/HDC pair is the exact pair returned by GetDC.
            let _ = unsafe { ReleaseDC(self.hwnd, self.hdc) };
        }
    }

    struct CompatibleDc {
        hdc: HDC,
    }

    impl CompatibleDc {
        fn create(source: HDC) -> Result<Self> {
            // SAFETY: `source` is a valid HDC owned by ScreenDc while this call runs. This guard
            // owns the returned compatible DC.
            let hdc = unsafe { CreateCompatibleDC(source) };
            if hdc.is_invalid() {
                return Err(anyhow!("CreateCompatibleDC failed"));
            }
            Ok(Self { hdc })
        }

        fn hdc(&self) -> HDC {
            self.hdc
        }
    }

    impl Drop for CompatibleDc {
        fn drop(&mut self) {
            // SAFETY: The HDC was returned by CreateCompatibleDC and is owned by this guard.
            let _ = unsafe { DeleteDC(self.hdc) };
        }
    }

    struct Bitmap {
        bitmap: HBITMAP,
    }

    impl Bitmap {
        fn create_compatible(source: HDC, width: i32, height: i32) -> Result<Self> {
            // SAFETY: `source` is a valid HDC and the caller checks width/height are positive.
            let bitmap = unsafe { CreateCompatibleBitmap(source, width, height) };
            if bitmap.is_invalid() {
                return Err(anyhow!("CreateCompatibleBitmap failed"));
            }
            Ok(Self { bitmap })
        }

        fn as_hbitmap(&self) -> HBITMAP {
            self.bitmap
        }

        fn as_hgdiobj(&self) -> HGDIOBJ {
            HGDIOBJ(self.bitmap.0)
        }
    }

    impl Drop for Bitmap {
        fn drop(&mut self) {
            // SAFETY: The bitmap was returned by CreateCompatibleBitmap and is owned by this
            // guard. Local variable drop order ensures any selection guard is dropped first.
            let _ = unsafe { DeleteObject(self.as_hgdiobj()) };
        }
    }

    struct BitmapSelection {
        hdc: HDC,
        previous: HGDIOBJ,
    }

    impl BitmapSelection {
        fn select(hdc: HDC, bitmap: &Bitmap) -> Self {
            // SAFETY: `hdc` is a valid memory DC and `bitmap` is live. The previous object is
            // restored in Drop.
            let previous = unsafe { SelectObject(hdc, bitmap.as_hgdiobj()) };
            Self { hdc, previous }
        }
    }

    impl Drop for BitmapSelection {
        fn drop(&mut self) {
            // SAFETY: Restores the object returned by SelectObject for the same DC.
            unsafe {
                SelectObject(self.hdc, self.previous);
            }
        }
    }

    pub fn stable_machine_material() -> Result<Vec<u8>> {
        let mut bytes = vec![0_u8; 512];
        let mut len = bytes.len() as u32;
        // SAFETY: The output buffer is valid for `len` bytes, both registry strings are static
        // null-terminated UTF-16 values, and `len` is a valid output pointer.
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
        let mut token = HANDLE::default();
        // SAFETY: GetCurrentProcess returns a pseudo-handle accepted by OpenProcessToken, and
        // `token` is a valid output slot.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.is_err() {
            return false;
        }
        let token = HandleGuard::new(token);
        let mut elevation = TOKEN_ELEVATION::default();
        let mut returned = 0_u32;
        // SAFETY: `token` is a live process token, `elevation` is the correctly sized output
        // buffer for TokenElevation, and `returned` is a valid output slot.
        let ok = unsafe {
            GetTokenInformation(
                token.get(),
                TokenElevation,
                Some(&mut elevation as *mut _ as *mut c_void),
                size_of::<TOKEN_ELEVATION>() as u32,
                &mut returned,
            )
        }
        .is_ok();
        ok && elevation.TokenIsElevated != 0
    }

    pub fn copy_connection_info(text: &str) -> Result<()> {
        let wide = text
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let _clipboard = ClipboardGuard::open()?;
        // SAFETY: The clipboard is open for this process while `_clipboard` is alive.
        unsafe { EmptyClipboard() }.map_err(|err| anyhow!("EmptyClipboard failed: {err}"))?;
        let bytes = wide.len() * size_of::<u16>();
        let global = GlobalMemGuard::alloc(bytes)?;
        {
            let locked = GlobalLockGuard::lock(global.handle())?;
            // SAFETY: `locked` points to `bytes` writable bytes and the source slice has exactly
            // the same byte length. The source and destination cannot overlap.
            unsafe {
                copy_nonoverlapping(wide.as_ptr(), locked.as_ptr(), wide.len());
            }
        }
        // SAFETY: The clipboard is open, `handle` is movable global memory containing unlocked
        // NUL-terminated UTF-16 data, and ownership transfers to the clipboard on success.
        let handle = global.handle();
        unsafe { SetClipboardData(CF_UNICODETEXT, HANDLE(handle.0)) }
            .map_err(|err| anyhow!("SetClipboardData failed: {err}"))?;
        let _ = global.release_to_clipboard();
        Ok(())
    }

    pub fn screenshot_png(_display: Option<u32>) -> Result<Vec<u8>> {
        // SAFETY: GetSystemMetrics reads process-global desktop metrics and has no pointer
        // parameters.
        let (left, top, width, height) = unsafe {
            (
                GetSystemMetrics(SM_XVIRTUALSCREEN),
                GetSystemMetrics(SM_YVIRTUALSCREEN),
                GetSystemMetrics(SM_CXVIRTUALSCREEN),
                GetSystemMetrics(SM_CYVIRTUALSCREEN),
            )
        };
        if width <= 0 || height <= 0 {
            return Err(anyhow!("no interactive desktop dimensions available"));
        }

        let screen = ScreenDc::acquire(null_hwnd())?;
        let memory = CompatibleDc::create(screen.hdc())?;
        let bitmap = Bitmap::create_compatible(screen.hdc(), width, height)?;
        let _selection = BitmapSelection::select(memory.hdc(), &bitmap);

        // SAFETY: The source and destination HDCs are valid, the compatible bitmap is selected
        // into the destination memory DC, and the dimensions were checked positive.
        if let Err(err) = unsafe {
            BitBlt(
                memory.hdc(),
                0,
                0,
                width,
                height,
                screen.hdc(),
                left,
                top,
                SRCCOPY,
            )
        } {
            return Err(anyhow!("BitBlt failed: {err}"));
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
        // SAFETY: The bitmap is live, `bgra` is large enough for a 32-bit top-down image of the
        // requested size, and `info` points to a valid BITMAPINFO structure.
        let rows = unsafe {
            GetDIBits(
                memory.hdc(),
                bitmap.as_hbitmap(),
                0,
                height as u32,
                Some(bgra.as_mut_ptr() as *mut c_void),
                &mut info,
                DIB_RGB_COLORS,
            )
        };
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
            ColorType::Rgba8.into(),
        )?;
        Ok(png)
    }

    pub fn list_windows() -> Result<Vec<WindowInfo>> {
        unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
            // SAFETY: EnumWindows receives this lparam from the call below, where it is a unique
            // mutable pointer to `items` that remains live for the duration of enumeration.
            let items = unsafe { &mut *(lparam.0 as *mut Vec<WindowInfo>) };
            // SAFETY: hwnd is supplied by EnumWindows and is valid for window query APIs during
            // this callback.
            if unsafe { !IsWindowVisible(hwnd).as_bool() } {
                return BOOL(1);
            }
            let mut title = vec![0_u16; 512];
            // SAFETY: `title` is a valid writable UTF-16 buffer.
            let len = unsafe { GetWindowTextW(hwnd, &mut title) };
            if len <= 0 {
                return BOOL(1);
            }
            title.truncate(len as usize);
            let mut pid = 0_u32;
            // SAFETY: `pid` is a valid output slot.
            unsafe {
                GetWindowThreadProcessId(hwnd, Some(&mut pid));
            }
            let mut rect = RECT::default();
            // SAFETY: `rect` is a valid output slot.
            if unsafe { GetWindowRect(hwnd, &mut rect) }.is_err() {
                return BOOL(1);
            }
            // SAFETY: GetForegroundWindow has no pointer parameters and only reads process-global
            // window state.
            let focused = unsafe { GetForegroundWindow() } == hwnd;
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
        // SAFETY: The lparam is a valid unique pointer to `items`, and EnumWindows invokes the
        // callback synchronously before returning.
        unsafe { EnumWindows(Some(enum_proc), LPARAM(&mut items as *mut _ as isize)) }?;
        Ok(items)
    }

    pub fn mouse_move(x: i32, y: i32) -> Result<()> {
        // SAFETY: SetCursorPos takes plain coordinates and does not access Rust-managed memory.
        if let Err(err) = unsafe { SetCursorPos(x, y) } {
            return Err(anyhow!("SetCursorPos failed: {err}"));
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
        let input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    mouseData: data as u32,
                    dwFlags: flags,
                    ..Default::default()
                },
            },
        };
        // SAFETY: The INPUT slice is valid for the duration of the call and cbSize matches
        // windows::Win32::UI::Input::KeyboardAndMouse::INPUT.
        let sent = unsafe { SendInput(&[input], size_of::<INPUT>() as i32) };
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
        let input = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 { ki },
        };
        // SAFETY: The INPUT slice is valid for the duration of the call and cbSize matches
        // windows::Win32::UI::Input::KeyboardAndMouse::INPUT.
        let sent = unsafe { SendInput(&[input], size_of::<INPUT>() as i32) };
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
            "insert" | "ins" => 0x2D,
            "home" => 0x24,
            "end" => 0x23,
            "pageup" | "page_up" | "page-up" | "pgup" => 0x21,
            "pagedown" | "page_down" | "page-down" | "pgdn" => 0x22,
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
