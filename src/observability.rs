//! Correlation helpers for tracing a message across bridge handoffs.
//!
//! Flow identifiers deliberately carry no timing or client data. They only
//! connect bounded bridge spans and expire after a fixed correlation window.

use std::fmt;

use redis::{aio::ConnectionManager, AsyncCommands};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::{field, Span};
use uuid::Uuid;

use crate::utils::EXPIRE_AFTER_SECONDS;

/// Redis namespace used to carry a flow identifier to the response leg.
pub const FLOW_PREFIX: &str = "flow:";

const IDKIT_FLOW_ID_PREFIX: &str = "idkitflow_";

/// Flow correlation can span one full request TTL before consumption and one
/// full response TTL afterward. Allocate both 15-minute legs up front so
/// expiry stays deterministic and GET does not refresh observability state.
///
/// This 30-minute lifetime is an intentional exception to the bridge's usual
/// uniform TTL: payloads still expire after `EXPIRE_AFTER_SECONDS`.
const FLOW_EXPIRE_AFTER_SECONDS: u64 = EXPIRE_AFTER_SECONDS * 2;

/// Opaque correlation identifier shared by the spans in one `IDKit` flow.
///
/// The textual prefix distinguishes this value from request IDs and other
/// UUID-shaped identifiers without attaching any client-specific meaning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct IdkitFlowId(String);

impl IdkitFlowId {
    /// Generate a new prefixed correlation identifier.
    fn new() -> Self {
        Self(format!("{IDKIT_FLOW_ID_PREFIX}{}", Uuid::new_v4()))
    }

    /// Return the serialized representation stored in Redis and JSON.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn from_redis(value: &str) -> Option<Self> {
        let uuid = value.strip_prefix(IDKIT_FLOW_ID_PREFIX)?;
        Uuid::parse_str(uuid).ok()?;
        Some(Self(value.to_string()))
    }
}

impl fmt::Display for IdkitFlowId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Build the Redis key used to carry a flow identifier between route legs.
#[must_use]
pub fn flow_key(request_id: &str) -> String {
    format!("{FLOW_PREFIX}{request_id}")
}

/// Mint and persist a flow identifier for a newly created request.
///
/// # Errors
///
/// Returns `flow_id_write_failed` when Redis cannot store the identifier.
pub async fn mint_and_store_idkit_flow_id(
    redis: &mut ConnectionManager,
    request_id: &str,
) -> Result<IdkitFlowId, &'static str> {
    let idkit_flow_id = IdkitFlowId::new();
    record_idkit_flow_id(&idkit_flow_id);

    redis
        .set_ex::<_, _, ()>(
            flow_key(request_id),
            idkit_flow_id.as_str(),
            FLOW_EXPIRE_AFTER_SECONDS,
        )
        .await
        .map_err(|_| "flow_id_write_failed")?;

    Ok(idkit_flow_id)
}

/// Record the flow identifier observed while consuming a request.
pub fn record_request_handoff(value: Option<&str>) -> Option<IdkitFlowId> {
    let Some(value) = value else {
        tracing::warn!(
            outcome = "flow_id_missing",
            operation = "request_handoff",
            "Failed to observe IDKit flow ID"
        );
        return None;
    };
    let Some(idkit_flow_id) = IdkitFlowId::from_redis(value) else {
        tracing::warn!(
            outcome = "flow_id_invalid",
            operation = "request_handoff",
            "Failed to observe IDKit flow ID"
        );
        return None;
    };
    record_idkit_flow_id(&idkit_flow_id);

    Some(idkit_flow_id)
}

/// Record a response-side flow identifier when one exists.
///
/// Missing identifiers are expected for standalone and legacy response flows.
/// Malformed identifiers are reported but never block payload delivery.
pub fn record_response_handoff(value: Option<&str>) {
    let Some(value) = value else {
        return;
    };
    let Some(idkit_flow_id) = IdkitFlowId::from_redis(value) else {
        tracing::warn!(
            outcome = "flow_id_invalid",
            operation = "response_handoff",
            "Failed to observe IDKit flow ID"
        );
        return;
    };
    record_idkit_flow_id(&idkit_flow_id);
}

fn record_idkit_flow_id(idkit_flow_id: &IdkitFlowId) {
    Span::current().record("idkit_flow_id", field::display(idkit_flow_id));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idkit_flow_id_has_a_recognizable_prefix_and_uuid_suffix() {
        let flow_id = IdkitFlowId::new();
        let uuid = flow_id
            .as_str()
            .strip_prefix(IDKIT_FLOW_ID_PREFIX)
            .expect("flow ID should use the IDKit prefix");

        assert!(Uuid::parse_str(uuid).is_ok());
        assert_eq!(
            IdkitFlowId::from_redis(flow_id.as_str()),
            Some(flow_id.clone())
        );
        assert!(IdkitFlowId::from_redis("not-a-flow-id").is_none());
    }
}
