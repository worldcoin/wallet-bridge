use std::str::FromStr;

use aide::axum::{
    routing::{get, post},
    ApiRouter,
};
use axum::{
    extract::Path,
    http::{Method, StatusCode},
    Extension,
};
use axum_jsonschema::Json;
use redis::{aio::ConnectionManager, AsyncCommands, ExistenceCheck, SetExpiry, SetOptions};
use schemars::JsonSchema;
use std::str;
use tower_http::cors::{AllowHeaders, Any, CorsLayer};
use uuid::Uuid;

use crate::observability;
use crate::utils::{
    handle_redis_error, validate_request_id, RequestPayload, RequestStatus, EXPIRE_AFTER_SECONDS,
    REQ_STATUS_PREFIX,
};

const RES_PREFIX: &str = "res:";

#[derive(Debug, serde::Deserialize, serde::Serialize, JsonSchema)]
struct Response {
    status: RequestStatus,
    response: Option<RequestPayload>,
}

#[derive(Debug, serde::Serialize, JsonSchema)]
struct ResponseCreatedPayload {
    /// The unique identifier for the response
    request_id: String,
}

pub fn handler() -> ApiRouter {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_headers(AllowHeaders::any())
        .allow_methods([Method::GET, Method::PUT, Method::POST]); //TODO: PUT is required by the simulator but should not be included

    ApiRouter::new()
        .api_route(
            "/response/:request_id",
            get(get_response_handler)
                .head(has_response_status)
                .put(insert_response_handler)
                .layer(cors.clone()),
        )
        .api_route("/response", post(create_response).layer(cors))
}

async fn get_response_handler(
    Path(request_id): Path<String>,
    Extension(mut redis): Extension<ConnectionManager>,
) -> Result<Json<Response>, StatusCode> {
    let request_id = request_id.to_lowercase();
    let (response, current_status) = get_response(&mut redis, &request_id).await?;

    if let Some(current_status) = current_status {
        // Keep request identifiers in the pre-existing operational logs outside
        // the bounded root span created by `get_response`.
        tracing::info!(
            "Request {request_id} state transition: {} -> {}",
            current_status,
            RequestStatus::Completed
        );
    }

    Ok(response)
}

