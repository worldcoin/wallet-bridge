use axum::{
    extract::Path,
    http::{HeaderMap, StatusCode},
    Extension,
};
use axum_jsonschema::Json;
use redis::{aio::ConnectionManager, AsyncCommands};
use schemars::JsonSchema;

use crate::{
    observability::{self, IdkitFlowId},
    utils::{
        handle_redis_error, validate_request_id, RequestPayload, RequestStatus,
        EXPIRE_AFTER_SECONDS, REQ_STATUS_PREFIX,
    },
};

use super::REQ_PREFIX;

/// Capability-negotiation header for backwards-compatible flow correlation.
///
/// Existing request consumers may reject unknown JSON fields. Only consumers
/// that send this header receive `idkit_flow_id`; callers that omit it retain
/// the legacy `{iv, payload}` response shape.
const ACCEPT_IDKIT_FLOW_ID_HEADER: &str = "accept-idkit-flow-id";

#[derive(Debug, serde::Serialize, JsonSchema)]
pub(super) struct RequestResponse {
    /// The initialization vector for the encrypted payload.
    iv: String,
    /// The encrypted payload.
    payload: String,
    /// Correlates this bridge handoff with compatible `IDKit` telemetry.
    #[serde(skip_serializing_if = "Option::is_none")]
    idkit_flow_id: Option<IdkitFlowId>,
}

#[tracing::instrument(
    parent = None,
    name = "message_bridge.request.consume",
    skip_all,
    fields(
        idkit_flow_id = tracing::field::Empty,
        http.route = "/request/:request_id",
    )
)]
pub(super) async fn handler(
    Path(request_id): Path<String>,
    Extension(mut redis): Extension<ConnectionManager>,
    headers: HeaderMap,
) -> Result<Json<RequestResponse>, StatusCode> {
    let request_id = request_id.to_lowercase();
    validate_request_id(&request_id)?;

    let mut pipe = redis::pipe();
    pipe.atomic()
        .get_del(format!("{REQ_PREFIX}{request_id}"))
        .get(observability::flow_key(&request_id));

    let (value, idkit_flow_id): (Option<Vec<u8>>, Option<String>) = pipe
        .query_async(&mut redis)
        .await
        .map_err(handle_redis_error)?;

    let value = value.ok_or(StatusCode::NOT_FOUND)?;
    let payload: RequestPayload =
        serde_json::from_slice(&value).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    redis
        .set_ex::<_, _, ()>(
            format!("{REQ_STATUS_PREFIX}{request_id}"),
            RequestStatus::Retrieved.to_string(),
            EXPIRE_AFTER_SECONDS,
        )
        .await
        .map_err(handle_redis_error)?;

    let idkit_flow_id = observability::record_request_handoff(idkit_flow_id.as_deref());

    telemetry_batteries::reexports::metrics::counter!("message_bridge.request_consumed")
        .increment(1);

    Ok(Json(RequestResponse {
        iv: payload.iv,
        payload: payload.payload,
        idkit_flow_id: headers
            .contains_key(ACCEPT_IDKIT_FLOW_ID_HEADER)
            .then_some(idkit_flow_id)
            .flatten(),
    }))
}
