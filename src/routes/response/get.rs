use std::str::FromStr;

use axum::{extract::Path, http::StatusCode, Extension};
use axum_jsonschema::Json;
use redis::aio::ConnectionManager;
use schemars::JsonSchema;

use crate::{
    observability,
    utils::{
        handle_redis_error, validate_request_id, RequestPayload, RequestStatus, REQ_STATUS_PREFIX,
    },
};

use super::RES_PREFIX;

#[derive(Debug, serde::Deserialize, serde::Serialize, JsonSchema)]
pub(super) struct Response {
    status: RequestStatus,
    response: Option<RequestPayload>,
}

#[tracing::instrument(
    parent = None,
    name = "wallet_bridge.response.consume",
    skip_all,
    fields(
        idkit_flow_id = tracing::field::Empty,
        http.route = "/response/:request_id",
    )
)]
pub(super) async fn handler(
    Path(request_id): Path<String>,
    Extension(mut redis): Extension<ConnectionManager>,
) -> Result<Json<Response>, StatusCode> {
    let request_id = request_id.to_lowercase();
    validate_request_id(&request_id)?;

    // Read the correlation key without consuming it while the response is
    // still being polled. Successful response delivery cleans it up below.
    let mut pipe = redis::pipe();
    pipe.atomic()
        .get(format!("{REQ_STATUS_PREFIX}{request_id}"))
        .get_del(format!("{RES_PREFIX}{request_id}"))
        .get(observability::flow_key(&request_id));

    let (status, value, flow): (Option<String>, Option<Vec<u8>>, Option<String>) = pipe
        .query_async(&mut redis)
        .await
        .map_err(handle_redis_error)?;

    observability::record_response_handoff(flow.as_deref());

    if let Some(value) = value {
        let response =
            serde_json::from_slice(&value).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        // Cleanup is best effort after GETDEL: the correlation key has a
        // fixed expiry and response delivery must not fail because of it.
        let mut cleanup = redis::pipe();
        cleanup
            .atomic()
            .del(format!("{REQ_STATUS_PREFIX}{request_id}"))
            .del(observability::flow_key(&request_id));
        if cleanup.query_async::<(u64, u64)>(&mut redis).await.is_err() {
            tracing::warn!(
                outcome = "flow_id_cleanup_failed",
                operation = "response_handoff",
                "Failed to clean up IDKit flow ID"
            );
        }

        return Ok(Json(Response {
            response,
            status: RequestStatus::Completed,
        }));
    }

    let status = status.ok_or(StatusCode::NOT_FOUND)?;
    let status = RequestStatus::from_str(&status).map_err(|error| {
        tracing::error!("Failed to parse status: {error}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(Json(Response {
        status,
        response: None,
    }))
}
