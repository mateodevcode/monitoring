use crate::audio_processor;
use crate::handlers::AppState;
use crate::models::{ClientType, Event};
use axum::extract::{
    ws::{Message, WebSocket, WebSocketUpgrade},
    State,
};
use axum::response::IntoResponse;
use futures::{SinkExt, StreamExt};
use serde_json::json;
use std::sync::Arc;
use tracing::{debug, error, info};

pub async fn websocket_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();

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

    if let Some(clients) = state.channel_manager.get_clients(&channel_id) {
        let clients_list: Vec<String> = clients.iter().map(|c| c.id.clone()).collect();
        let join_event = Event::new(
            channel_id.clone(),
            "__system__".to_string(),
            vec!["*".to_string()],
            json!({ "type": "client_joined", "client_id": client_id, "clients": clients_list, "total_clients": clients.len() }),
        );
        let _ = tx.send(join_event);
    }

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
                    Err(_) => { debug!("Channel broadcast closed"); break; }
                }
            }
            msg = receiver.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(obj) = serde_json::from_str::<serde_json::Value>(&text) {
                            if let Some(action) = obj.get("action").and_then(|v| v.as_str()) {
                                if action == "transcribe" {
                                    if let Some(lang) = obj.get("lang").and_then(|v| v.as_str()) {
                                        current_lang = Some(lang.to_string());
                                        let _ = sender.send(Message::Text(json!({"type": "ready"}).to_string())).await;
                                    }
                                }
                            }
                        }
                    }
                    Some(Ok(Message::Binary(data))) => {
                        if let Some(lang) = &current_lang {
                            let _ = tx.send(Event::new(
                                channel_id.clone(), "__system__".to_string(), vec![client_id.clone()],
                                json!({ "type": "status", "message": "🎧 Audio recibido, procesando..." }),
                            ));

                            let whisper_engine = Arc::clone(&state.whisper_engine);
                            let heart_agent = Arc::clone(&state.heart_agent);
                            let lang_clone = lang.clone();
                            let tx_clone = tx.clone();
                            let channel_id_clone = channel_id.clone();
                            let client_id_clone = client_id.clone();
                            let audio_bytes = data.clone();

                            tokio::task::spawn_blocking(move || {
                                let t0 = std::time::Instant::now();
                                match audio_processor::webm_to_f32_samples(&audio_bytes) {
                                    Ok(samples) => {
                                        info!("⏱️ ffmpeg+wav: {:?}", t0.elapsed());
                                        if samples.is_empty() {
                                            let _ = tx_clone.send(Event::new(channel_id_clone.clone(), "__system__".to_string(), vec![client_id_clone.clone()], json!({ "type": "error", "message": "No se detectó audio" })));
                                            return;
                                        }
                                        let duration = samples.len() as f32 / 16000.0;
                                        match whisper_engine.transcribe(&samples, &lang_clone) {
                                            Ok(text) => {
                                                info!("⏱️ TOTAL: {:?} | Transcripción: \"{}\"", t0.elapsed(), text);

                                                // 1. Enviar transcripción cruda
                                                let _ = tx_clone.send(Event::new(
                                                    channel_id_clone.clone(), client_id_clone.clone(),
                                                    vec!["*".to_string(), client_id_clone.clone()],
                                                    json!({ "type": "transcription", "text": text.clone(), "duration_secs": duration }),
                                                ));

                                                // 2. Delegar al Heart Agent asíncronamente
                                                let agent_clone = Arc::clone(&heart_agent);
                                                let tx_ai = tx_clone.clone();
                                                let cid_clone = channel_id_clone.clone();
                                                let client_for_error = client_id_clone.clone();
                                                let text_owned = text.clone();

                                                tokio::spawn(async move {
                                                    match agent_clone.ask(&text_owned).await {
                                                        Ok(ai_response) => {
                                                            info!("🤖 JARVIS responde: {}", ai_response);
                                                            let _ = tx_ai.send(Event::new(
                                                                cid_clone, "heart_agent".to_string(), vec!["*".to_string()],
                                                                json!({ "type": "ai_response", "text": ai_response, "original_text": text_owned }),
                                                            ));
                                                        }
                                                        Err(e) => {
                                                            error!("Heart Agent falló: {}", e);
                                                            let _ = tx_ai.send(Event::new(
                                                                cid_clone, "__system__".to_string(), vec![client_for_error],
                                                                json!({ "type": "error", "message": format!("IA no disponible: {}", e) }),
                                                            ));
                                                        }
                                                    }
                                                });
                                            }
                                            Err(e) => {
                                                error!("Whisper failed: {}", e);
                                                let _ = tx_clone.send(Event::new(channel_id_clone.clone(), "__system__".to_string(), vec![client_id_clone.clone()], json!({ "type": "error", "message": format!("Error de transcripción: {}", e) })));
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        error!("Audio processing failed: {}", e);
                                        let _ = tx_clone.send(Event::new(channel_id_clone.clone(), "__system__".to_string(), vec![client_id_clone.clone()], json!({ "type": "error", "message": format!("Error procesando audio: {}", e) })));
                                    }
                                }
                            });
                        } else {
                            let _ = sender.send(Message::Text(json!({"type": "error", "message": "Debes enviar acción 'transcribe' con 'lang' primero"}).to_string())).await;
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
