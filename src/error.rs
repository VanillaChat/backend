use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;
use serde_json::json;

#[derive(Debug, Serialize)]
pub struct ApiError {
    pub code: String,
    pub path: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ValidationError {
    pub code: String,
    pub errors: Vec<FieldError>,
}

#[derive(Debug, Serialize)]
pub struct FieldError {
    pub code: String,
    pub path: String,
}

#[derive(Debug)]
pub enum AppError {
    Unauthorized,
    Forbidden(String),
    NotFound(String),
    BadRequest(String),
    Conflict(ApiError),
    ValidationFailed(Vec<FieldError>),
    InternalServerError(String),
    TooManyRequests { message: String, retry_after: i64 },
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, body) = match self {
            AppError::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                json!({"code": "UNAUTHORIZED"}),
            ),
            AppError::Forbidden(msg) => (
                StatusCode::FORBIDDEN,
                json!({"code": "FORBIDDEN", "message": msg}),
            ),
            AppError::NotFound(code) => (
                StatusCode::NOT_FOUND,
                json!({"code": code}),
            ),
            AppError::BadRequest(msg) => (
                StatusCode::BAD_REQUEST,
                json!({"code": "BAD_REQUEST", "message": msg}),
            ),
            AppError::Conflict(err) => (
                StatusCode::CONFLICT,
                serde_json::to_value(err).unwrap_or_default(),
            ),
            AppError::ValidationFailed(errors) => (
                StatusCode::BAD_REQUEST,
                json!({
                    "code": "VALIDATION_FAILED",
                    "errors": errors
                }),
            ),
            AppError::InternalServerError(msg) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({"code": "INTERNAL_SERVER_ERROR", "message": msg}),
            ),
            AppError::TooManyRequests { message, retry_after } => (
                StatusCode::TOO_MANY_REQUESTS,
                json!({"message": message, "retryAfter": retry_after}),
            ),
        };

        (status, Json(body)).into_response()
    }
}

impl From<anyhow::Error> for AppError {
    fn from(err: anyhow::Error) -> Self {
        AppError::InternalServerError(err.to_string())
    }
}

impl From<Box<dyn std::error::Error + Send + Sync>> for AppError {
    fn from(err: Box<dyn std::error::Error + Send + Sync>) -> Self {
        AppError::InternalServerError(err.to_string())
    }
}

impl From<redis::RedisError> for AppError {
    fn from(err: redis::RedisError) -> Self {
        AppError::InternalServerError(err.to_string())
    }
}
