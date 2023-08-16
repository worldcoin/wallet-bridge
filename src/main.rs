#![warn(clippy::all, clippy::pedantic, clippy::nursery)]

use dotenvy::dotenv;
use redis::{aio::ConnectionManager, Client};
use std::env;

mod routes;
mod server;

const EXPIRE_AFTER_SECONDS: usize = 60;

#[tokio::main]
async fn main() {
    dotenv().ok();

    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_target(false)
        .init();

    let redis = ConnectionManager::new(
        Client::open(env::var("REDIS_URL").expect("REDIS_URL not set"))
            .expect("Failed to create redis client"),
    )
    .await
    .expect("Failed to create redis connection manager");

    server::start(redis).await;
}
