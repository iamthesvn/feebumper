use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("transaction not found in mempool: {0}")]
    TxNotInMempool(String),

    #[error("invalid anchor output: vout {0} does not exist on transaction")]
    InvalidAnchorVout(u32),

    #[error("fee bump not needed: parent already pays sufficient fee rate")]
    BumpNotNeeded,

    #[error("insufficient service funds to cover the fee bump")]
    InsufficientFunds,

    #[error("lightning error: {0}")]
    Lightning(String),

    #[error("invalid request: {0}")]
    InvalidRequest(String),

    #[error("bump not found: {0}")]
    BumpNotFound(uuid::Uuid),

    #[error("bitcoin rpc error: {0}")]
    BitcoinRpc(String),

    #[error("internal error: {0}")]
    Internal(String),
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let (status, msg) = match &self {
            Error::TxNotInMempool(_) => (StatusCode::NOT_FOUND, self.to_string()),
            Error::InvalidAnchorVout(_) => (StatusCode::BAD_REQUEST, self.to_string()),
            Error::BumpNotNeeded => (StatusCode::BAD_REQUEST, self.to_string()),
            Error::InsufficientFunds => (StatusCode::SERVICE_UNAVAILABLE, self.to_string()),
            Error::Lightning(_) => (StatusCode::BAD_GATEWAY, self.to_string()),
            Error::InvalidRequest(_) => (StatusCode::BAD_REQUEST, self.to_string()),
            Error::BumpNotFound(_) => (StatusCode::NOT_FOUND, self.to_string()),
            Error::BitcoinRpc(_) => (StatusCode::BAD_GATEWAY, self.to_string()),
            Error::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, self.to_string()),
        };

        (status, Json(json!({ "error": msg }))).into_response()
    }
}
