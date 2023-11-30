use axum::{
    extract::Path,
    http::{Method, StatusCode},
    routing::{head, post},
    Extension, Json, Router,
};
use redis::{aio::ConnectionManager, AsyncCommands};
use tower_http::cors::{AllowHeaders, Any, CorsLayer};
use uuid::Uuid;

use crate::utils::{
    handle_redis_error, RequestPayload, RequestStatus, EXPIRE_AFTER_SECONDS, REQ_STATUS_PREFIX,
};

const REQ_PREFIX: &str = "req:";

#[derive(Debug, serde::Serialize)]
struct CustomResponse {
    request_id: Uuid,
}

pub fn handler() -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_headers(AllowHeaders::any())
        .allow_methods([Method::POST, Method::HEAD]);

    // You must chain the routes to the same Router instance
    Router::new()
        .route("/request", post(insert_request))
        .route("/request/:request_id", head(has_request).get(get_request))
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
    let value = redis
        .get_del::<_, Option<Vec<u8>>>(format!("{REQ_PREFIX}{request_id}"))
        .await
        .map_err(handle_redis_error)?;

    if value.is_none() {
        return Err(StatusCode::NOT_FOUND);
    }

    //ANCHOR - Update the status of the request
    redis
        .set_ex::<_, _, ()>(
            format!("{REQ_STATUS_PREFIX}{request_id}"),
            RequestStatus::Retrieved.to_string(),
            EXPIRE_AFTER_SECONDS,
        )
        .await
        .map_err(handle_redis_error)?;

    serde_json::from_slice(&value.unwrap()).map_or(
        Err(StatusCode::INTERNAL_SERVER_ERROR),
        |value| {
            tracing::info!(
                "{}",
                format!("Successfully retrieved /request: {request_id}")
            );

            Ok(Json(value))
        },
    )
}

async fn insert_request(
    Extension(mut redis): Extension<ConnectionManager>,
    Json(request): Json<RequestPayload>,
) -> Result<Json<CustomResponse>, StatusCode> {
    let request_id = Uuid::new_v4();

    tracing::info!("{}", format!("Processing /request: {request_id}"));

    //ANCHOR - Set request status
    redis
        .set_ex::<_, _, ()>(
            format!("{REQ_STATUS_PREFIX}{request_id}"),
            RequestStatus::Initialized.to_string(),
            EXPIRE_AFTER_SECONDS,
        )
        .await
        .map_err(handle_redis_error)?;

    //ANCHOR - Store payload
    redis
        .set_ex::<_, _, ()>(
            format!("{REQ_PREFIX}{request_id}"),
            serde_json::to_vec(&request).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
            EXPIRE_AFTER_SECONDS,
        )
        .await
        .map_err(handle_redis_error)?;

    tracing::info!("{}", format!("Successfully stored /request: {request_id}"));

    Ok(Json(CustomResponse { request_id }))
}
