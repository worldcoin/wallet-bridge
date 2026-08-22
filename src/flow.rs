//! `IDKit` flow tracing: minting, persistence, and telemetry emission.
//!
//! The bridge mints an `idkit_flow_id` when a request is created and stores it
//! in Redis (`flow:<request_id>`) alongside the request itself, with the same
//! uniform TTL. The `request_id -> idkit_flow_id` mapping lives *only* in
//! Redis: the sanitized spans emitted here carry the flow id but never the
//! request id, raw URL paths, or payload contents, so the two identifiers
//! cannot be joined outside the bridge. Metrics carry only bounded tags
//! (`environment`, `leg`) — never the flow id.
//!
//! Everything in this module is best-effort telemetry: failures are logged and
//! swallowed so the payload handoff path never fails because of tracing.
//!
//! The four spans are separate short traces correlated by `idkit_flow_id`, not
//! one long-lived trace. The handoff *durations* live in the
//! `wallet_bridge.handoff.duration_ms` distribution:
//! - `leg:request` — durable request storage to successful consumption. This
//!   includes QR/deep-link and user-open time, so it is a handoff-freshness
//!   signal rather than HTTP server latency.
//! - `leg:response` — durable response storage to the poll that consumes it,
//!   which includes the `IDKit` poll interval.

use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use redis::{aio::ConnectionManager, AsyncCommands};
use serde::{Deserialize, Serialize};
use telemetry_batteries::reexports::metrics;
use tracing::Instrument;
use uuid::Uuid;

use crate::utils::EXPIRE_AFTER_SECONDS;

/// Redis key prefix for flow metadata. Disjoint from `req:`/`res:` so the
/// GETDEL single-use semantics of the payload keys are untouched.
pub const FLOW_PREFIX: &str = "flow:";
/// Prefix on every minted flow id, so the identifier is self-describing when
/// it shows up in spans or downstream telemetry.
pub const IDKIT_FLOW_ID_PREFIX: &str = "idkitflow_";

const HANDOFF_METRIC: &str = "wallet_bridge.handoff.duration_ms";

/// Flow metadata persisted next to a request for the lifetime of the flow.
///
/// Not part of any API schema — this is an internal Redis value. Timestamps
/// are wall-clock epoch milliseconds; handoff durations are seconds-to-minutes
/// (QR scan time, poll intervals), so NTP-level skew between pods is noise.
#[derive(Debug, Serialize, Deserialize)]
pub struct FlowMetadata {
    pub idkit_flow_id: String,
    pub request_persisted_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_persisted_at_ms: Option<u64>,
}

/// The Redis key holding [`FlowMetadata`] for a request.
#[must_use]
pub fn key(request_id: &str) -> String {
    format!("{FLOW_PREFIX}{request_id}")
}

/// Mint a flow id and persist fresh metadata for a newly created request.
///
/// Called after the request payload is durably stored. Emits the
/// `wallet_bridge.request.create` span and, on successful persistence, the
/// `request_created` counter.
pub async fn record_request_created(
    redis: &mut ConnectionManager,
    request_id: &str,
    route: &'static str,
    http_status: u64,
) {
    let flow = FlowMetadata {
        idkit_flow_id: format!("{IDKIT_FLOW_ID_PREFIX}{}", Uuid::new_v4()),
        request_persisted_at_ms: now_ms(),
        response_persisted_at_ms: None,
    };

    let span = tracing::info_span!(
        "wallet_bridge.request.create",
        http.route = route,
        http.status_code = http_status,
        redis.outcome = tracing::field::Empty,
        idkit_flow_id = %flow.idkit_flow_id,
    );

    async {
        if write(redis, request_id, &flow).await {
            increment("wallet_bridge.request_created");
        }
    }
    .instrument(span)
    .await;
}

/// Record the consumption of a request payload from its flow metadata.
///
/// The metadata bytes come from the caller's pipeline (no extra round trip).
/// Returns the persisted flow id for the caller to surface to opted-in
/// clients.
///
/// Emits the `wallet_bridge.request.consume` span, the `request_consumed`
/// counter, and the `leg:request` handoff distribution. A missing or
/// unparseable blob (expired, or written by a pre-flow bridge) skips all
/// telemetry — the payload handoff itself already succeeded.
pub fn record_request_consumed(flow_raw: Option<&[u8]>) -> Option<String> {
    let flow = parse(flow_raw?)?;
    let queue_age_ms = now_ms().saturating_sub(flow.request_persisted_at_ms);

    tracing::info_span!(
        "wallet_bridge.request.consume",
        http.route = "/request/:request_id",
        http.status_code = 200_u64,
        queue_age_ms,
        idkit_flow_id = %flow.idkit_flow_id,
    );

    record_handoff("request", queue_age_ms);
    increment("wallet_bridge.request_consumed");

    Some(flow.idkit_flow_id)
}

