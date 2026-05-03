use std::{env, fmt::Display, str::FromStr};

use axum::http::StatusCode;
use base64::{engine::general_purpose::STANDARD, Engine};
use rand::RngCore;
use redis::RedisError;
use schemars::JsonSchema;
use sha2::{Digest, Sha256};

pub const EXPIRE_AFTER_SECONDS: u64 = 900; // Increasing to allow partner verifications.
pub const REQ_STATUS_PREFIX: &str = "req:status:";
pub const CODE_IDX_PREFIX: &str = "code:idx:";

const DEFAULT_CODE_TTL_SECONDS: u64 = 600;

/// TTL applied to an unredeemed invite code. Defaults to 10 minutes; overridable
/// via `CODE_TTL_SECONDS` so integration tests can exercise expiry without sleeping
/// for the full production window.
pub fn code_ttl_seconds() -> u64 {
    env::var("CODE_TTL_SECONDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_CODE_TTL_SECONDS)
}

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

/// Generate a fresh 256-bit random token, base64-encoded.
/// Returned to a caller exactly once; only its SHA-256 hash is persisted.
pub fn random_token() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    STANDARD.encode(bytes)
}

/// SHA-256 hex digest. Used to store secret tokens (`session_nonce`) at rest
/// so a Redis snapshot leak doesn't compromise live sessions.
pub fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Reject inputs that don't look like standard base64 of plausible length.
/// Bounds are byte-counts of the decoded value, inclusive.
pub fn validate_base64(s: &str, min_bytes: usize, max_bytes: usize) -> Result<(), StatusCode> {
    let decoded = STANDARD.decode(s).map_err(|_| StatusCode::BAD_REQUEST)?;
    if decoded.len() < min_bytes || decoded.len() > max_bytes {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(())
}
