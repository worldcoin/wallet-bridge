use std::str::FromStr;

use aide::axum::{routing::get, ApiRouter};
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

use crate::utils::{
    handle_redis_error, RequestPayload, RequestStatus, EXPIRE_AFTER_SECONDS, REQ_STATUS_PREFIX,
};

const RES_PREFIX: &str = "res:";

#[derive(Debug, serde::Deserialize, serde::Serialize, JsonSchema)]
struct Response {
    status: RequestStatus,
    response: Option<RequestPayload>,
}

pub fn handler() -> ApiRouter {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_headers(AllowHeaders::any())
        .allow_methods([Method::GET, Method::PUT]); //TODO: PUT is required by the simulator but should not be included

    ApiRouter::new().api_route(
        "/response/:request_id",
        get(get_response)
            .head(has_response_status)
            .put(insert_response)
            .layer(cors),
    )
}

async fn get_response(
    Path(request_id): Path<Uuid>,
    Extension(mut redis): Extension<ConnectionManager>,
) -> Result<Json<Response>, StatusCode> {
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
