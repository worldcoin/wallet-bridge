use std::str::FromStr;

use aide::axum::{
    routing::{get, post},
    ApiRouter,
};
use axum::{
    extract::Path,
    http::{HeaderMap, Method, StatusCode},
    Extension,
};
use axum_jsonschema::Json;
use redis::{aio::ConnectionManager, AsyncCommands, ExistenceCheck, SetExpiry, SetOptions};
use schemars::JsonSchema;
use std::str;
use tower_http::cors::{AllowHeaders, Any, CorsLayer};
use uuid::Uuid;

use crate::utils::{
    constant_time_eq, handle_redis_error, sha256_hex, RequestPayload, RequestStatus,
    EXPIRE_AFTER_SECONDS, REQ_NONCE_PREFIX, REQ_STATUS_PREFIX,
};

const SESSION_NONCE_HEADER: &str = "x-session-nonce";

const RES_PREFIX: &str = "res:";

#[derive(Debug, serde::Deserialize, serde::Serialize, JsonSchema)]
struct Response {
    status: RequestStatus,
    response: Option<RequestPayload>,
}

#[derive(Debug, serde::Serialize, JsonSchema)]
struct ResponseCreatedPayload {
    /// The unique identifier for the response
    request_id: Uuid,
}

pub fn handler() -> ApiRouter {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_headers(AllowHeaders::any())
        .allow_methods([Method::GET, Method::PUT, Method::POST]); //TODO: PUT is required by the simulator but should not be included

    ApiRouter::new()
        .api_route(
            "/response/:request_id",
            get(get_response)
                .head(has_response_status)
                .put(insert_response)
                .layer(cors.clone()),
        )
        .api_route("/response", post(create_response).layer(cors))
}

/// Code-variant gate: if a `session_nonce` hash is stored for this `request_id`,
/// require an `X-Session-Nonce` header whose SHA-256 matches it. Legacy
/// (non-code-variant) requests have no such row and are unaffected.
///
/// On success, returns the stored hash (or `None` for legacy requests) so
/// the caller can decide whether to clean up the gate row after consumption.
async fn enforce_session_nonce_gate(
    request_id: &Uuid,
    headers: &HeaderMap,
    redis: &mut ConnectionManager,
) -> Result<Option<String>, StatusCode> {
    let stored: Option<String> = redis
        .get(format!("{REQ_NONCE_PREFIX}{request_id}"))
        .await
        .map_err(handle_redis_error)?;

    if let Some(stored_hash) = stored.as_deref() {
        let presented = headers
            .get(SESSION_NONCE_HEADER)
            .and_then(|h| h.to_str().ok())
            .ok_or(StatusCode::UNAUTHORIZED)?;
        let presented_hash = sha256_hex(presented);
        if !constant_time_eq(presented_hash.as_bytes(), stored_hash.as_bytes()) {
            return Err(StatusCode::UNAUTHORIZED);
        }
    }

    Ok(stored)
}

/// Best-effort cleanup of bookkeeping rows after a response has been
/// successfully retrieved. Failures are logged but never propagated; both keys
/// have a TTL fallback.
async fn cleanup_after_consumption(
    request_id: &Uuid,
    delete_nonce: bool,
    redis: &mut ConnectionManager,
) {
    if let Err(e) = redis
        .del::<_, ()>(format!("{REQ_STATUS_PREFIX}{request_id}"))
        .await
    {
        tracing::warn!("Failed to delete status for {request_id} after response retrieval: {e}");
    }

    if delete_nonce {
        if let Err(e) = redis
            .del::<_, ()>(format!("{REQ_NONCE_PREFIX}{request_id}"))
            .await
        {
            tracing::warn!(
                "Failed to delete session nonce hash for {request_id} after response retrieval: {e}"
            );
        }
    }
}

