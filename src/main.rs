use axum::{
    Router,
    http::{HeaderValue, Method, StatusCode},
    routing::get,
};
use config::Config;
use state::AppState;
use std::net::SocketAddr;
use std::sync::Arc;
use tower::ServiceBuilder;
use tower_http::{
    compression::CompressionLayer,
    cors::{AllowOrigin, CorsLayer},
    trace::TraceLayer,
};
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod auth;
mod config;
mod error;
mod gateway;
mod middleware;
mod models;
mod routes;
mod state;
mod voice;

fn print_banner(port: u16) {
    println!(
        r#"
+------------------------------------------------------------+
|                                                            |
|   ██╗   ██╗ █████╗ ███╗   ██╗██╗██╗     ██╗      █████╗   |
|   ██║   ██║██╔══██╗████╗  ██║██║██║     ██║     ██╔══██╗  |
|   ██║   ██║███████║██╔██╗ ██║██║██║     ██║     ███████║  |
|   ╚██╗ ██╔╝██╔══██║██║╚██╗██║██║██║     ██║     ██╔══██║  |
|    ╚████╔╝ ██║  ██║██║ ╚████║██║███████╗███████╗██║  ██║  |
|     ╚═══╝  ╚═╝  ╚═╝╚═╝  ╚═══╝╚═╝╚══════╝╚══════╝╚═╝  ╚═╝  |
|                                                            |
|   Backend online                                           |
|   HTTP: http://localhost:{port:<5}                              |
|   TUI:  cargo run --bin vanilla-tui                        |
|                                                            |
+------------------------------------------------------------+
"#
    );
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "vanilla_backend=debug,tower_http=debug,axum=trace".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let config = Config::from_env()?;
    let port = config.port;
    let cors_enabled = config.cors_enabled;
    let cors_allowed_origins = if cors_enabled {
        Some(
            config
                .cors_allowed_origins
                .iter()
                .map(|origin| origin.parse::<HeaderValue>())
                .collect::<Result<Vec<_>, _>>()?,
        )
    } else {
        None
    };

    let state = AppState::new(config).await?;
    let shared_state = Arc::new(state);

    info!("Database and Redis connections established.");
    print_banner(port);

    let cors = cors_allowed_origins.map(|allowed_origins| {
        CorsLayer::new()
            .allow_origin(AllowOrigin::list(allowed_origins))
            .allow_methods([
                Method::GET,
                Method::POST,
                Method::PUT,
                Method::PATCH,
                Method::DELETE,
                Method::OPTIONS,
            ])
            .allow_headers([
                axum::http::header::CONTENT_TYPE,
                axum::http::header::AUTHORIZATION,
                axum::http::header::ACCEPT,
                axum::http::header::COOKIE,
            ])
            .allow_credentials(true)
    });

    let service_builder = ServiceBuilder::new()
        .layer(TraceLayer::new_for_http())
        .option_layer(cors)
        .layer(CompressionLayer::new());

    let app = Router::new()
        .route("/", get(|| async { "OK" }))
        .route("/health", get(|| async { "OK" }))
        .merge(routes::create_router())
        .merge(gateway::router())
        .layer(service_builder)
        .with_state(shared_state)
        .fallback(|| async { (StatusCode::NOT_FOUND, "404 Not Found!") });

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    info!("[Core] Server listening on port {}", port);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;

    Ok(())
}
