use thiserror::Error;

pub type RcwResult<T> = Result<T, RcwError>;

#[derive(Debug, Error)]
pub enum RcwError {
    #[error("missing required configuration: {0}")]
    MissingConfig(&'static str),
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("url error: {0}")]
    Url(#[from] url::ParseError),
    #[error("{0}")]
    Other(String),
}
