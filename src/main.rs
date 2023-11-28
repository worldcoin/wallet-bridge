#![warn(clippy::all, clippy::pedantic, clippy::nursery)]
use dotenvy::dotenv;
use redis::aio::ConnectionManager;
use std::env;

mod axum;
mod routes;
mod server;
mod utils;

#[tokio::main]
async fn main() {
    dotenv().ok();

    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_target(false)
        .init();

    tracing::info!("Starting wallet bridge...");

    // Construct Redis URL
    let redis_url = env::var("REDIS_URL").unwrap_or_else(|_| {
        let host = env::var("REDIS_HOST").expect("REDIS_URL or REDIS_HOST is required.");
        let port = env::var("REDIS_PORT").expect("REDIS_PORT required if REDIS_URL is not set.");
        let username =
            env::var("REDIS_USERNAME").expect("REDIS_USERNAME required if REDIS_URL is not set.");
        let password =
            env::var("REDIS_PASSWORD").expect("REDIS_PASSWORD required if REDIS_URL is not set.");
        // Get the REDIS_USE_TLS environment variable and parse it as a boolean
        let use_tls = env::var("REDIS_USE_TLS")
            .map(|val| val.to_lowercase() == "true")
            .unwrap_or(false);

        format!(
            "{}://{}:{}@{}:{}",
            if use_tls { "rediss" } else { "redis" },
            username,
            password,
            host,
            port
        )
    });

    let redis = build_redis_pool(redis_url)
        .await
        .expect("Failed to connect to Redis");

    tracing::info!("✅ Connection to Redis established.");

    server::start(redis).await;
}

async fn build_redis_pool(mut redis_url: String) -> redis::RedisResult<ConnectionManager> {
    if !redis_url.starts_with("redis://") && !redis_url.starts_with("rediss://") {
        redis_url = format!("redis://{redis_url}");
    }

    let client = redis::Client::open(redis_url)?;

    ConnectionManager::new(client).await
}
