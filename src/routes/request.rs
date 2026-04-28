use std::env;
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

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
use redis::{aio::ConnectionManager, AsyncCommands};
use schemars::JsonSchema;
use tower_http::cors::{AllowHeaders, Any, CorsLayer};
use uuid::Uuid;

use crate::utils::{
    code_ttl_seconds, handle_redis_error, random_token, sha256_hex, validate_base64url,
    RequestPayload, RequestStatus, CODE_IDX_PREFIX, EXPIRE_AFTER_SECONDS, REQ_STATUS_PREFIX,
};

const REQ_PREFIX: &str = "req:";

const INDEX_MIN_BYTES: usize = 8;
const INDEX_MAX_BYTES: usize = 128;

/// Atomic insert for the secure invite-code variant. Returns 1 on success and 0
/// if the index is already occupied (live row), giving us the 409-on-collision
/// guarantee in a single round-trip.
const INSERT_CODE_LUA: &str = r#"
if redis.call("EXISTS", KEYS[1]) == 1 then
    return 0
end
redis.call("HSET", KEYS[1],
    "request_id", ARGV[1],
    "iv", ARGV[2],
    "payload", ARGV[3],
    "session_nonce_hash", ARGV[4],
    "redeemed", "false")
redis.call("EXPIRE", KEYS[1], ARGV[5])
return 1
"#;

#[derive(Debug, serde::Deserialize, serde::Serialize, JsonSchema)]
struct CreateRequestBody {
    /// The initialization vector for the encrypted payload (base64url for the
    /// invite-code variant, opaque otherwise).
    iv: String,
    /// The encrypted payload.
    payload: String,
    /// When `true`, the body is the secure invite-code variant and `index` must
    /// be present. When absent or `false`, the legacy shape is used.
    #[serde(default)]
    request_code_enabled: bool,
    /// HKDF-derived index (base64url) — required when `request_code_enabled` is `true`.
    #[serde(default)]
    index: Option<String>,
}

#[derive(Debug, serde::Serialize, JsonSchema)]
#[serde(untagged)]
enum CreateRequestResponse {
    Legacy(LegacyCreated),
    Code(CodeCreated),
}

#[derive(Debug, serde::Serialize, JsonSchema)]
struct LegacyCreated {
    /// The unique identifier for the request
    request_id: Uuid,
}

#[derive(Debug, serde::Serialize, JsonSchema)]
struct CodeCreated {
    /// The unique identifier for the request
    request_id: Uuid,
    /// Opaque token returned to the RP exactly once; required (alongside
    /// `delivery_token`) to retrieve the eventual response.
    session_nonce: String,
    /// Unix timestamp (seconds) at which the unredeemed code expires.
    code_expires_at: u64,
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

/// Create a new request. Branches on `request_code_enabled` to select the
/// secure invite-code variant; the legacy shape is byte-identical to before.
async fn insert_request(
    Extension(mut redis): Extension<ConnectionManager>,
    Json(body): Json<CreateRequestBody>,
) -> Result<Json<CreateRequestResponse>, StatusCode> {
    let request_id = Uuid::new_v4();

    if body.request_code_enabled {
        return insert_code_request(&mut redis, request_id, body)
            .await
            .map(|created| Json(CreateRequestResponse::Code(created)));
    }

    tracing::info!("Processing /request: {request_id}");

    let payload = RequestPayload::new(body.iv, body.payload);
    persist_request(&mut redis, request_id, &payload).await?;

    tracing::info!(
        "{}",
        format!("Successfully processed /request: {request_id}")
    );

    Ok(Json(CreateRequestResponse::Legacy(LegacyCreated {
        request_id,
    })))
}

/// Secure invite-code variant. Bridge stores the encrypted blob keyed by the
/// RP-supplied `index` (HKDF output). The user-typed code `C` and derived key
/// `K` never reach Bridge — that's the whole point.
async fn insert_code_request(
    redis: &mut ConnectionManager,
    request_id: Uuid,
    body: CreateRequestBody,
) -> Result<CodeCreated, StatusCode> {
    let index = body.index.ok_or(StatusCode::BAD_REQUEST)?;

    validate_base64url(&index, INDEX_MIN_BYTES, INDEX_MAX_BYTES)?;
    // iv/payload are validated as base64url-decodable; their byte-lengths are
    // intentionally not pinned here so we don't bake AES-GCM parameters into the
    // bridge (it's a dumb pipe).
    validate_base64url(&body.iv, 1, 1024)?;
    validate_base64url(&body.payload, 1, 5 * 1024 * 1024)?;

    tracing::info!("Processing /request (code variant): {request_id}");

    let session_nonce = random_token();
    let session_nonce_hash = sha256_hex(&session_nonce);
    let ttl = code_ttl_seconds();

    let inserted: i32 = redis::Script::new(INSERT_CODE_LUA)
        .key(format!("{CODE_IDX_PREFIX}{index}"))
        .arg(request_id.to_string())
        .arg(&body.iv)
        .arg(&body.payload)
        .arg(&session_nonce_hash)
        .arg(ttl)
        .invoke_async(redis)
        .await
        .map_err(handle_redis_error)?;

    if inserted == 0 {
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

    let code_expires_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() + ttl)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    tracing::info!("Successfully processed /request (code variant): {request_id}");

    Ok(CodeCreated {
        request_id,
        session_nonce,
        code_expires_at,
    })
}

/// Create a new request by ID idempotently — retries succeed, even if the request exits
/// Note: only enabled in staging
async fn put_request(
    Path(request_id): Path<Uuid>,
    Extension(mut redis): Extension<ConnectionManager>,
    Json(request): Json<RequestPayload>,
) -> Result<StatusCode, StatusCode> {
    tracing::info!("Processing PUT /request: {request_id}");

    // Same logic as post, but always overwrites the existing payload, set status, and reset the TTL
    persist_request(&mut redis, request_id, &request).await?;

    tracing::info!("Successfully PUT /request: {request_id}");

    Ok(StatusCode::CREATED)
}

/// Persist request payload and initialize status with TTL
async fn persist_request(
    redis: &mut ConnectionManager,
    request_id: Uuid,
    request: &RequestPayload,
) -> Result<(), StatusCode> {
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

    Ok(())
}
