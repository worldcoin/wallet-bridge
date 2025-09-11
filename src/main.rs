#![deny(clippy::all, clippy::pedantic, clippy::nursery)]

use dotenvy::dotenv;
use redis::aio::ConnectionManager;
use std::env;

mod routes;
mod server;
mod utils;

#[tokio::main]
async fn main() {
    dotenv().ok();

    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .json()
        .with_target(false)
        .flatten_event(true)
        .without_time()
        .init();

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

    tracing::info!("Attempting to connect to Redis...");

    let redis = build_redis_pool(redis_url)
        .await
        .map_err(|e| {
            tracing::error!("Redis connection failed: {}", e);
            e
        })
        .expect("Failed to connect to Redis");

    tracing::info!("✅ Connection to Redis established.");

    server::start(redis).await;
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
            redis::ErrorKind::IoError,
            "Redis connection timeout after 30 seconds",
        ))
    })?
}
