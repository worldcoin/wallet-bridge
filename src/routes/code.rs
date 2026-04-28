use std::str::FromStr;

use aide::axum::{routing::post, ApiRouter};
use axum::{
    http::{Method, StatusCode},
    Extension,
};
use axum_jsonschema::Json;
use redis::{aio::ConnectionManager, AsyncCommands};
use schemars::JsonSchema;
use tower_http::cors::{AllowHeaders, Any, CorsLayer};
use uuid::Uuid;

use crate::utils::{
    handle_redis_error, random_token, sha256_hex, validate_base64url, RequestStatus,
    CODE_IDX_PREFIX, EXPIRE_AFTER_SECONDS, REQ_STATUS_PREFIX,
};

const INDEX_MIN_BYTES: usize = 8;
const INDEX_MAX_BYTES: usize = 128;

/// Atomic one-shot redemption. Returns nil if the row is missing, expired, or
/// already redeemed; otherwise flips `redeemed` to true, records the delivery
/// token hash, and returns the stored ciphertext + `request_id`. The atomicity
/// here is what guarantees the "exactly one winner" property under concurrent
/// redeems — splitting this into separate read + write would reintroduce the
/// race the integration test specifically checks for.
const REDEEM_LUA: &str = r#"
local f = redis.call("HMGET", KEYS[1], "redeemed", "request_id", "iv", "payload")
if not f[1] or f[1] == "true" then
    return nil
end
redis.call("HSET", KEYS[1], "redeemed", "true", "delivery_token_hash", ARGV[1])
return {f[2], f[3], f[4]}
"#;

#[derive(Debug, serde::Deserialize, JsonSchema)]
struct RedeemRequest {
    /// HKDF-derived index (base64url) the World App computed from the
    /// user-typed code.
    index: String,
}

#[derive(Debug, serde::Serialize, JsonSchema)]
struct RedeemResponse {
    request_id: Uuid,
    /// AES-GCM IV the RP supplied at request creation.
    iv: String,
    /// AES-GCM ciphertext the RP supplied at request creation.
    payload: String,
    /// Opaque token returned to the World App exactly once. The follow-up
    /// `/response` change will require it (alongside `session_nonce`) to
    /// retrieve the eventual response.
    delivery_token: String,
}

pub fn handler() -> ApiRouter {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_headers(AllowHeaders::any())
        .allow_methods([Method::POST]);

    ApiRouter::new()
        .api_route("/code/redeem", post(redeem))
        .layer(cors)
}

async fn redeem(
    Extension(mut redis): Extension<ConnectionManager>,
    Json(body): Json<RedeemRequest>,
) -> Result<Json<RedeemResponse>, StatusCode> {
    // Malformed indexes return 404 (same shape as missing/expired/redeemed) so
    // we never leak which arm of the lookup actually rejected the request.
    if validate_base64url(&body.index, INDEX_MIN_BYTES, INDEX_MAX_BYTES).is_err() {
        return Err(StatusCode::NOT_FOUND);
    }

    let delivery_token = random_token();
    let delivery_token_hash = sha256_hex(&delivery_token);

    let result: Option<(String, String, String)> = redis::Script::new(REDEEM_LUA)
        .key(format!("{CODE_IDX_PREFIX}{}", body.index))
        .arg(&delivery_token_hash)
        .invoke_async(&mut redis)
        .await
        .map_err(handle_redis_error)?;

    let Some((request_id_str, iv, payload)) = result else {
        return Err(StatusCode::NOT_FOUND);
    };

    let request_id = Uuid::from_str(&request_id_str).map_err(|e| {
        tracing::error!("Stored request_id is not a valid UUID: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Code consumption is the moral equivalent of the legacy `GET /request/:id`
    // pull, so flip initialized -> retrieved using the canonical log shape ops
    // dashboards depend on.
    let prior = redis
        .get::<_, Option<String>>(format!("{REQ_STATUS_PREFIX}{request_id}"))
        .await
        .map_err(handle_redis_error)?
        .and_then(|s| RequestStatus::from_str(&s).ok())
        .unwrap_or(RequestStatus::Initialized);

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
        prior,
        RequestStatus::Retrieved
    );

    Ok(Json(RedeemResponse {
        request_id,
        iv,
        payload,
        delivery_token,
    }))
}
