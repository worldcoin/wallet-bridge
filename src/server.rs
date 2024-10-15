use std::{env, net::SocketAddr};

use aide::openapi::{Info, License, OpenApi};
use axum::{extract::DefaultBodyLimit, Extension};
use redis::aio::ConnectionManager;
use tokio::net::TcpListener;

use crate::routes;

pub async fn start(redis: ConnectionManager) {
    let mut openapi = OpenApi {
        info: Info {
            title: "Wallet Bridge".to_string(),
            summary: Some(
                "An end-to-end encrypted bridge for communicating with World App.".to_string(),
            ),
            license: Some(License {
                name: "MIT".to_string(),
                identifier: Some("MIT".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        },
        ..Default::default()
    };

    let app = routes::handler()
        .finish_api(&mut openapi)
        .layer(Extension(redis))
        .layer(Extension(openapi))
        .layer(DefaultBodyLimit::max(5 * 1024 * 1024));

    let address = SocketAddr::from((
        [0, 0, 0, 0],
        env::var("PORT").map_or(8000, |p| p.parse().unwrap()),
    ));
    let listener = TcpListener::bind(&address)
        .await
        .expect("Failed to bind address");

    println!("🪩 World Bridge started on http://{address}");
    axum::serve(listener, app.into_make_service())
        .await
        .expect("Failed to start server");
}
