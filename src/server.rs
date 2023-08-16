use axum::Extension;
use axum_aws_lambda::LambdaLayer;
use redis::aio::ConnectionManager;
use tower::ServiceBuilder;

use crate::routes;

pub async fn start(redis: ConnectionManager) {
    let router = routes::handler().layer(Extension(redis));

    let app = ServiceBuilder::new()
        .layer(LambdaLayer::default())
        .service(router);

    lambda_http::run(app).await.unwrap();
}
