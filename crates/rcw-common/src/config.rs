use std::env;

use url::Url;

use crate::{RcwError, RcwResult};

pub const DEFAULT_TOTP_PERIOD_SECONDS: u64 = 120;
pub const DEFAULT_BIND_ADDR: &str = "127.0.0.1:7800";
pub const DEFAULT_SERVER_AUDIT_LOG: &str = "rcw-server-audit.jsonl";

pub fn embedded_server_url() -> Option<&'static str> {
    option_env!("RCW_EMBED_SERVER_URL").filter(|value| !value.trim().is_empty())
}

pub fn embedded_totp_period_seconds() -> Option<u64> {
    option_env!("RCW_EMBED_TOTP_PERIOD_SECONDS").and_then(|value| parse_u64(value).ok())
}

pub fn resolve_server_url(explicit: Option<&str>) -> RcwResult<String> {
    if let Some(value) = non_empty(explicit) {
        return Ok(value.to_owned());
    }
    if let Some(value) = non_empty(env::var("RCW_SERVER_URL").ok().as_deref()) {
        return Ok(value.to_owned());
    }
    if let Some(value) = embedded_server_url() {
        return Ok(value.to_owned());
    }
    Err(RcwError::MissingConfig("RCW_SERVER_URL"))
}

pub fn resolve_totp_period_seconds(explicit: Option<u64>) -> RcwResult<u64> {
    if let Some(value) = explicit {
        return validate_totp_period(value);
    }
    if let Ok(value) = env::var("RCW_TOTP_PERIOD_SECONDS") {
        return validate_totp_period(parse_u64(&value)?);
    }
    if let Some(value) = embedded_totp_period_seconds() {
        return validate_totp_period(value);
    }
    Ok(DEFAULT_TOTP_PERIOD_SECONDS)
}

pub fn control_token(explicit: Option<&str>) -> RcwResult<String> {
    if let Some(value) = non_empty(explicit) {
        return Ok(value.to_owned());
    }
    if let Some(value) = non_empty(env::var("RCW_CONTROL_TOKEN").ok().as_deref()) {
        return Ok(value.to_owned());
    }
    Err(RcwError::MissingConfig("RCW_CONTROL_TOKEN"))
}

pub fn bind_addr() -> String {
    env::var("RCW_BIND_ADDR")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_BIND_ADDR.to_owned())
}

pub fn server_audit_log_path() -> String {
    env::var("RCW_AUDIT_LOG")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_SERVER_AUDIT_LOG.to_owned())
}

pub fn ws_endpoint_url(server_url: &str, endpoint: &str) -> RcwResult<String> {
    let mut url = Url::parse(server_url)?;
    let scheme = match url.scheme() {
        "ws" | "wss" => url.scheme().to_owned(),
        "http" => "ws".to_owned(),
        "https" => "wss".to_owned(),
        other => {
            return Err(RcwError::InvalidConfig(format!(
                "unsupported server URL scheme: {other}"
            )))
        }
    };
    url.set_scheme(&scheme)
        .map_err(|_| RcwError::InvalidConfig("failed to set websocket scheme".to_owned()))?;

    let base_path = url.path().trim_end_matches('/');
    let endpoint = endpoint.trim_start_matches('/');
    let joined = if base_path.is_empty() {
        format!("/{endpoint}")
    } else {
        format!("{base_path}/{endpoint}")
    };
    url.set_path(&joined);
    url.set_query(None);
    Ok(url.to_string())
}

pub fn parse_u64(value: &str) -> RcwResult<u64> {
    value
        .trim()
        .parse::<u64>()
        .map_err(|_| RcwError::InvalidConfig(format!("expected positive integer, got {value:?}")))
}

fn validate_totp_period(value: u64) -> RcwResult<u64> {
    if value == 0 {
        return Err(RcwError::InvalidConfig(
            "TOTP period must be greater than zero".to_owned(),
        ));
    }
    Ok(value)
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_http_to_ws_endpoint() {
        let url = ws_endpoint_url("https://example.com/base", "/ws/host").unwrap();
        assert_eq!(url, "wss://example.com/base/ws/host");
    }
}
