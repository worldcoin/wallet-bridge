use std::{env, net::SocketAddr};

use axum::{Extension, Server};
use redis::aio::ConnectionManager;

use crate::routes;

pub async fn start(redis: ConnectionManager) {
    let app = routes::handler().layer(Extension(redis));

    let address = SocketAddr::from((
        [0, 0, 0, 0],
        env::var("PORT").map_or(8000, |p| p.parse().unwrap()),
    ));

    println!("🪩 World Bridge started on http://{address}");
    Server::bind(&address)
        .serve(app.into_make_service())
        .await
        .expect("Failed to start server");
}
