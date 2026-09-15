//! Correlation helpers for tracing a message across bridge handoffs.
//!
//! Records request/response create and consume spans and counters. Flow
//! identifiers connect those spans so handoff latency can be measured. A
//! client-reported `slo_metric` flag is relayed onto those counters as a
//! `slo_metric:{bool}` tag, uninterpreted.

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

/// Covers the request wait, app processing, and response wait windows.
const FLOW_EXPIRE_AFTER_SECONDS: u64 = EXPIRE_AFTER_SECONDS * 3;

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

/// Redis namespace carrying the `slo_metric` flag between legs.
const SLO_METRIC_PREFIX: &str = "slo_metric:";

#[must_use]
pub fn slo_metric_key(request_id: &str) -> String {
    format!("{SLO_METRIC_PREFIX}{request_id}")
}

/// Write-only-if-true: absence means "not in cohort". Best-effort — a write
/// failure just degrades the `slo_metric` tag to `false` for this flow.
pub async fn store_slo_metric_flag(
    redis: &mut ConnectionManager,
    request_id: &str,
    in_cohort: bool,
) {
    if !in_cohort {
        return;
    }
    if let Err(error) = redis
        .set_ex::<_, _, ()>(slo_metric_key(request_id), "1", FLOW_EXPIRE_AFTER_SECONDS)
        .await
    {
        tracing::warn!(
            outcome = "slo_metric_write_failed",
            operation = "request_handoff",
            "Failed to persist slo_metric cohort flag: {error}"
        );
    }
}

/// Render the `slo_metric` tag value for a `message_bridge.*` counter.
#[must_use]
pub const fn slo_metric_tag(in_cohort: bool) -> &'static str {
    if in_cohort {
        "true"
    } else {
        "false"
    }
}

/// Redis namespace carrying the client's `platform` between legs.
const PLATFORM_PREFIX: &str = "platform:";

#[must_use]
pub fn platform_key(request_id: &str) -> String {
    format!("{PLATFORM_PREFIX}{request_id}")
}

/// Best-effort — a write failure just degrades the `platform` tag to
/// `unknown` for this flow.
pub async fn store_platform(
    redis: &mut ConnectionManager,
    request_id: &str,
    platform: Option<&str>,
) {
    let Some(platform) = platform else {
        return;
    };
    if let Err(error) = redis
        .set_ex::<_, _, ()>(
            platform_key(request_id),
            platform,
            FLOW_EXPIRE_AFTER_SECONDS,
        )
        .await
    {
        tracing::warn!(
            outcome = "platform_write_failed",
            operation = "request_handoff",
            "Failed to persist platform: {error}"
        );
    }
}

/// Render the `platform` tag value for a `message_bridge.*` counter. Owned
/// because the counter macro's tag values must outlive the request.
#[must_use]
pub fn platform_tag(platform: Option<&str>) -> String {
    platform.unwrap_or("unknown").to_string()
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

    #[test]
    fn slo_metric_tag_reflects_cohort_membership() {
        assert_eq!(slo_metric_tag(true), "true");
        assert_eq!(slo_metric_tag(false), "false");
    }

    #[test]
    fn platform_tag_falls_back_to_unknown() {
        assert_eq!(platform_tag(Some("ios")), "ios");
        assert_eq!(platform_tag(None), "unknown");
    }
}
