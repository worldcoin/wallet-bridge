use std::{fmt::Display, str::FromStr};

use axum::http::StatusCode;
use redis::RedisError;
use schemars::JsonSchema;

pub const EXPIRE_AFTER_SECONDS: u64 = 900; // Increasing to allow partner verifications.
pub const REQ_STATUS_PREFIX: &str = "req:status:";

/// Maximum length of a `request_id`, whether supplied by the client on
/// `POST /request` or extracted from a route path. Bounds Redis-key memory and
/// keeps URLs reasonable; 256 is more than enough for UUIDs (36), HKDF-derived
/// hex (64), and base64-shaped identifiers.
pub const REQUEST_ID_MAX_LEN: usize = 256;

/// Minimum length of a `request_id`. Anything shorter is almost certainly a
/// mistake — UUIDs are 36 chars, UUID-simple is 32, HKDF-derived hex is 64,
/// and base64url of 12 random bytes is 16. The bound doesn't enforce
/// cryptographic entropy on its own (a 16-char string of `aaaa…` passes),
/// but it stops trivial typos and obvious attacks; the RP is responsible
/// for picking high-entropy identifiers.
pub const REQUEST_ID_MIN_LEN: usize = 16;

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum RequestStatus {
    /// The request has been initiated by the client
    Initialized,
    /// The request has been retrieved by World App
    Retrieved,
    /// The request has received a response from World App
    Completed,
}

impl Display for RequestStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Retrieved => write!(f, "retrieved"),
            Self::Completed => write!(f, "completed"),
            Self::Initialized => write!(f, "initialized"),
        }
    }
}

impl FromStr for RequestStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "initialized" => Ok(Self::Initialized),
            "retrieved" => Ok(Self::Retrieved),
            "completed" => Ok(Self::Completed),
            _ => Err(format!("Invalid status: {s}")),
        }
    }
}

#[derive(Debug, serde::Deserialize, serde::Serialize, JsonSchema)]
pub struct RequestPayload {
    /// The initialization vector for the encrypted payload
    iv: String,
    /// The encrypted payload
    payload: String,
}

impl RequestPayload {
    pub const fn new(iv: String, payload: String) -> Self {
        Self { iv, payload }
    }
}

#[allow(clippy::needless_pass_by_value)]
pub fn handle_redis_error(e: RedisError) -> StatusCode {
    tracing::error!("Redis error: {e}");
    StatusCode::INTERNAL_SERVER_ERROR
}

/// Validate a `request_id` (path param or client-supplied body field):
/// length between `REQUEST_ID_MIN_LEN` and `REQUEST_ID_MAX_LEN`, charset
/// limited to URL-path-safe ASCII (alphanumeric plus `-`, `_`, `.`).
///
/// `:` is intentionally excluded from the charset: an ID like `status:foo`
/// would round-trip into the Redis key `req:status:foo`, colliding with the
/// status-namespace key `req:status:foo` that the bridge writes for some
/// other request. Keeping the charset disjoint from the literal `:` prevents
/// that overlap by construction.
pub fn validate_request_id(id: &str) -> Result<(), StatusCode> {
    if id.len() < REQUEST_ID_MIN_LEN || id.len() > REQUEST_ID_MAX_LEN {
        return Err(StatusCode::BAD_REQUEST);
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(())
}
