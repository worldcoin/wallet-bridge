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
use std::sync::Arc;
use tower_http::cors::{AllowHeaders, Any, CorsLayer};
use uuid::Uuid;

use crate::utils::{
    handle_redis_error, validate_request_id, AppOverrides, RequestPayload, RequestStatus,
    EXPIRE_AFTER_SECONDS, REQ_STATUS_PREFIX,
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
    ///
    /// **Entropy is the RP's responsibility.** The bridge enforces length
    /// bounds and a charset (see `validate_request_id`) but does not check
    /// cryptographic randomness. A predictable identifier lets a third party
    /// GETDEL the request before the legitimate consumer does — confidentiality
    /// still holds because the payload is encrypted, but the single-use
    /// guarantee is broken. RPs should use a high-entropy source (UUID v4,
    /// HKDF output, etc.). TODO: enforce a stronger guarantee at the protocol
    /// layer — e.g. a server-mixed nonce — once we have a clear story for it.
    #[serde(default)]
    request_id: Option<String>,
}

#[derive(Debug, serde::Serialize, JsonSchema)]
struct RequestCreatedPayload {
    /// The unique identifier for the request — the client-supplied value if
    /// one was provided, otherwise a server-generated UUID v4.
    request_id: String,
    /// Temporary workaround for the World ID app rollout.
    /// to configure deeplink values from the server and set app-specific overrides
    #[serde(skip_serializing_if = "AppOverrides::is_empty")]
    app_overrides: AppOverrides,
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
    let request_id = request_id.to_lowercase();
    if validate_request_id(&request_id).is_err() {
        return StatusCode::BAD_REQUEST;
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
    let request_id = request_id.to_lowercase();
    if validate_request_id(&request_id).is_err() {
        return Err(StatusCode::BAD_REQUEST);
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
    Extension(app_overrides): Extension<Arc<AppOverrides>>,
    Json(body): Json<CreateRequestBody>,
) -> Result<Json<RequestCreatedPayload>, StatusCode> {
    let request_id = match body.request_id {
        Some(id) => {
            let id = id.to_lowercase();
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

    Ok(Json(RequestCreatedPayload {
        request_id,
        app_overrides: (*app_overrides).clone(),
    }))
}

/// Create a new request by ID idempotently — retries succeed, even if the request exists.
/// Note: only enabled in staging.
async fn put_request(
    Path(request_id): Path<String>,
    Extension(mut redis): Extension<ConnectionManager>,
    Json(request): Json<RequestPayload>,
) -> Result<StatusCode, StatusCode> {
    let request_id = request_id.to_lowercase();
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
