use crate::audio_processor;
use crate::handlers::AppState;
use crate::models::{ClientType, Event};
use axum::extract::{
    ws::{Message, WebSocket, WebSocketUpgrade},
    State,
};
use axum::response::IntoResponse;
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use tracing::{debug, error, info};

#[derive(Deserialize)]
#[serde(tag = "action")]
enum ClientAction {
    #[serde(rename = "transcribe")]
    Transcribe { lang: String },

    #[serde(rename = "audio_data")]
    AudioData { data: String, format: String },
}

#[derive(Serialize)]
#[serde(tag = "type")]
enum ServerResponse {
    #[serde(rename = "ready")]
    Ready,

    #[serde(rename = "transcription")]
    Transcription { text: String, duration_secs: f32 },

    #[serde(rename = "error")]
    Error { message: String },
}

pub async fn websocket_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();

    // Handshake: recibir channel y client_id
    let (channel_id, client_id) = match receiver.next().await {
        Some(Ok(msg)) => {
            if let Ok(text) = msg.to_text() {
                match serde_json::from_str::<serde_json::Value>(text) {
                    Ok(obj) => {
                        let channel = obj.get("channel").and_then(|v| v.as_str());
                        let client = obj.get("client_id").and_then(|v| v.as_str());
                        match (channel, client) {
                            (Some(c), Some(cl)) => (c.to_string(), cl.to_string()),
                            _ => {
                                let _ = sender
                                    .send(Message::Text(
                                        json!({"error": "Missing channel or client_id"})
                                            .to_string(),
                                    ))
                                    .await;
                                return;
                            }
                        }
                    }
                    _ => {
                        let _ = sender
                            .send(Message::Text(json!({"error": "Invalid JSON"}).to_string()))
                            .await;
                        return;
                    }
                }
            } else {
                return;
            }
        }
        _ => return,
    };

    // Suscribirse al canal
    let tx =
        match state
            .channel_manager
            .subscribe(&channel_id, client_id.clone(), ClientType::WebSocket)
        {
            Ok(tx) => tx,
            Err(e) => {
                let _ = sender
                    .send(Message::Text(json!({"error": e}).to_string()))
                    .await;
                return;
            }
        };

    info!("WebSocket client {} connected to {}", client_id, channel_id);
    let mut rx = tx.subscribe();

    // Emitir evento de client_joined
    if let Some(clients) = state.channel_manager.get_clients(&channel_id) {
        let clients_list: Vec<String> = clients.iter().map(|c| c.id.clone()).collect();
        let join_event = Event::new(
            channel_id.clone(),
            "__system__".to_string(),
            vec!["*".to_string()],
            json!({
                "type": "client_joined",
                "client_id": client_id,
                "clients": clients_list,
                "total_clients": clients.len()
            }),
        );
        let _ = tx.send(join_event);
    }

    // Estado para manejar sesiones de transcripción
    let mut current_lang: Option<String> = None;

    loop {
        tokio::select! {
            result = rx.recv() => {
                match result {
                    Ok(event) => {
                        let should_receive = event.targets.contains(&"*".to_string()) || event.targets.contains(&client_id);
                        if should_receive {
                            if let Ok(json_str) = serde_json::to_string(&event) {
                                if let Err(e) = sender.send(Message::Text(json_str)).await {
                                    error!("Failed to send to client: {}", e);
                                    break;
                                }
                            }
                        } else {
                            debug!("Event for {:?} received but not for client {}", event.targets, client_id);
                        }
                    }
                    Err(_) => {
                        debug!("Channel broadcast closed");
                        break;
                    }
                }
            }
            msg = receiver.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<ClientAction>(&text) {
                            Ok(ClientAction::Transcribe { lang }) => {
                                current_lang = Some(lang);
                                let response = ServerResponse::Ready;
                                if let Ok(json_str) = serde_json::to_string(&response) {
                                    let _ = sender.send(Message::Text(json_str)).await;
                                }
                            }
                            Ok(ClientAction::AudioData { data, format: _ }) => {
                                if let Some(lang) = &current_lang {
                                    // Procesar audio en background
                                    let whisper_engine = Arc::clone(&state.whisper_engine);
                                    let lang_clone = lang.clone();
                                    let tx_clone = tx.clone();
                                    let channel_id_clone = channel_id.clone();
                                    let client_id_clone = client_id.clone();

                                    tokio::spawn(async move {
                                        // Convertir audio
                                        match audio_processor::webm_to_f32_samples(&data) {
                                            Ok(samples) => {
                                                if samples.is_empty() {
                                                    let response = ServerResponse::Error {
                                                        message: "No audio detected".to_string(),
                                                    };
                                                    return;
                                                }

                                                let duration = samples.len() as f32 / 16000.0;

                                                // Transcribir con whisper
                                                match whisper_engine.transcribe(&samples, &lang_clone) {
                                                    Ok(text) => {
                                                        info!("Transcription: \"{}\"", text);

                                                        // Enviar respuesta directa al cliente
                                                        let response = ServerResponse::Transcription {
                                                            text: text.clone(),
                                                            duration_secs: duration,
                                                        };
                                                        if let Ok(json_str) = serde_json::to_string(&response) {
                                                            // No podemos enviar aquí directamente, pero el event de abajo llegará
                                                        }

                                                        // Emitir event al canal (todos lo ven)
                                                        let event = Event::new(
                                                            channel_id_clone,
                                                            client_id_clone,
                                                            vec!["*".to_string()],
                                                            json!({
                                                                "type": "transcription",
                                                                "text": text,
                                                                "duration_secs": duration,
                                                            }),
                                                        );
                                                        let _ = tx_clone.send(event);
                                                    }
                                                    Err(e) => {
                                                        error!("Whisper transcription failed: {}", e);
                                                    }
                                                }
                                            }
                                            Err(e) => {
                                                error!("Audio processing failed: {}", e);
                                            }
                                        }
                                    });
                                }
                            }
                            Err(e) => {
                                debug!("Failed to parse client message: {}", e);
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        info!("Client {} disconnected", client_id);
                        break;
                    }
                    Some(Err(e)) => {
                        error!("WebSocket error: {}", e);
                        break;
                    }
                    _ => {}
                }
            }
        }
    }

    state.channel_manager.unsubscribe(&channel_id, &client_id);
}
