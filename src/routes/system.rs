use axum::{routing::get, Json, Router};

pub fn handler() -> ApiRouter {
    ApiRouter::new()
        .route("/", get(get_info))
        .route("/openapi.json", get(api_schema))
}

#[derive(Debug, serde::Serialize)]
pub struct AppVersion {
    semver: String,
    rev: Option<String>,
    compile_time: String,
}

#[derive(Debug, serde::Serialize)]
pub struct RootResponse {
    /// Repository URL
    pub repo_url: String,
    /// Application version
    pub version: AppVersion,
}

#[allow(clippy::unused_async)]
async fn get_info() -> Json<RootResponse> {
    Json(RootResponse {
        repo_url: "https://github.com/worldcoin/wallet-bridge".to_string(),
        version: AppVersion {
            semver: env!("CARGO_PKG_VERSION").to_string(),
            compile_time: env!("STATIC_BUILD_DATE").to_string(),
            rev: option_env!("GIT_REV").map(ToString::to_string),
        },
    })
}

#[allow(clippy::unused_async)]
async fn api_schema(Extension(openapi): Extension<OpenApi>) -> Json<OpenApi> {
    Json(openapi)
}
