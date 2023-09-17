use axum::{
    body::Bytes,
    extract::Path,
    http::{Method, StatusCode},
    routing::head,
    Extension, Router,
};
use redis::{aio::ConnectionManager, AsyncCommands};
use tower_http::cors::{AllowHeaders, Any, CorsLayer};

use crate::EXPIRE_AFTER_SECONDS;

const REQ_PREFIX: &str = "req:";

pub fn handler() -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_headers(AllowHeaders::any())
        .allow_methods([Method::PUT, Method::HEAD]);

    Router::new().route(
        "/request/:request_id",
        head(has_request)
            .get(get_request)
            .put(insert_request)
            .layer(cors),
    )
}

async fn has_request(
    Path(request_id): Path<String>,
    Extension(mut redis): Extension<ConnectionManager>,
) -> StatusCode {
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
) -> Result<Vec<u8>, StatusCode> {
    let value = redis
        .get_del::<_, Option<Vec<u8>>>(format!("{REQ_PREFIX}{request_id}"))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    value.ok_or(StatusCode::NOT_FOUND)
}

async fn insert_request(
    Path(request_id): Path<String>,
    Extension(mut redis): Extension<ConnectionManager>,
    body: Bytes,
) -> Result<StatusCode, StatusCode> {
    if !redis
        .set_nx::<_, _, bool>(format!("{REQ_PREFIX}{request_id}"), body.to_vec())
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    {
        return Ok(StatusCode::CONFLICT);
    }

    redis
        .expire::<_, ()>(format!("{REQ_PREFIX}{request_id}"), EXPIRE_AFTER_SECONDS)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(StatusCode::CREATED)
}
