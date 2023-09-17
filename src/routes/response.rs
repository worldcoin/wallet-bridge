use axum::{
    body::Bytes,
    extract::Path,
    http::{Method, StatusCode},
    routing::get,
    Extension, Router,
};
use redis::{aio::ConnectionManager, AsyncCommands};
use tower_http::cors::{AllowHeaders, Any, CorsLayer};

use crate::EXPIRE_AFTER_SECONDS;

const RES_PREFIX: &str = "res:";

pub fn handler() -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET])
        .allow_headers(AllowHeaders::any());

    Router::new().route(
        "/response/:request_id",
        get(get_response).put(insert_response).layer(cors),
    )
}

async fn get_response(
    Path(request_id): Path<String>,
    Extension(mut redis): Extension<ConnectionManager>,
) -> Result<Vec<u8>, StatusCode> {
    let value = redis
        .get_del::<_, Option<Vec<u8>>>(format!("{RES_PREFIX}{request_id}"))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    value.ok_or(StatusCode::NOT_FOUND)
}

async fn insert_response(
    Path(request_id): Path<String>,
    Extension(mut redis): Extension<ConnectionManager>,
    body: Bytes,
) -> Result<StatusCode, StatusCode> {
    if !redis
        .set_nx::<_, _, bool>(format!("{RES_PREFIX}{request_id}"), body.to_vec())
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    {
        return Ok(StatusCode::CONFLICT);
    }

    redis
        .expire::<_, ()>(&request_id, EXPIRE_AFTER_SECONDS)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(StatusCode::CREATED)
}
