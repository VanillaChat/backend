use axum::{
    extract::Path,
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Router,
};
use tower_http::services::ServeDir;

use crate::state::SharedState;

pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/{*path}", get(serve_cdn))
}

async fn serve_cdn(
    Path(path): Path<String>,
) -> impl IntoResponse {
    let cdn_path = std::path::PathBuf::from("./cdn").join(&path);
    
    if cdn_path.exists() && cdn_path.is_file() {
        match tokio::fs::read(&cdn_path).await {
            Ok(contents) => {
                let content_type = if path.ends_with(".webp") {
                    "image/webp"
                } else if path.ends_with(".png") {
                    "image/png"
                } else if path.ends_with(".jpg") || path.ends_with(".jpeg") {
                    "image/jpeg"
                } else if path.ends_with(".gif") {
                    "image/gif"
                } else {
                    "application/octet-stream"
                };
                
                (
                    StatusCode::OK,
                    [(axum::http::header::CONTENT_TYPE, content_type)],
                    contents
                ).into_response()
            }
            Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        }
    } else {
        StatusCode::NOT_FOUND.into_response()
    }
}
