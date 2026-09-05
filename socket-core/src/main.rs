mod audio_processor;
mod channel_manager;
mod handlers;
mod heart_agent;
mod models;
mod prompts;
mod ssh_tool;
mod tool_registry;
mod tts_engine;
mod vps_config;
mod websocket;
mod whisper_engine;

use axum::{
    extract::DefaultBodyLimit,
    routing::{get, post},
    Router,
};
use handlers::{
    create_channel, debug_execute_tool, emit_event, get_channel, get_channel_clients, health,
    list_channels, stats, AppState,
};
use heart_agent::{create_provider, AiConfig};
use ssh_tool::SshCommandTool;
use std::net::SocketAddr;
use std::sync::Arc;
use tool_registry::ToolRegistry;
use tower_http::cors::CorsLayer;
use tracing::info;
use tts_engine::TtsEngine;
use vps_config::VpsConfig;

#[tokio::main]
async fn main() {
    // 1. Cargar variables de entorno desde .env (si existe)
    let _ = dotenv::dotenv();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".parse().unwrap()),
        )
        .init();

    // 2. Inicializar Whisper
    let model_path = std::env::var("WHISPER_MODEL_PATH")
        .unwrap_or_else(|_| "/app/models/ggml-small.bin".to_string());

    let whisper_engine = match whisper_engine::WhisperEngine::new(&model_path) {
        Ok(engine) => {
            info!("✅ Whisper engine initialized: {}", model_path);
            Arc::new(engine)
        }
        Err(e) => {
            tracing::error!("❌ Failed to initialize Whisper: {}", e);
            std::process::exit(1);
        }
    };

    // 3. Inicializar Heart Agent (IA)
    let ai_config = AiConfig::from_env();
    let heart_agent = create_provider(&ai_config);
    info!("🤖 AI Provider configured: {}", ai_config.provider);

    // 3.5. Inicializar TTS Engine (reutiliza AI_API_KEY de Gemini)
    let tts_engine = Arc::new(TtsEngine::from_env());
    info!("🔊 TTS engine configured");

    // 3.6. Cargar registro de VPS y construir el Tool Registry
    let vps_config_path =
        std::env::var("VPS_CONFIG_PATH").unwrap_or_else(|_| "/app/vps.toml".to_string());
    let vps_config = match VpsConfig::load(&vps_config_path) {
        Ok(cfg) => {
            info!("🖥️  VPS registrados: {:?}", cfg.vps_ids());
            cfg
        }
        Err(e) => {
            tracing::warn!(
                "⚠️ No se pudo cargar {} ({}). Arrancando sin VPS registrados.",
                vps_config_path,
                e
            );
            VpsConfig::default()
        }
    };

    let mut tool_registry = ToolRegistry::new();
    tool_registry.register(Arc::new(SshCommandTool::new(vps_config)));
    let tool_registry = Arc::new(tool_registry);

    // 4. Crear el estado compartido
    let channel_manager = channel_manager::ChannelManager::new();
    let state = AppState {
        channel_manager,
        whisper_engine,
        heart_agent,
        tts_engine,
        tool_registry,
    };

    let app = Router::new()
        .route("/health", get(health))
        .route("/stats", get(stats))
        .route("/channels", post(create_channel))
        .route("/channels", get(list_channels))
        .route("/channels/:id", get(get_channel))
        .route("/channels/:id/clients", get(get_channel_clients))
        .route("/channels/:id/events", post(emit_event))
        .route("/debug/tools/execute", post(debug_execute_tool))
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
