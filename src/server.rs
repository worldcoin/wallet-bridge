use std::{env, net::SocketAddr, sync::Arc};

use aide::openapi::{Info, License, OpenApi};
use axum::{extract::DefaultBodyLimit, Extension};
use redis::aio::ConnectionManager;
use tokio::{net::TcpListener, signal};

use crate::routes;
use crate::utils::AppOverrides;

pub async fn start(redis: ConnectionManager, app_overrides: Arc<AppOverrides>) {
    let mut openapi = OpenApi {
        info: Info {
            title: "Message Bridge".to_string(),
            summary: Some(
                "A dumb, environment and client agnostic relay of arbitrary messages. It lets two parties share an arbitrary message where parties can gossip a symmetric key off-band.".to_string(),
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
        .layer(Extension(app_overrides))
        .layer(Extension(openapi))
        .layer(DefaultBodyLimit::max(5 * 1024 * 1024));

    let address = SocketAddr::from((
        [0, 0, 0, 0],
        env::var("PORT").map_or(8000, |p| p.parse().unwrap()),
    ));
    let listener = TcpListener::bind(&address)
        .await
        .expect("Failed to bind address");

    println!("🔛💬 Message Bridge started on http://{address}");

    axum::serve(listener, app.into_make_service())
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("Failed to start server");
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("Failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {
            tracing::info!("👋 Received Ctrl+C, shutting down gracefully...");
        },
        () = terminate => {
            tracing::info!("👋Received SIGTERM, shutting down gracefully...");
        },
    }
}
