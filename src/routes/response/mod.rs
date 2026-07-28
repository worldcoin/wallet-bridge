use aide::axum::{
    routing::{get, post},
    ApiRouter,
};
use axum::http::Method;
use tower_http::cors::{AllowHeaders, Any, CorsLayer};

mod get;
mod head;
mod post;
mod put;

pub(super) const RES_PREFIX: &str = "res:";

pub fn handler() -> ApiRouter {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_headers(AllowHeaders::any())
        .allow_methods([Method::GET, Method::PUT, Method::POST]); //TODO: PUT is required by the simulator but should not be included

    ApiRouter::new()
        .api_route(
            "/response/:request_id",
            get(get::handler)
                .head(head::handler)
                .put(put::handler)
                .layer(cors.clone()),
        )
        .api_route("/response", post(post::handler).layer(cors))
}
