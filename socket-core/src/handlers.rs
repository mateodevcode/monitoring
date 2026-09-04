use crate::channel_manager::ChannelManager;
use crate::models::*;
use crate::whisper_engine::WhisperEngine;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde_json::json;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub channel_manager: ChannelManager,
    pub whisper_engine: Arc<WhisperEngine>,
}

pub async fn health() -> impl IntoResponse {
    Json(json!({"status": "ok"}))
}

pub async fn stats(State(state): State<AppState>) -> impl IntoResponse {
    let (channels, clients) = state.channel_manager.stats();
    Json(ApiResponse::ok(ServerStats {
        channels,
        total_clients: clients,
    }))
}

pub async fn create_channel(
    State(state): State<AppState>,
    Json(req): Json<CreateChannelRequest>,
) -> impl IntoResponse {
    match state
        .channel_manager
        .create_channel(req.name, req.description)
    {
        Ok(channel) => (StatusCode::CREATED, Json(ApiResponse::ok(channel))).into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::<()> {
                success: false,
                data: None,
                error: Some(e),
            }),
        )
            .into_response(),
    }
}

pub async fn list_channels(State(state): State<AppState>) -> impl IntoResponse {
    Json(ApiResponse::ok(state.channel_manager.list_channels()))
}

pub async fn get_channel(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.channel_manager.get_channel(&id) {
        Some(channel) => Json(ApiResponse::ok(channel)).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()> {
                success: false,
                data: None,
                error: Some("Channel not found".to_string()),
            }),
        )
            .into_response(),
    }
}

pub async fn get_channel_clients(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.channel_manager.get_clients(&id) {
        Some(clients) => Json(ApiResponse::ok(clients)).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()> {
                success: false,
                data: None,
                error: Some("Channel not found".to_string()),
            }),
        )
            .into_response(),
    }
}

pub async fn emit_event(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<EmitEventRequest>,
) -> impl IntoResponse {
    let event = Event::new(id.clone(), req.source, req.targets, req.payload);
    match state.channel_manager.emit_event(&id, event) {
        Ok(_) => (
            StatusCode::OK,
            Json(ApiResponse::ok(json!({"success": true}))),
        )
            .into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::<()> {
                success: false,
                data: None,
                error: Some(e),
            }),
        )
            .into_response(),
    }
}
