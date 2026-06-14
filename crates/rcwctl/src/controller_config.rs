use std::time::Duration;

use anyhow::Result;

use crate::cli::Cli;

#[derive(Debug, Clone)]
pub(crate) struct ControllerConfig {
    pub(crate) server: Option<String>,
    pub(crate) token: Option<String>,
    pub(crate) audit_label: Option<String>,
}

impl ControllerConfig {
    pub(crate) fn from_cli(cli: &Cli) -> Self {
        Self {
            server: cli.server.clone(),
            token: cli.token.clone(),
            audit_label: cli.audit_label.clone(),
        }
    }
}

pub(crate) fn config_wait_timeout(_config: &ControllerConfig) -> Result<Duration> {
    Ok(Duration::from_secs(30))
}

pub(crate) fn parse_duration(value: &str) -> Result<Duration> {
    let value = value.trim();
    if let Some(ms) = value.strip_suffix("ms") {
        return Ok(Duration::from_millis(ms.parse()?));
    }
    if let Some(seconds) = value.strip_suffix('s') {
        return Ok(Duration::from_secs(seconds.parse()?));
    }
    if let Some(minutes) = value.strip_suffix('m') {
        return Ok(Duration::from_secs(minutes.parse::<u64>()? * 60));
    }
    Ok(Duration::from_secs(value.parse()?))
}
