#[cfg(not(windows))]
use std::{fs, path::PathBuf};

use anyhow::{anyhow, Result};

#[cfg(not(windows))]
const APP_DIR_NAME: &str = "RemoteControlForWindows";
pub(crate) struct SingleInstanceGuard {
    #[cfg(windows)]
    handle: windows::Win32::Foundation::HANDLE,
    #[cfg(not(windows))]
    path: PathBuf,
}

// The guard only owns an OS mutex/file-lock handle and closes it on drop. It does
// not expose handle mutation, and releasing these handles is thread-independent.
unsafe impl Send for SingleInstanceGuard {}
unsafe impl Sync for SingleInstanceGuard {}

impl SingleInstanceGuard {
    pub(crate) fn acquire() -> Result<Self> {
        #[cfg(windows)]
        {
            windows_single_instance_guard()
        }

        #[cfg(not(windows))]
        {
            let path = app_data_dir()?.join("rcw-host.lock");
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(_) => Ok(Self { path }),
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => Err(anyhow!(
                    "another rcw-host instance is already running on this machine"
                )),
                Err(err) => Err(err.into()),
            }
        }
    }
}

impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        #[cfg(windows)]
        {
            // SAFETY: The handle is owned by this guard and was returned by CreateMutexW.
            let _ = unsafe { windows::Win32::Foundation::CloseHandle(self.handle) };
        }

        #[cfg(not(windows))]
        {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(not(windows))]
fn app_data_dir() -> Result<PathBuf> {
    if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
        return Ok(PathBuf::from(local_app_data).join(APP_DIR_NAME));
    }
    if let Some(data_home) = std::env::var_os("XDG_DATA_HOME") {
        return Ok(PathBuf::from(data_home).join("rcw-host"));
    }
    Ok(std::env::current_dir()?.join(".rcw-host"))
}

#[cfg(windows)]
fn windows_single_instance_guard() -> Result<SingleInstanceGuard> {
    use windows::{
        core::w,
        Win32::{
            Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS},
            System::Threading::CreateMutexW,
        },
    };

    // SAFETY: CreateMutexW is called with no security attributes and a static
    // null-terminated UTF-16 name. The returned handle is closed by the guard.
    let handle = unsafe { CreateMutexW(None, true, w!("Global\\RemoteControlForWindowsHost")) }
        .map_err(|err| anyhow!("failed to create single-instance mutex: {err}"))?;
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        // SAFETY: The handle was just returned by CreateMutexW and is not kept by the guard.
        let _ = unsafe { CloseHandle(handle) };
        return Err(anyhow!(
            "another rcw-host instance is already running on this machine"
        ));
    }
    Ok(SingleInstanceGuard { handle })
}
