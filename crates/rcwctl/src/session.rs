use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{anyhow, Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

use crate::cli::Cli;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SessionFile {
    pub(crate) server: String,
    pub(crate) machine_id: String,
    pub(crate) session_id: String,
    pub(crate) session_token: String,
    pub(crate) created_at: String,
    pub(crate) last_used_at: String,
}

pub(crate) trait SessionStore: Send + Sync {
    fn read_session(&self) -> Result<SessionFile>;
    fn write_session(&self, session: &SessionFile) -> Result<()>;
    fn touch_session(&self, session: SessionFile) -> Result<()>;
    fn remove_session(&self) -> Result<()>;
}

pub(crate) struct FileSessionStore<'a> {
    cli: &'a Cli,
}

impl<'a> FileSessionStore<'a> {
    pub(crate) fn new(cli: &'a Cli) -> Self {
        Self { cli }
    }
}

impl SessionStore for FileSessionStore<'_> {
    fn read_session(&self) -> Result<SessionFile> {
        read_session(self.cli)
    }

    fn write_session(&self, session: &SessionFile) -> Result<()> {
        write_session(self.cli, session)
    }

    fn touch_session(&self, session: SessionFile) -> Result<()> {
        touch_session(self.cli, session)
    }

    fn remove_session(&self) -> Result<()> {
        remove_session(self.cli)
    }
}

#[derive(Debug, Default, Clone)]
pub(crate) struct MemorySessionStore {
    session: Arc<std::sync::Mutex<Option<SessionFile>>>,
}

impl MemorySessionStore {
    pub(crate) fn shared(session: Arc<std::sync::Mutex<Option<SessionFile>>>) -> Self {
        Self { session }
    }
}

impl SessionStore for MemorySessionStore {
    fn read_session(&self) -> Result<SessionFile> {
        self.session
            .lock()
            .map_err(|_| anyhow!("memory session lock poisoned"))?
            .clone()
            .ok_or_else(|| anyhow!("not connected; call connect first"))
    }

    fn write_session(&self, session: &SessionFile) -> Result<()> {
        *self
            .session
            .lock()
            .map_err(|_| anyhow!("memory session lock poisoned"))? = Some(session.clone());
        Ok(())
    }

    fn touch_session(&self, mut session: SessionFile) -> Result<()> {
        session.last_used_at = rcw_common::audit::now_rfc3339();
        self.write_session(&session)
    }

    fn remove_session(&self) -> Result<()> {
        *self
            .session
            .lock()
            .map_err(|_| anyhow!("memory session lock poisoned"))? = None;
        Ok(())
    }
}

pub(crate) fn project_dirs() -> Result<ProjectDirs> {
    ProjectDirs::from("", "", "rcwctl").ok_or_else(|| anyhow!("failed to resolve app data dir"))
}

fn session_path(cli: &Cli) -> Result<PathBuf> {
    if let Some(path) = &cli.session {
        return Ok(path.clone());
    }
    Ok(project_dirs()?.data_dir().join("session.json"))
}

fn read_session(cli: &Cli) -> Result<SessionFile> {
    let path = session_path(cli)?;
    let data = fs::read_to_string(&path)
        .with_context(|| format!("failed to read session file {}", path.display()))?;
    Ok(serde_json::from_str(&data)?)
}

fn write_session(cli: &Cli, session: &SessionFile) -> Result<()> {
    let path = session_path(cli)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let data = serde_json::to_vec_pretty(session)?;
    fs::write(&path, data)?;
    restrict_user_only(&path)?;
    Ok(())
}

fn touch_session(cli: &Cli, mut session: SessionFile) -> Result<()> {
    session.last_used_at = rcw_common::audit::now_rfc3339();
    write_session(cli, &session)
}

fn remove_session(cli: &Cli) -> Result<()> {
    let path = session_path(cli)?;
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.into()),
    }
}

#[cfg(unix)]
fn restrict_user_only(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn restrict_user_only(_path: &Path) -> Result<()> {
    Ok(())
}
