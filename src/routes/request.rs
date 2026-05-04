use aide::axum::{
    routing::{head, post, put},
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
use std::env;
use std::str::FromStr;
use tower_http::cors::{AllowHeaders, Any, CorsLayer};
use uuid::Uuid;

use crate::utils::{
    handle_redis_error, validate_request_id, RequestPayload, RequestStatus, EXPIRE_AFTER_SECONDS,
    REQ_STATUS_PREFIX,
};

const REQ_PREFIX: &str = "req:";

#[derive(Debug, serde::Deserialize, JsonSchema)]
struct CreateRequestBody {
    /// The initialization vector for the encrypted payload (opaque to the bridge).
    iv: String,
    /// The encrypted payload (opaque to the bridge).
    payload: String,
    /// Optional client-supplied `request_id`. When present, the bridge stores
    /// the request under this key with NX semantics (409 on collision); when
    /// absent, the bridge generates a UUID v4. Lets the RP address requests by
    /// any opaque identifier they choose (e.g. an HKDF output) so the bridge
    /// stays a generic content-addressable single-use store rather than baking
    /// in any specific application's flow.
    #[serde(default)]
    request_id: Option<String>,
}

#[derive(Debug, serde::Serialize, JsonSchema)]
struct RequestCreatedPayload {
    /// The unique identifier for the request — the client-supplied value if
    /// one was provided, otherwise a server-generated UUID v4.
    request_id: String,
}

pub fn handler() -> ApiRouter {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_headers(AllowHeaders::any())
        .allow_methods([Method::POST, Method::HEAD, Method::PUT]);

    let environment = env::var("ENVIRONMENT")
        .unwrap_or_else(|_| "unknown".to_string())
        .trim()
        .to_lowercase();

    // Base routes
    let mut router = ApiRouter::new()
        .api_route("/request", post(insert_request))
        .api_route("/request/:request_id", head(has_request).get(get_request))
        .layer(cors);

    // Only enable PUT in staging
    if environment == "staging" {
        router = router.api_route("/request/:request_id", put(put_request));
    }

    router
}

async fn has_request(
    Path(request_id): Path<String>,
    Extension(mut redis): Extension<ConnectionManager>,
) -> StatusCode {
    if validate_request_id(&request_id).is_err() {
        return StatusCode::NOT_FOUND;
    }

    let Ok(exists) = redis
        .exists::<_, bool>(format!("{REQ_PREFIX}{request_id}"))
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

async fn get_request(
    Path(request_id): Path<String>,
    Extension(mut redis): Extension<ConnectionManager>,
) -> Result<Json<RequestPayload>, StatusCode> {
    // Malformed IDs return 404 (same shape as missing), so the bridge doesn't
    // leak information about its key-format expectations to callers.
    if validate_request_id(&request_id).is_err() {
        return Err(StatusCode::NOT_FOUND);
    }

    // Use a transaction to get both status and request data atomically
    let mut pipe = redis::pipe();
    pipe.get(format!("{REQ_STATUS_PREFIX}{request_id}"))
        .get_del(format!("{REQ_PREFIX}{request_id}"));

    let (status, value): (Option<String>, Option<Vec<u8>>) = pipe
        .query_async(&mut redis)
        .await
        .map_err(handle_redis_error)?;

    let current_status = status
        .and_then(|s| RequestStatus::from_str(&s).ok())
        .unwrap_or(RequestStatus::Initialized);

    let value = value.ok_or(StatusCode::NOT_FOUND)?;

    //ANCHOR - Update the status of the request
    redis
        .set_ex::<_, _, ()>(
            format!("{REQ_STATUS_PREFIX}{request_id}"),
            RequestStatus::Retrieved.to_string(),
            EXPIRE_AFTER_SECONDS,
        )
        .await
        .map_err(handle_redis_error)?;

    tracing::info!(
        "Request {request_id} state transition: {} -> {}",
        current_status,
        RequestStatus::Retrieved
    );

    serde_json::from_slice(&value).map_or(Err(StatusCode::INTERNAL_SERVER_ERROR), |value| {
        Ok(Json(value))
    })
}

/// Create a new request. Optionally accepts a client-supplied `request_id`
/// with NX semantics; otherwise generates a UUID v4.
async fn insert_request(
    Extension(mut redis): Extension<ConnectionManager>,
    Json(body): Json<CreateRequestBody>,
) -> Result<Json<RequestCreatedPayload>, StatusCode> {
    let request_id = match body.request_id {
        Some(id) => {
            validate_request_id(&id)?;
            id
        }
        None => Uuid::new_v4().to_string(),
    };

    tracing::info!("Processing /request: {request_id}");

    let payload = RequestPayload::new(body.iv, body.payload);
    let payload_bytes =
        serde_json::to_vec(&payload).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // SET NX on the payload — collisions return 409 in a single round trip.
    // The status marker is set afterwards (and is idempotent under retries).
    let options = SetOptions::default()
        .conditional_set(ExistenceCheck::NX)
        .with_expiration(SetExpiry::EX(EXPIRE_AFTER_SECONDS));

    let set_ok: Option<String> = redis
        .set_options(format!("{REQ_PREFIX}{request_id}"), payload_bytes, options)
        .await
        .map_err(handle_redis_error)?;

    if set_ok.is_none() {
        return Err(StatusCode::CONFLICT);
    }

    redis
        .set_ex::<_, _, ()>(
            format!("{REQ_STATUS_PREFIX}{request_id}"),
            RequestStatus::Initialized.to_string(),
            EXPIRE_AFTER_SECONDS,
        )
        .await
        .map_err(handle_redis_error)?;

    tracing::info!(
        "Request {request_id} state transition: new -> {}",
        RequestStatus::Initialized
    );

    tracing::info!("Successfully processed /request: {request_id}");

    Ok(Json(RequestCreatedPayload { request_id }))
}

/// Create a new request by ID idempotently — retries succeed, even if the request exists.
/// Note: only enabled in staging.
async fn put_request(
    Path(request_id): Path<String>,
    Extension(mut redis): Extension<ConnectionManager>,
    Json(request): Json<RequestPayload>,
) -> Result<StatusCode, StatusCode> {
    validate_request_id(&request_id)?;

    tracing::info!("Processing PUT /request: {request_id}");

    // Same logic as POST, but always overwrites the existing payload, sets status, and resets the TTL.
    redis
        .set_ex::<_, _, ()>(
            format!("{REQ_STATUS_PREFIX}{request_id}"),
            RequestStatus::Initialized.to_string(),
            EXPIRE_AFTER_SECONDS,
        )
        .await
        .map_err(handle_redis_error)?;

    tracing::info!(
        "Request {request_id} state transition: new -> {}",
        RequestStatus::Initialized
    );

    redis
        .set_ex::<_, _, ()>(
            format!("{REQ_PREFIX}{request_id}"),
            serde_json::to_vec(&request).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
            EXPIRE_AFTER_SECONDS,
        )
        .await
        .map_err(handle_redis_error)?;

    tracing::info!("Successfully PUT /request: {request_id}");

    Ok(StatusCode::CREATED)
}
