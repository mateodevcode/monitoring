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
#[allow(dead_code)]
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

    // Handshake
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

    // Emitir client_joined
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

    let mut current_lang: Option<String> = None;

    loop {
        tokio::select! {
            result = rx.recv() => {
                match result {
                    Ok(event) => {
                        info!("📥 Evento recibido en rx: {:?}", event);
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
                        match serde_json::from_str::<serde_json::Value>(&text) {
                            Ok(obj) => {
                                if let Some(action) = obj.get("action").and_then(|v| v.as_str()) {
                                    match action {
                                        "transcribe" => {
                                            if let Some(lang) = obj.get("lang").and_then(|v| v.as_str()) {
                                                current_lang = Some(lang.to_string());
                                                let _ = sender.send(Message::Text(
                                                    json!({"type": "ready"}).to_string()
                                                )).await;
                                            }
                                        }
                                        _ => {
                                            debug!("Unknown action: {}", action);
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                debug!("Failed to parse client message: {}", e);
                            }
                        }
                    }
                    Some(Ok(Message::Binary(data))) => {
                        if let Some(lang) = &current_lang {
                            // Enviar "procesando" inmediatamente
                            let _ = tx.send(Event::new(
                                channel_id.clone(),
                                "__system__".to_string(),
                                vec![client_id.clone()],
                                json!({ "type": "status", "message": "🎧 Audio recibido, procesando..." }),
                            ));

                            let whisper_engine = Arc::clone(&state.whisper_engine);
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
                                            let _ = tx_clone.send(Event::new(
                                                channel_id_clone, "__system__".to_string(),
                                                vec![client_id_clone],
                                                json!({ "type": "error", "message": "No se detectó audio" }),
                                            ));
                                            return;
                                        }

                                        let duration = samples.len() as f32 / 16000.0;

                                        match whisper_engine.transcribe(&samples, &lang_clone) {
                                            Ok(text) => {
                                                info!("⏱️ TOTAL: {:?} | \"{}\"", t0.elapsed(), text);

                                                // 🔥 CREAR EVENTO CON TARGET EXPLÍCITO
                                                let event = Event::new(
                                                    channel_id_clone,
                                                    client_id_clone.clone(),
                                                    vec!["*".to_string(), client_id_clone.clone()],
                                                    json!({ "type": "transcription", "text": text, "duration_secs": duration }),
                                                );

                                                // 📡 LOG ANTES DE ENVIAR
                                                info!("📤 Enviando evento de transcripción a cliente {}: {:?}", client_id_clone, event);

                                                match tx_clone.send(event) {
                                                    Ok(_) => {
                                                        info!("✅ Evento enviado exitosamente");
                                                    }
                                                    Err(e) => {
                                                        error!("❌ Error al enviar evento: {}", e);
                                                    }
                                                }
                                            }
                                            Err(e) => {
                                                error!("Whisper failed: {}", e);
                                                let _ = tx_clone.send(Event::new(
                                                    channel_id_clone, "__system__".to_string(),
                                                    vec![client_id_clone],
                                                    json!({ "type": "error", "message": e }),
                                                ));
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        error!("Audio processing failed: {}", e);
                                        let _ = tx_clone.send(Event::new(
                                            channel_id_clone, "__system__".to_string(),
                                            vec![client_id_clone],
                                            json!({ "type": "error", "message": e }),
                                        ));
                                    }
                                }
                            });
                        } else {
                            let _ = sender.send(Message::Text(
                                json!({"type": "error", "message": "Debes enviar 'transcribe' primero"}).to_string()
                            )).await;
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
