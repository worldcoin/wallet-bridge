use axum::{
    extract::Path,
    http::{Method, StatusCode},
    routing::{head, post},
    Extension, Json, Router,
};
use redis::{aio::ConnectionManager, AsyncCommands};
use tower_http::cors::{AllowHeaders, Any, CorsLayer};
use uuid::Uuid;

use crate::EXPIRE_AFTER_SECONDS;

const REQ_PREFIX: &str = "req:";

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct Request {
    iv: String,
    payload: String,
}

pub fn handler() -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_headers(AllowHeaders::any())
        .allow_methods([Method::PUT, Method::HEAD]);

    Router::new()
        .route("/request", post(insert_request).layer(cors.clone()))
        .route(
            "/request/:request_id",
            head(has_request).get(get_request).layer(cors),
        )
}

async fn has_request(
    Path(request_id): Path<Uuid>,
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
    Path(request_id): Path<Uuid>,
    Extension(mut redis): Extension<ConnectionManager>,
) -> Result<Json<Request>, StatusCode> {
    let value = redis
        .get_del::<_, Option<Vec<u8>>>(format!("{REQ_PREFIX}{request_id}"))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    value.map_or_else(
        || Err(StatusCode::NOT_FOUND),
        |value| {
            serde_json::from_slice(&value).map_or(Err(StatusCode::INTERNAL_SERVER_ERROR), |value| {
                Ok(Json(value))
            })
        },
    )
}

async fn insert_request(
    // Path(request_id): Path<Uuid>,
    Extension(mut redis): Extension<ConnectionManager>,
    Json(request): Json<Request>,
) -> Result<StatusCode, StatusCode> {
    let request_id = Uuid::new_v4();

    if !redis
        .set_nx::<_, _, bool>(
            format!("{REQ_PREFIX}{request_id}"),
            serde_json::to_vec(&request).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
        )
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