async fn get_response(
    Path(request_id): Path<Uuid>,
    headers: HeaderMap,
    Extension(mut redis): Extension<ConnectionManager>,
) -> Result<Json<Response>, StatusCode> {
    // Returns Some(stored_hash) for code-variant requests (gate fired and passed)
    // and None for legacy requests (no gate to fire).
    let stored_nonce_hash = enforce_session_nonce_gate(&request_id, &headers, &mut redis).await?;

    // Use a transaction to get both status and response atomically
    let mut pipe = redis::pipe();
    pipe.get(format!("{REQ_STATUS_PREFIX}{request_id}"))
        .get_del(format!("{RES_PREFIX}{request_id}"));

    let (status, value): (Option<String>, Option<Vec<u8>>) = pipe
        .query_async(&mut redis)
        .await
        .map_err(handle_redis_error)?;

    if let Some(value) = value {
        let current_status = status
            .and_then(|s| RequestStatus::from_str(&s).ok())
            .unwrap_or(RequestStatus::Retrieved);

        tracing::info!(
            "Request {request_id} state transition: {} -> {}",
            current_status,
            RequestStatus::Completed
        );

        cleanup_after_consumption(&request_id, stored_nonce_hash.is_some(), &mut redis).await;

        return serde_json::from_slice(&value).map_or(
            Err(StatusCode::INTERNAL_SERVER_ERROR),
            |value| {
                Ok(Json(Response {
                    response: value,
                    status: RequestStatus::Completed,
                }))
            },
        );
    }

    //ANCHOR - Return the current status for the request
    // If no response exists, use the status we already got from the transaction
    let Some(status) = status else {
        return Err(StatusCode::NOT_FOUND);
    };

    let status: RequestStatus = RequestStatus::from_str(&status).map_err(|e| {
        tracing::error!("Failed to parse status: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(Json(Response {
        status,
        response: None,
    }))
}

async fn has_response_status(
    Path(request_id): Path<Uuid>,
    Extension(mut redis): Extension<ConnectionManager>,
) -> StatusCode {
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

async fn insert_response(
    Path(request_id): Path<Uuid>,
    Extension(mut redis): Extension<ConnectionManager>,
    Json(request): Json<RequestPayload>,
) -> Result<StatusCode, StatusCode> {
    //ANCHOR - Check the request is valid
    let current_status = redis
        .get::<_, Option<String>>(format!("{REQ_STATUS_PREFIX}{request_id}"))
        .await
        .map_err(handle_redis_error)?
        .and_then(|s| RequestStatus::from_str(&s).ok());

    let Some(current_status) = current_status else {
        return Err(StatusCode::BAD_REQUEST);
    };

    //ANCHOR - Atomically store the response with TTL if not already set (idempotent)
    let options = SetOptions::default()
        .conditional_set(ExistenceCheck::NX)
        .with_expiration(SetExpiry::EX(EXPIRE_AFTER_SECONDS));

    let set_ok: Option<String> = redis
        .set_options(
            format!("{RES_PREFIX}{request_id}"),
            serde_json::to_vec(&request).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
            options,
        )
        .await
        .map_err(handle_redis_error)?;

    if set_ok.is_none() {
        return Err(StatusCode::CONFLICT);
    }

    tracing::info!(
        "Request {request_id} state transition: {} -> {}",
        current_status,
        RequestStatus::Completed
    );

    //ANCHOR - Delete status
    //NOTE - We can delete the status at this point as the presence of a response implies the request is complete
    redis
        .del::<_, ()>(format!("{REQ_STATUS_PREFIX}{request_id}"))
        .await
        .map_err(handle_redis_error)?;

    Ok(StatusCode::CREATED)
}

/// Create a new standalone response (World App initiates)
async fn create_response(
    Extension(mut redis): Extension<ConnectionManager>,
    Json(request): Json<RequestPayload>,
) -> Result<(StatusCode, Json<ResponseCreatedPayload>), StatusCode> {
    let request_id = Uuid::new_v4();

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
