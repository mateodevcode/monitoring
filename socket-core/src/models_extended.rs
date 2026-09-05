use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Representa un llamado a tool que el LLM quiere ejecutar
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

/// Resultado de ejecutar una tooll
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub tool_call_id: String,
    pub name: String,
    pub content: String,
    pub is_error: bool,
}

/// Respuesta del LLM que puede contener:
/// - Tool calls a ejecutar
/// - O un texto final (sin más tool calls)
#[derive(Debug, Clone)]
pub enum LLmResponse {
    /// El LLM quiere ejecutar tools
    ToolCalls(Vec<ToolCall>),
    /// El LLM responde con texto final (fin de la conversación)
    FinalText(String),
}
