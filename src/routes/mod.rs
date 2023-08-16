use axum::Router;

mod request;
mod response;
mod system;

pub fn handler() -> Router {
    Router::new()
        .merge(system::handler())
        .merge(request::handler())
        .merge(response::handler())
}
