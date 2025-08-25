use std::{fmt::Display, str::FromStr};

use axum::http::StatusCode;
use redis::RedisError;
use schemars::JsonSchema;

pub const EXPIRE_AFTER_SECONDS: u64 = 900; // Increasing to allow partner verifications.
pub const REQ_STATUS_PREFIX: &str = "req:status:";

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

#[derive(Debug, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
pub struct PutRequestPayload {
    /// Client-specified request id
    pub(crate) id: String,
    /// IV and encrypted payload
    #[serde(flatten)]
    payload: RequestPayload,
}

#[allow(clippy::needless_pass_by_value)]
pub fn handle_redis_error(e: RedisError) -> StatusCode {
    tracing::error!("Redis error: {e}");
    StatusCode::INTERNAL_SERVER_ERROR
}
