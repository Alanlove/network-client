//! Cross-cutting error type and small helpers.

use thiserror::Error;

/// Internal error taxonomy (design doc §55). User-facing text is produced on
/// the UI side from the stable code returned in `Response.code` /
/// `error_code`.
#[derive(Error, Debug)]
pub enum SvcError {
    #[error("invalid state transition: {0} -> {1}")]
    InvalidState(String, String),
    #[error("config invalid: {0}")]
    ConfigInvalid(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("node not found: {0}")]
    NodeNotFound(String),
    #[error("subscription error: {0}")]
    Subscription(String),
    #[error("parse error: {0}")]
    Parse(String),
    #[error("core error: {0}")]
    Core(String),
    #[error("tun error: {0}")]
    Tun(String),
    #[error("route error: {0}")]
    Route(String),
    #[error("dns failed: {0}")]
    Dns(String),
    #[error("proxy failed: {0}")]
    Proxy(String),
    #[error("network timeout")]
    Timeout,
    #[error("authentication failed: {0}")]
    Auth(String),
    #[error("already running: {0}")]
    AlreadyRunning(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
    #[error("db: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("{0}")]
    Other(String),
}

pub type SvcResult<T> = Result<T, SvcError>;

impl SvcError {
    /// Stable numeric code carried by the IPC envelope.
    pub fn code(&self) -> i32 {
        match self {
            SvcError::InvalidState(..) => 409,
            SvcError::ConfigInvalid(..) | SvcError::Parse(..) | SvcError::Json(..) => 422,
            SvcError::NotFound(..) | SvcError::NodeNotFound(..) => 404,
            SvcError::Auth(..) => 401,
            SvcError::Timeout => 504,
            SvcError::AlreadyRunning(..) => 409,
            SvcError::Subscription(..) => 600,
            SvcError::Core(..) => 601,
            SvcError::Tun(..) => 602,
            SvcError::Route(..) => 603,
            SvcError::Dns(..) => 604,
            SvcError::Proxy(..) => 605,
            SvcError::Io(..) | SvcError::Http(..) | SvcError::Db(..) => 500,
            SvcError::Other(..) => 500,
        }
    }
}

/// Mask a URL's userinfo / token for safe logging: `https://host/sub/****`.
pub fn mask_url(raw: &str) -> String {
    match raw.find("://") {
        Some(idx) => {
            let rest = &raw[idx + 3..];
            match rest.find('/') {
                Some(slash) => format!("{}://{}/****", &raw[..idx], &rest[..slash]),
                None => format!("{}://{}/****", &raw[..idx], rest),
            }
        }
        None => "****".to_string(),
    }
}

pub fn now_millis() -> i64 {
    chrono::Local::now().timestamp_millis()
}

pub fn today_midnight_millis() -> i64 {
    use chrono::{Datelike, Local, TimeZone, Timelike};
    let now = Local::now();
    match Local.with_ymd_and_hms(now.year(), now.month(), now.day(), 0, 0, 0).single() {
        Some(dt) => dt.timestamp_millis(),
        None => {
            // Ambiguous DST midnight — fall back to arithmetic.
            now.timestamp_millis() - (now.num_seconds_from_midnight() as i64 * 1000)
        }
    }
}
