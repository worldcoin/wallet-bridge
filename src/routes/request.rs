use aide::axum::{
    routing::{head, post},
    ApiRouter,
};
use axum::{
    extract::Path,
    http::{Method, StatusCode},
    Extension,
};
use axum_jsonschema::Json;
use redis::{aio::ConnectionManager, AsyncCommands};
use schemars::JsonSchema;
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
        .allow_methods([Method::POST, Method::HEAD]);

    // You must chain the routes to the same Router instance
    ApiRouter::new()
        .api_route("/request", post(insert_request))
        .api_route("/request/:request_id", head(has_request).get(get_request))
        .layer(cors) // Apply the CORS layer to all routes
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
