use std::time::{SystemTime, UNIX_EPOCH};

use redis::{aio::ConnectionManager, AsyncCommands};
use serde::{Deserialize, Serialize};
use tracing::{field, Span};
use uuid::Uuid;

use crate::utils::EXPIRE_AFTER_SECONDS;

pub const FLOW_PREFIX: &str = "flow:";

const HANDOFF_DURATION_METRIC: &str = "wallet_bridge.handoff.duration_ms";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IDKitFlowMetadata {
    pub idkit_flow_id: Uuid,
    pub request_persisted_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_persisted_at_ms: Option<u64>,
}

#[derive(Debug)]
enum FlowMetadataSnapshot {
    Present(IDKitFlowMetadata),
    Missing,
    Invalid,
}

impl FlowMetadataSnapshot {
    fn from_redis(value: Option<&str>) -> Self {
        value.map_or(Self::Missing, |value| {
            serde_json::from_str(value)
                .map(Self::Present)
                .unwrap_or(Self::Invalid)
        })
    }
}

fn now_ms() -> Option<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| duration.as_millis().try_into().ok())
}

const fn queue_age_ms(persisted_at_ms: u64, observed_at_ms: u64) -> Option<u64> {
    observed_at_ms.checked_sub(persisted_at_ms)
}

pub fn flow_key(request_id: &str) -> String {
    format!("{FLOW_PREFIX}{request_id}")
}

async fn persist_flow_metadata(
    redis: &mut ConnectionManager,
    request_id: &str,
    metadata: &IDKitFlowMetadata,
) -> Result<(), &'static str> {
    let value = serde_json::to_string(metadata).map_err(|_| "metadata_serialize_failed")?;

    redis
        .set_ex::<_, _, ()>(flow_key(request_id), value, EXPIRE_AFTER_SECONDS)
        .await
        .map_err(|_| "metadata_write_failed")
}

pub async fn mint_and_store_idkit_flow_id(
    redis: &mut ConnectionManager,
    request_id: &str,
) -> Result<Uuid, &'static str> {
    let idkit_flow_id = Uuid::new_v4();
    Span::current().record("idkit_flow_id", field::display(idkit_flow_id));

    let request_persisted_at_ms = now_ms().ok_or("clock_unavailable")?;
    let metadata = IDKitFlowMetadata {
        idkit_flow_id,
        request_persisted_at_ms,
        response_persisted_at_ms: None,
    };

    persist_flow_metadata(redis, request_id, &metadata).await?;

    Ok(idkit_flow_id)
}

pub async fn observe_request_handoff(
    redis: &mut ConnectionManager,
    request_id: &str,
    value: Option<&str>,
) -> Option<Uuid> {
    let metadata = metadata_or_warn(value, "request_handoff")?;
    let idkit_flow_id = metadata.idkit_flow_id;

    observe_handoff(&metadata, Some(metadata.request_persisted_at_ms), "request");

    // Refresh the metadata TTL for the response leg. This is intentionally
    // best effort: observability must never break a successfully consumed
    // encrypted request.
    if let Err(outcome) = persist_flow_metadata(redis, request_id, &metadata).await {
        tracing::warn!(
            outcome,
            operation = "request_handoff",
            "Failed to persist IDKit flow metadata"
        );
    }

    Some(idkit_flow_id)
}

fn metadata_or_warn(value: Option<&str>, operation: &'static str) -> Option<IDKitFlowMetadata> {
    match FlowMetadataSnapshot::from_redis(value) {
        FlowMetadataSnapshot::Present(metadata) => Some(metadata),
        FlowMetadataSnapshot::Missing => {
            tracing::warn!(
                outcome = "metadata_missing",
                operation,
                "Failed to observe IDKit flow metadata"
            );
            None
        }
        FlowMetadataSnapshot::Invalid => {
            tracing::warn!(
                outcome = "metadata_invalid",
                operation,
                "Failed to observe IDKit flow metadata"
            );
            None
        }
    }
}

fn observe_handoff(metadata: &IDKitFlowMetadata, persisted_at_ms: Option<u64>, leg: &'static str) {
    record_flow(metadata);

    let age_ms = persisted_at_ms
        .and_then(|persisted_at_ms| now_ms().and_then(|now| queue_age_ms(persisted_at_ms, now)));
    let Some(age_ms) = age_ms else {
        tracing::warn!(
            outcome = "clock_unavailable_or_skewed",
            operation = leg,
            "Failed to observe IDKit flow metadata"
        );
        return;
    };

    record_queue_age(age_ms);
    record_handoff_duration(leg, age_ms);
}

fn record_flow(metadata: &IDKitFlowMetadata) {
    Span::current().record("idkit_flow_id", field::display(metadata.idkit_flow_id));
}

fn record_queue_age(age_ms: u64) {
    Span::current().record("queue_age_ms", age_ms);
}

fn record_handoff_duration(leg: &'static str, duration_ms: u64) {
    let duration_ms = u32::try_from(duration_ms).unwrap_or(u32::MAX);
    telemetry_batteries::reexports::metrics::histogram!(
        HANDOFF_DURATION_METRIC,
        "leg" => leg
    )
    .record(duration_ms);
}

#[cfg(test)]
mod tests {
    use metrics_util::debugging::DebuggingRecorder;

    use super::*;

    #[test]
    fn queue_age_uses_checked_subtraction() {
        assert_eq!(queue_age_ms(100, 140), Some(40));
        assert_eq!(queue_age_ms(140, 100), None);
    }

    #[test]
    fn idkit_flow_metadata_is_backward_compatible_with_missing_response_timestamp() {
        let flow_id = Uuid::new_v4();
        let raw = format!(r#"{{"idkit_flow_id":"{flow_id}","request_persisted_at_ms":123}}"#);
        let metadata: IDKitFlowMetadata = serde_json::from_str(&raw).unwrap();

        assert_eq!(metadata.idkit_flow_id, flow_id);
        assert_eq!(metadata.request_persisted_at_ms, 123);
        assert_eq!(metadata.response_persisted_at_ms, None);
    }

    #[test]
    fn handoff_duration_uses_only_the_bounded_leg_tag() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();

        telemetry_batteries::reexports::metrics::with_local_recorder(&recorder, || {
            record_handoff_duration("request", 42);
        });

        let snapshot = snapshotter.snapshot();
        let metrics = snapshot.into_vec();
        assert_eq!(metrics.len(), 1);

        for (composite_key, _, _, _) in metrics {
            let key = composite_key.key();
            let labels: Vec<_> = key
                .labels()
                .map(|label| (label.key(), label.value()))
                .collect();

            assert_eq!(key.name(), HANDOFF_DURATION_METRIC);
            assert_eq!(labels, vec![("leg", "request")]);
        }
    }
}
