use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub id: String,
    pub channel: String,
    pub source: String,
    pub targets: Vec<String>,
    pub payload: serde_json::Value,
    pub timestamp: i64,
}

impl Event {
    pub fn new(
        channel: String,
        source: String,
        targets: Vec<String>,
        payload: serde_json::Value,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            channel,
            source,
            targets,
            payload,
            timestamp: chrono::Utc::now().timestamp(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Channel {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub created_at: i64,
    pub clients_count: usize,
}

impl Channel {
    pub fn new(name: String, description: Option<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            name,
            description,
            created_at: chrono::Utc::now().timestamp(),
            clients_count: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientInfo {
    pub id: String,
    pub client_type: ClientType,
    pub connected_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
#[allow(dead_code)] // Permitido porque usaremos Webhook/Internal en el futuro
pub enum ClientType {
    WebSocket,
    Webhook(String),
    Internal,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ApiResponse<T> {
    pub success: bool,
    pub data: Option<T>,
    pub error: Option<String>,
}

impl<T> ApiResponse<T> {
    pub fn ok(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateChannelRequest {
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct EmitEventRequest {
    pub source: String,
    pub targets: Vec<String>,
    pub payload: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct ServerStats {
    pub channels: usize,
    pub total_clients: usize,
}
