use std::time::SystemTime;

use async_trait::async_trait;
use axum::{
    body::{Bytes, HttpBody},
    extract::FromRequest,
    http::Request,
    response::IntoResponse,
    BoxError,
};
use serde::de::DeserializeOwned;

#[derive(Debug, thiserror::Error)]
pub enum JsonError {
    #[error("failed to deserialize json")]
    Deserialize(#[from] serde_path_to_error::Error<serde_json::Error>),

    #[error("failed to extract body")]
    Body(#[from] axum::extract::rejection::BytesRejection),
}

impl IntoResponse for JsonError {
    fn into_response(self) -> axum::response::Response {
        unimplemented!()
    }
}

#[derive(Debug, Clone, Copy, Default)]
#[must_use]
pub struct TemporaryForceDecodeJson<T>(pub T);

#[async_trait]
impl<T, S, B> FromRequest<S, B> for TemporaryForceDecodeJson<T>
where
    T: DeserializeOwned,
    B: HttpBody + Send + 'static,
    B::Data: Send,
    B::Error: Into<BoxError>,
    S: Send + Sync,
{
    type Rejection = JsonError;

    async fn from_request(req: Request<B>, state: &S) -> Result<Self, Self::Rejection> {
        assert!(
            !is_after_nov_30(),
            "This was supposed to be a very temporary hack you fools!!!! Time to use proper JSON headers."
        );

        let bytes = Bytes::from_request(req, state).await?;
        let deserializer = &mut serde_json::Deserializer::from_slice(&bytes);

        Ok(Self(serde_path_to_error::deserialize(deserializer)?))
    }
}

fn is_after_nov_30() -> bool {
    SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        > 1_701_331_200
}
