mod audio_processor;
mod channel_manager;
mod handlers;
mod models;
mod websocket;
mod whisper_engine;

use axum::{
    extract::DefaultBodyLimit,
    routing::{get, post},
    Router,
};
use handlers::{
    create_channel, emit_event, get_channel, get_channel_clients, health, list_channels, stats,
    AppState,
};
use std::net::SocketAddr;
use std::sync::Arc;
use tower_http::cors::CorsLayer;
use tracing::info;
use whisper_engine::WhisperEngine;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".parse().unwrap()),
        )
        .init();

    // Inicializar el motor de Whisper UNA vez al arrancar
    // El modelo se descarga automáticamente en el Dockerfile
    let model_path = std::env::var("WHISPER_MODEL_PATH")
        .unwrap_or_else(|_| "/app/models/ggml-small.bin".to_string());

    let whisper_engine = match WhisperEngine::new(&model_path) {
        Ok(engine) => {
            info!("✅ Whisper engine initialized with model: {}", model_path);
            Arc::new(engine)
        }
        Err(e) => {
            tracing::error!("❌ Failed to initialize Whisper engine: {}", e);
            std::process::exit(1);
        }
    };

    let channel_manager = channel_manager::ChannelManager::new();
    let state = AppState {
        channel_manager,
        whisper_engine,
    };

    let app = Router::new()
        .route("/health", get(health))
        .route("/stats", get(stats))
        .route("/channels", post(create_channel))
        .route("/channels", get(list_channels))
        .route("/channels/:id", get(get_channel))
        .route("/channels/:id/clients", get(get_channel_clients))
        .route("/channels/:id/events", post(emit_event))
        .route("/ws", get(websocket::websocket_handler))
        .layer(DefaultBodyLimit::max(10 * 1024 * 1024)) // 10MB
        .layer(CorsLayer::permissive())
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], 3005));
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();

    info!("🚀 Socket Server running on http://0.0.0.0:3005");
    info!("📊 Health check: http://localhost:3005/health");

    axum::serve(listener, app).await.unwrap();
}
