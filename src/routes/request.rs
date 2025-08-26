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
    handle_redis_error, RequestPayload, RequestStatus, EXPIRE_AFTER_SECONDS, REQ_STATUS_PREFIX,
};

const REQ_PREFIX: &str = "req:";

#[derive(Debug, serde::Serialize, JsonSchema)]
struct RequestCreatedPayload {
    /// The unique identifier for the request
    request_id: Uuid,
}

pub fn handler() -> ApiRouter {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_headers(AllowHeaders::any())
        .allow_methods([Method::POST, Method::HEAD, Method::PUT]);

    let environment = env::var("ENVIRONMENT").unwrap_or_else(|_| "unknown".to_string());

    // Base routes
    let mut router = ApiRouter::new()
        .api_route("/request", post(insert_request))
        .api_route("/request/:request_id", head(has_request).get(get_request))
        .layer(cors);

    // Only enable PUT in staging
    if environment.trim().to_lowercase() == "staging" {
        router = router.api_route("/request/:request_id", put(put_request));
    }

    router
}

async fn has_request(
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

async fn get_request(
    Path(request_id): Path<Uuid>,
    Extension(mut redis): Extension<ConnectionManager>,
) -> Result<Json<RequestPayload>, StatusCode> {
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

/// Create a new request
async fn insert_request(
    Extension(mut redis): Extension<ConnectionManager>,
    Json(request): Json<RequestPayload>,
) -> Result<Json<RequestCreatedPayload>, StatusCode> {
    let request_id = Uuid::new_v4();

    tracing::info!("Processing /request: {request_id}");

    //ANCHOR - Set request status
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

    //ANCHOR - Store payload
    redis
        .set_ex::<_, _, ()>(
            format!("{REQ_PREFIX}{request_id}"),
            serde_json::to_vec(&request).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
            EXPIRE_AFTER_SECONDS,
        )
        .await
        .map_err(handle_redis_error)?;

    tracing::info!(
        "{}",
        format!("Successfully processed /request: {request_id}")
    );

    Ok(Json(RequestCreatedPayload { request_id }))
}

async fn put_request(
    Path(request_id): Path<Uuid>,
    Extension(mut redis): Extension<ConnectionManager>,
    Json(request): Json<RequestPayload>,
) -> Result<StatusCode, StatusCode> {
    tracing::info!("Processing PUT /request: {request_id}");

    //ANCHOR - Store payload only if it does not already exist (idempotent)
    let options = SetOptions::default()
        .conditional_set(ExistenceCheck::NX)
        .with_expiration(SetExpiry::EX(EXPIRE_AFTER_SECONDS));

    let set_ok: Option<String> = redis
        .set_options(
            format!("{REQ_PREFIX}{request_id}"),
            serde_json::to_vec(&request).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
            options,
        )
        .await
        .map_err(handle_redis_error)?;

    if set_ok.is_none() {
        // Key already exists: refresh TTLs and treat as success
        redis
            .expire::<_, ()>(
                format!("{REQ_PREFIX}{request_id}"),
                EXPIRE_AFTER_SECONDS as i64,
            )
            .await
            .map_err(handle_redis_error)?;
        // Refresh status TTL if present; do not overwrite value/state
        let _ = redis
            .expire::<_, ()>(
                format!("{REQ_STATUS_PREFIX}{request_id}"),
                EXPIRE_AFTER_SECONDS as i64,
            )
            .await;
        return Ok(StatusCode::OK);
    }

    //ANCHOR - Set request status (only after successful creation)
    redis
        .set_ex::<_, _, ()>(
            format!("{REQ_STATUS_PREFIX}{request_id}"),
            RequestStatus::Initialized.to_string(),
            EXPIRE_AFTER_SECONDS,
        )
        .await
        .map_err(handle_redis_error)?;

    tracing::info!(
        "Request {request_id} state transition: new -> {0}",
        RequestStatus::Initialized
    );

    tracing::info!("Successfully processed /request: {request_id}");

    Ok(StatusCode::CREATED)
}
