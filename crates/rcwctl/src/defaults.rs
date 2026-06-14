use std::time::Duration;

pub(crate) const DEFAULT_EXEC_TIMEOUT_MS: u64 = 24 * 60 * 60 * 1000;
pub(crate) const DEFAULT_EXEC_WAIT_MS: u64 = 90_000;
pub(crate) const DEFAULT_MCP_TRANSFER_WAIT_MS: u64 = 60_000;
pub(crate) const EXEC_STATUS_POLL_INTERVAL: Duration = Duration::from_millis(250);
