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

use crate::{
    axum::TemporaryForceDecodeJson,
    utils::{
        handle_redis_error, RequestPayload, RequestStatus, EXPIRE_AFTER_SECONDS, REQ_STATUS_PREFIX,
    },
};

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
        .map_err(handle_redis_error)?;

    if let Some(value) = value {
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
    let Some(status) = redis
        .get::<_, Option<String>>(format!("{REQ_STATUS_PREFIX}{request_id}"))
        .await
        .map_err(handle_redis_error)?
    else {
        //ANCHOR - Request ID does not exist
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

async fn insert_response(
    Path(request_id): Path<Uuid>,
    Extension(mut redis): Extension<ConnectionManager>,
    TemporaryForceDecodeJson(request): TemporaryForceDecodeJson<RequestPayload>,
) -> Result<StatusCode, StatusCode> {
    //ANCHOR - Check the request is valid
    if !redis
        .exists::<_, bool>(format!("{REQ_STATUS_PREFIX}{request_id}"))
        .await
        .map_err(handle_redis_error)?
    {
        return Err(StatusCode::BAD_REQUEST);
    }

    //ANCHOR - Check the response has not been set already
    if redis
        .exists::<_, bool>(format!("{RES_PREFIX}{request_id}"))
        .await
        .map_err(handle_redis_error)?
    {
        return Err(StatusCode::CONFLICT);
    }

    //ANCHOR - Store the response
    redis
        .set_ex::<_, _, ()>(
            format!("{RES_PREFIX}{request_id}"),
            serde_json::to_vec(&request).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
            EXPIRE_AFTER_SECONDS,
        )
        .await
        .map_err(handle_redis_error)?;

    //ANCHOR - Delete status
    //NOTE - We can delete the status at this point as the presence of a response implies the request is complete
    redis
        .del(format!("{REQ_STATUS_PREFIX}{request_id}"))
        .await
        .map_err(handle_redis_error)?;

    Ok(StatusCode::CREATED)
}
