// src/websocket.rs
use crate::handlers::AppState;
use crate::models::{ClientType, Event};
use axum::extract::{
    ws::{Message, WebSocket, WebSocketUpgrade},
    State,
};
use axum::response::IntoResponse;
use futures::{SinkExt, StreamExt};
use serde_json::json;
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
                    Some(Ok(_)) => { /* Ignorar mensajes del cliente por ahora */ }
                    Some(Err(e)) => { error!("WebSocket error: {}", e); break; }
                    None => { info!("Client {} disconnected", client_id); break; }
                }
            }
        }
    }

    state.channel_manager.unsubscribe(&channel_id, &client_id);
}
