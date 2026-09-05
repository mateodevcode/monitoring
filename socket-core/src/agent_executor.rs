use crate::heart_agent::AiProvider;
use crate::models_extended::{LLmResponse, ToolCall, ToolResult};
use crate::tool_registry::ToolRegistry;
use serde_json::{json, Value};
use std::sync::Arc;
use tracing::{info, warn};

/// Orquesta la ejecución del agente: pregunta al LLM, ejecuta tools, itera hasta respuesta final
pub struct AgentExecutor {
    ai_provider: Arc<dyn AiProvider>,
    tool_registry: Arc<ToolRegistry>,
}

impl AgentExecutor {
    pub fn new(ai_provider: Arc<dyn AiProvider>, tool_registry: Arc<ToolRegistry>) -> Self {
        Self {
            ai_provider,
            tool_registry,
        }
    }

    /// Ejecuta el loop agentico completo: pregunta → tools → respuesta final
    pub async fn execute(&self, user_message: &str) -> Result<String, String> {
        let max_iterations = 5;
        let mut iteration = 0;
        let mut messages: Vec<Value> = vec![json!({"role": "user", "content": user_message})];

        loop {
            iteration += 1;
            if iteration > max_iterations {
                warn!("⚠️ Max iteraciones alcanzadas, devolviendo respuesta parcial");
                return Ok("Jefe, hice mi mejor esfuerzo pero necesito más tiempo.".to_string());
            }

            info!("🔄 Iteración {} del agente agentico", iteration);

            // 1. Pregunta al LLM con el contexto actual (mensajes + tools disponibles)
            let response = self
                .ai_provider
                .ask_with_tools(user_message, &messages, &self.tool_registry)
                .await
                .map_err(|e| format!("Error LLM: {}", e))?;

            match response {
                LLmResponse::FinalText(text) => {
                    info!("✅ Agente finalizó con respuesta: {}", text);
                    return Ok(text);
                }
                LLmResponse::ToolCalls(tool_calls) => {
                    info!("🛠️  Agente quiere ejecutar {} tools", tool_calls.len());

                    let mut tool_results = Vec::new();

                    // 2. Ejecuta cada tool call
                    for tool_call in tool_calls {
                        let result = self.execute_tool_call(&tool_call).await;
                        tool_results.push(result);
                    }

                    // 3. Agrega los resultados al historial de mensajes
                    // (así el LLM ve qué tools ejecutamos y qué resultados tuvieron)
                    for result in tool_results {
                        messages.push(json!({
                            "role": "tool",
                            "tool_call_id": result.tool_call_id,
                            "name": result.name,
                            "content": result.content
                        }));
                    }

                    // 4. Siguiente iteración: el LLM responde basándose en los resultados
                }
            }
        }
    }

    /// Ejecuta un tool call individual
    async fn execute_tool_call(&self, tool_call: &ToolCall) -> ToolResult {
        info!(
            "🔌 Ejecutando tool: {} con args: {:?}",
            tool_call.name, tool_call.arguments
        );

        let result = self
            .tool_registry
            .execute(&tool_call.name, tool_call.arguments.clone())
            .await;

        match result {
            Ok(content) => {
                info!("✅ Tool {} ejecutada exitosamente", tool_call.name);
                ToolResult {
                    tool_call_id: tool_call.id.clone(),
                    name: tool_call.name.clone(),
                    content,
                    is_error: false,
                }
            }
            Err(e) => {
                warn!("❌ Tool {} falló: {}", tool_call.name, e);
                ToolResult {
                    tool_call_id: tool_call.id.clone(),
                    name: tool_call.name.clone(),
                    content: format!("Error: {}", e),
                    is_error: true,
                }
            }
        }
    }
}
