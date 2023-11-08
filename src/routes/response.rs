use std::str::FromStr;

use axum::{
    extract::Path,
    http::{Method, StatusCode},
    routing::get,
    Extension, Json, Router,
};
use redis::{aio::ConnectionManager, AsyncCommands};
use std::str;
use tower_http::cors::{AllowHeaders, Any, CorsLayer};
use uuid::Uuid;

use crate::{RequestPayload, RequestStatus, EXPIRE_AFTER_SECONDS, REQ_STATUS_PREFIX};

const RES_PREFIX: &str = "res:";

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct Response {
    status: RequestStatus,
    response: Option<RequestPayload>,
}

pub fn handler() -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_headers(AllowHeaders::any())
        .allow_methods([Method::GET, Method::PUT]); //TODO: PUT is required by the simulator but should not be included

    Router::new().route(
        "/response/:request_id",
        get(get_response).put(insert_response).layer(cors),
    )
}

async fn get_response(
    Path(request_id): Path<Uuid>,
    Extension(mut redis): Extension<ConnectionManager>,
) -> Result<Json<Response>, StatusCode> {
    //ANCHOR - Return the response if available
    let value = redis
        .get_del::<_, Option<Vec<u8>>>(format!("{RES_PREFIX}{request_id}"))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    if value.is_some() {
        return serde_json::from_slice(&value.unwrap()).map_or(
            Err(StatusCode::INTERNAL_SERVER_ERROR),
            |value| {
                Ok(Json(Response {
                    status: RequestStatus::Completed,
                    response: value,
                }))
            },
        );
    }

    //ANCHOR - Return the current status for the request
    let status_opt: Option<Vec<u8>> = redis
        .get::<_, Option<Vec<u8>>>(format!("{REQ_STATUS_PREFIX}{request_id}"))
        .await
        .map_err(|e| {
            tracing::error!("Redis error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    if let Some(bytes) = status_opt {
        let status_str = str::from_utf8(&bytes).map_err(|e| {
            tracing::error!(
                "Failed to convert bytes to string when fetching status from Redis: {}",
                e
            );
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
        let status: RequestStatus = RequestStatus::from_str(status_str).map_err(|e| {
            tracing::error!("Failed to parse status: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

        return Ok(Json(Response {
            status,
            response: None,
        }));
    }

    //ANCHOR - Request ID does not exist
    return Err(StatusCode::NOT_FOUND);
}

async fn insert_response(
    Path(request_id): Path<Uuid>,
    Extension(mut redis): Extension<ConnectionManager>,
    Json(request): Json<RequestPayload>,
) -> Result<StatusCode, StatusCode> {
    //ANCHOR - Store the response
    if !redis
        .set_nx::<_, _, bool>(
            format!("{RES_PREFIX}{request_id}"),
            serde_json::to_vec(&request).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
        )
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    {
        return Ok(StatusCode::CONFLICT);
    }

    redis
        .expire::<_, ()>(&request_id.to_string(), EXPIRE_AFTER_SECONDS)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    //ANCHOR - Delete status
    //NOTE - We can delete the status now as the presence of a response implies the request is complete
    redis
        .del::<_, Option<Vec<u8>>>(format!("{REQ_STATUS_PREFIX}{request_id}"))
        .await
        .map_err(|e| {
            tracing::error!("Redis error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(StatusCode::CREATED)
}