#[allow(clippy::too_many_lines)]
#[tracing::instrument(
    parent = None,
    name = "wallet_bridge.response.consume",
    skip_all,
    fields(
        idkit_flow_id = tracing::field::Empty,
        http.route = "/response/:request_id",
        queue_age_ms = tracing::field::Empty,
    )
)]
async fn get_response(
    redis: &mut ConnectionManager,
    request_id: &str,
) -> Result<(Json<Response>, Option<RequestStatus>), StatusCode> {
    validate_request_id(request_id)?;

    // MULTI/EXEC keeps the status, one-time response consumption, and flow
    // metadata snapshot consistent without requiring Redis Lua.
    let mut pipe = redis::pipe();
    pipe.atomic()
        .get(format!("{REQ_STATUS_PREFIX}{request_id}"))
        .get_del(format!("{RES_PREFIX}{request_id}"))
        .get(observability::flow_key(request_id));

    let (status, value, flow): (Option<String>, Option<Vec<u8>>, Option<String>) =
        pipe.query_async(redis).await.map_err(handle_redis_error)?;

    observability::record_idkit_flow_id(flow.as_deref());

    if let Some(value) = value {
        let current_status = status
            .and_then(|status| RequestStatus::from_str(&status).ok())
            .unwrap_or(RequestStatus::Retrieved);
        let response =
            serde_json::from_slice(&value).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        observability::observe_response_handoff(flow.as_deref());

        // Best-effort cleanup after GETDEL. The flow record remains
        // self-expiring if this command fails, and response delivery is
        // never turned into an error after the payload was consumed.
        let mut cleanup = redis::pipe();
        cleanup
            .atomic()
            .del(format!("{REQ_STATUS_PREFIX}{request_id}"))
            .del(observability::flow_key(request_id));
        if cleanup.query_async::<(u64, u64)>(redis).await.is_err() {
            tracing::warn!("Failed to clean up IDKit flow metadata");
        }

        return Ok((
            Json(Response {
                response,
                status: RequestStatus::Completed,
            }),
            Some(current_status),
        ));
    }

    // Return the current status for polling requests.
    let status = status.ok_or(StatusCode::NOT_FOUND)?;
    let status = RequestStatus::from_str(&status).map_err(|error| {
        tracing::error!("Failed to parse status: {error}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok((
        Json(Response {
            status,
            response: None,
        }),
        None,
    ))
}

async fn has_response_status(
    Path(request_id): Path<String>,
    Extension(mut redis): Extension<ConnectionManager>,
) -> StatusCode {
    let request_id = request_id.to_lowercase();
    if validate_request_id(&request_id).is_err() {
        return StatusCode::BAD_REQUEST;
    }

    let Ok(exists) = redis
        .exists::<_, bool>(format!("{REQ_STATUS_PREFIX}{request_id}"))
        .await
    else {
        return StatusCode::INTERNAL_SERVER_ERROR;
    };

    if exists {
        StatusCode::OK
    } else {
        StatusCode::NOT_FOUND
    }
}

async fn insert_response_handler(
    Path(request_id): Path<String>,
    Extension(mut redis): Extension<ConnectionManager>,
    Json(request): Json<RequestPayload>,
) -> Result<StatusCode, StatusCode> {
    let request_id = request_id.to_lowercase();
    let (status, current_status) = insert_response(&mut redis, &request_id, &request).await?;

    // Keep request identifiers in the pre-existing operational logs outside
    // the bounded root span created by `insert_response`.
    tracing::info!(
        "Request {request_id} state transition: {} -> {}",
        current_status,
        RequestStatus::Completed
    );

    Ok(status)
}

#[allow(clippy::too_many_lines)]
#[tracing::instrument(
    parent = None,
    name = "wallet_bridge.response.create",
    skip_all,
    fields(
        idkit_flow_id = tracing::field::Empty,
        http.route = "/response/:request_id",
    )
)]
async fn insert_response(
    redis: &mut ConnectionManager,
    request_id: &str,
    request: &RequestPayload,
) -> Result<(StatusCode, RequestStatus), StatusCode> {
    validate_request_id(request_id)?;

    // Snapshot the state and tracing metadata in one native Redis
    // transaction. The response payload still uses SET NX for concurrency.
    let mut pipe = redis::pipe();
    pipe.atomic()
        .get(format!("{REQ_STATUS_PREFIX}{request_id}"))
        .get(observability::flow_key(request_id));
    let (status, flow): (Option<String>, Option<String>) =
        pipe.query_async(redis).await.map_err(handle_redis_error)?;

    let current_status = status.and_then(|status| RequestStatus::from_str(&status).ok());
    let Some(current_status) = current_status else {
        return Err(StatusCode::BAD_REQUEST);
    };

    observability::record_idkit_flow_id(flow.as_deref());

    let payload = serde_json::to_vec(request).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Atomically store the response with TTL if not already set.
    let options = SetOptions::default()
        .conditional_set(ExistenceCheck::NX)
        .with_expiration(SetExpiry::EX(EXPIRE_AFTER_SECONDS));

    let set_ok: Option<String> = redis
        .set_options(format!("{RES_PREFIX}{request_id}"), payload, options)
        .await
        .map_err(handle_redis_error)?;

    if set_ok.is_none() {
        return Err(StatusCode::CONFLICT);
    }

    // Preserve the existing status semantics. Once this succeeds, metadata
    // failures below are strictly best effort.
    redis
        .del::<_, ()>(format!("{REQ_STATUS_PREFIX}{request_id}"))
        .await
        .map_err(handle_redis_error)?;

    observability::record_response_persisted(redis, request_id, flow.as_deref()).await;

    Ok((StatusCode::CREATED, current_status))
}

/// Create a new standalone response (World App initiates)
async fn create_response(
    Extension(mut redis): Extension<ConnectionManager>,
    Json(request): Json<RequestPayload>,
) -> Result<(StatusCode, Json<ResponseCreatedPayload>), StatusCode> {
    let request_id = Uuid::new_v4().to_string();

    tracing::info!("Processing POST /response: {request_id}");

    // Initialize status marker (will be deleted when IDKit retrieves response)
    redis
        .set_ex::<_, _, ()>(
            format!("{REQ_STATUS_PREFIX}{request_id}"),
            RequestStatus::Initialized.to_string(),
            EXPIRE_AFTER_SECONDS,
        )
        .await
        .map_err(handle_redis_error)?;

    tracing::info!(
        "Standalone response {request_id} state transition: new -> {}",
        RequestStatus::Initialized
    );

    // Store response payload with TTL
    redis
        .set_ex::<_, _, ()>(
            format!("{RES_PREFIX}{request_id}"),
            serde_json::to_vec(&request).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
            EXPIRE_AFTER_SECONDS,
        )
        .await
        .map_err(handle_redis_error)?;

    tracing::info!("Successfully processed POST /response: {request_id}");

    Ok((
        StatusCode::CREATED,
        Json(ResponseCreatedPayload { request_id }),
    ))
}