/// Stamp `response_persisted_at_ms` into the flow metadata and refresh its TTL,
/// after a response payload is durably stored.
///
/// Emits the `wallet_bridge.response.create` span and, on successful
/// persistence, the `response_created` counter. A missing flow key (expired,
/// or a standalone response with no request leg) skips all telemetry.
pub async fn record_response_created(redis: &mut ConnectionManager, request_id: &str) {
    let raw: Option<Vec<u8>> = match redis.get(key(request_id)).await {
        Ok(raw) => raw,
        Err(e) => {
            tracing::warn!("Failed to read flow metadata: {e}");
            return;
        }
    };
    let Some(mut flow) = raw.as_deref().and_then(parse) else {
        return;
    };
    flow.response_persisted_at_ms = Some(now_ms());

    let span = tracing::info_span!(
        "wallet_bridge.response.create",
        http.route = "/response/:request_id",
        http.status_code = 201_u64,
        redis.outcome = tracing::field::Empty,
        idkit_flow_id = %flow.idkit_flow_id,
    );

    async {
        if write(redis, request_id, &flow).await {
            increment("wallet_bridge.response_created");
        }
    }
    .instrument(span)
    .await;
}

/// Consume (GETDEL) the flow metadata after a response payload is returned to
/// the poller — the end of the flow's life.
///
/// Emits the `wallet_bridge.response.consume` span, the `response_consumed`
/// counter, and the `leg:response` handoff distribution. The distribution is
/// skipped if the response-creation stamp never landed (best-effort write
/// failed earlier); the flow id itself is never returned to the RP.
pub async fn record_response_consumed(redis: &mut ConnectionManager, request_id: &str) {
    let raw: Option<Vec<u8>> = match redis.get_del(key(request_id)).await {
        Ok(raw) => raw,
        Err(e) => {
            tracing::warn!("Failed to consume flow metadata: {e}");
            return;
        }
    };
    let Some(flow) = raw.as_deref().and_then(parse) else {
        return;
    };
    let queue_age_ms = flow
        .response_persisted_at_ms
        .map(|persisted| now_ms().saturating_sub(persisted));

    tracing::info_span!(
        "wallet_bridge.response.consume",
        http.route = "/response/:request_id",
        http.status_code = 200_u64,
        queue_age_ms,
        idkit_flow_id = %flow.idkit_flow_id,
    );

    if let Some(duration) = queue_age_ms {
        record_handoff("response", duration);
    }
    increment("wallet_bridge.response_consumed");
}

/// Add a read of the flow metadata plus a TTL refresh to an existing pipeline.
///
/// The refresh keeps the flow key alive for the response leg: `GET /request`
/// already refreshes the status key TTL, and without a matching refresh here
/// the flow key would expire mid-flow while the status key survives. The read
/// lands as one `Option<Vec<u8>>` in the pipeline's result tuple (the expire
/// is `ignore()`d).
pub fn pipe_read_and_refresh(pipe: &mut redis::Pipeline, request_id: &str) {
    let key = key(request_id);
    pipe.get(&key);
    pipe.expire(&key, ttl_seconds()).ignore();
}

/// Persist flow metadata under the uniform TTL. Returns whether the write
/// landed; failures are logged, never propagated.
async fn write(redis: &mut ConnectionManager, request_id: &str, flow: &FlowMetadata) -> bool {
    let Ok(bytes) = serde_json::to_vec(flow) else {
        tracing::warn!("Failed to serialize flow metadata");
        record_outcome("error");
        return false;
    };
    match redis
        .set_ex::<_, _, ()>(key(request_id), bytes, EXPIRE_AFTER_SECONDS)
        .await
    {
        Ok(()) => {
            record_outcome("ok");
            true
        }
        Err(e) => {
            tracing::warn!("Failed to persist flow metadata: {e}");
            record_outcome("error");
            false
        }
    }
}

/// Record the `redis.outcome` attribute on the enclosing create span.
fn record_outcome(outcome: &str) {
    tracing::Span::current().record("redis.outcome", outcome);
}

fn parse(raw: &[u8]) -> Option<FlowMetadata> {
    match serde_json::from_slice(raw) {
        Ok(flow) => Some(flow),
        Err(e) => {
            tracing::warn!("Failed to parse flow metadata: {e}");
            None
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

fn ttl_seconds() -> i64 {
    i64::try_from(EXPIRE_AFTER_SECONDS).unwrap_or(i64::MAX)
}

/// Deployment environment tag for metrics, from the `ENVIRONMENT` env var
/// (mirroring the normalization in the request router).
fn environment() -> &'static str {
    static ENVIRONMENT: OnceLock<String> = OnceLock::new();
    ENVIRONMENT.get_or_init(|| {
        std::env::var("ENVIRONMENT")
            .map_or_else(|_| "unknown".to_string(), |e| e.trim().to_lowercase())
    })
}

fn increment(counter: &'static str) {
    metrics::counter!(counter, "environment" => environment()).increment(1);
}

#[allow(clippy::cast_precision_loss)] // handoff durations sit far below 2^52 ms
fn record_handoff(leg: &'static str, duration_ms: u64) {
    metrics::histogram!(HANDOFF_METRIC, "leg" => leg, "environment" => environment())
        .record(duration_ms as f64);
}
