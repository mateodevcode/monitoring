use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

#[derive(Debug)]
pub struct ToolError(pub String);

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for ToolError {}

impl From<String> for ToolError {
    fn from(s: String) -> Self {
        ToolError(s)
    }
}

/// Interfaz genérica para cualquier capacidad que el Heart Agent pueda delegar:
/// puede ser código Rust nativo (SSH, lo que sea) o un adaptador hacia un servidor MCP.
/// El registro no distingue el origen — ambos implementan lo mismo.
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    /// JSON Schema de los parámetros, en formato compatible con function calling
    /// estilo OpenAI/DeepSeek.
    fn parameters_schema(&self) -> Value;
    async fn execute(&self, args: Value) -> Result<String, ToolError>;
}

#[derive(Clone, Default)]
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        tracing::info!("🛠️  Tool registrada: {}", tool.name());
        self.tools.insert(tool.name().to_string(), tool);
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Listado en formato "tools" compatible con function calling (OpenAI/DeepSeek).
    /// Esto es lo que se le manda al LLM en cada petición para que sepa qué puede invocar.
    pub fn to_openai_schema(&self) -> Value {
        let tools: Vec<Value> = self
            .tools
            .values()
            .map(|t| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": t.name(),
                        "description": t.description(),
                        "parameters": t.parameters_schema(),
                    }
                })
            })
            .collect();
        Value::Array(tools)
    }

    pub async fn execute(&self, name: &str, args: Value) -> Result<String, ToolError> {
        match self.get(name) {
            Some(tool) => tool.execute(args).await,
            None => Err(ToolError(format!("La tool '{}' no existe", name))),
        }
    }
}
