#![warn(clippy::all, clippy::pedantic, clippy::nursery)]

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
        .with_max_level(tracing::Level::DEBUG)
        .with_target(false)
        .init();

    let redis = build_redis_pool(env::var("REDIS_URL").expect("REDIS_URL not set"))
        .await
        .expect("Failed to connect to Redis");

    server::start(redis).await;
}

async fn build_redis_pool(mut redis_url: String) -> redis::RedisResult<ConnectionManager> {
    if !redis_url.starts_with("redis://") {
        redis_url = format!("redis://{redis_url}");
    }

    let client = redis::Client::open(redis_url)?;

    ConnectionManager::new(client).await
}
