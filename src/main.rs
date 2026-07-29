#![deny(clippy::all, clippy::pedantic, clippy::nursery)]

use dotenvy::dotenv;
use redis::aio::ConnectionManager;
use std::env;
use std::sync::Arc;

use world_id_bridge::utils::AppOverrides;

#[tokio::main]
async fn main() {
    dotenv().ok();

    let _telemetry_guard = telemetry_batteries::init().expect("Failed to initialize telemetry");

    tracing::info!("Starting wallet bridge...");

    let redis_url = env::var("REDIS_URL").unwrap_or_else(|_| {
        let host = env::var("REDIS_HOST").expect("REDIS_HOST required if REDIS_URL is not set.");
        let port = env::var("REDIS_PORT").expect("REDIS_PORT required if REDIS_URL is not set.");
        let username =
            env::var("REDIS_USERNAME").expect("REDIS_USERNAME required if REDIS_URL is not set.");
        let password =
            env::var("REDIS_PASSWORD").expect("REDIS_PASSWORD required if REDIS_URL is not set.");
        let use_tls = env::var("REDIS_USE_TLS")
            .map(|val| val.to_lowercase() == "true")
            .unwrap_or(false);

        format!(
            "{}://{username}:{password}@{host}:{port}",
            if use_tls { "rediss" } else { "redis" }
        )
    });

    // Rustls is used for Redis TLS
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install rustls ring crypto provider");

    tracing::info!("Attempting to connect to Redis...");

    let redis = build_redis_pool(redis_url)
        .await
        .map_err(|e| {
            tracing::error!("Redis connection failed: {}", e);
            e
        })
        .expect("Failed to connect to Redis");

    tracing::info!("✅ Connection to Redis established.");

    let app_overrides = Arc::new(load_app_overrides());

    world_id_bridge::server::start(redis, app_overrides).await;
}

/// Load the per-`app_id` URL override map from the `APP_URL_OVERRIDES` env var.
///
/// The value is a JSON object of `app_id → {app_clip_bundle_id?, verify_url?}`.
/// Unset or empty ⇒ the feature is off (empty map; the SDK keeps its built-in
/// defaults). This is the kill switch: clear the env var and restart.
///
/// Malformed JSON is a fatal startup error (panic) rather than a silently
/// empty map — a broken config must fail loudly at deploy time, not degrade
/// into "no overrides" that looks healthy. Parsed exactly once here; the
/// resulting map is shared read-only across all requests.
fn load_app_overrides() -> AppOverrides {
    let raw = match env::var("APP_URL_OVERRIDES") {
        Ok(s) if !s.trim().is_empty() => s,
        _ => {
            tracing::info!("APP_URL_OVERRIDES not set — app URL overrides disabled.");
            return AppOverrides::default();
        }
    };

    let overrides: AppOverrides = serde_json::from_str(&raw)
        .expect("APP_URL_OVERRIDES is set but is not valid JSON for app_id → override map");

    tracing::info!("Loaded {} app URL override(s).", overrides.len());
    overrides
}

async fn build_redis_pool(redis_url: String) -> redis::RedisResult<ConnectionManager> {
    let client = redis::Client::open(redis_url)?;

    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        ConnectionManager::new(client),
    )
    .await
    .map_err(|_| {
        redis::RedisError::from((
            redis::ErrorKind::Io,
            "Redis connection timeout after 30 seconds",
        ))
    })?
}
