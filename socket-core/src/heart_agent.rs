use crate::models_extended::{LLmResponse, ToolCall};
use crate::prompts::load_system_prompt;
use crate::tool_registry::ToolRegistry;
use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use std::env;
use std::sync::Arc;
use tracing::{error, info};
use uuid::Uuid;

#[async_trait]
pub trait AiProvider: Send + Sync {
    /// Simple ask sin tools (para compatibilidad hacia atrás)
    async fn ask(&self, message: &str) -> Result<String, Box<dyn std::error::Error + Send + Sync>>;

    /// Ask con soporte de tools y function calling
    async fn ask_with_tools(
        &self,
        user_message: &str,
        messages_history: &[Value],
        tool_registry: &ToolRegistry,
    ) -> Result<LLmResponse, Box<dyn std::error::Error + Send + Sync>>;
}

#[derive(Clone)]
pub struct AiConfig {
    pub provider: String,
    pub api_key: String,
    pub model: String,
}

impl AiConfig {
    pub fn from_env() -> Self {
        Self {
            provider: env::var("AI_PROVIDER").unwrap_or_else(|_| "deepseek".to_string()),
            api_key: env::var("AI_API_KEY").unwrap_or_else(|_| String::new()),
            model: env::var("AI_MODEL").unwrap_or_else(|_| "deepseek-chat".to_string()),
        }
    }
}

// ============================================================================
// GEMINI PROVIDER
// ============================================================================

pub struct GeminiProvider {
    client: Client,
    api_key: String,
    model: String,
}

impl GeminiProvider {
    pub fn new(api_key: String, model: String) -> Self {
        Self {
            client: Client::new(),
            api_key,
            model,
        }
    }

    /// Método auxiliar: convierte OpenAI schema a Gemini schema
    fn convert_to_gemini_tools(&self, openai_schema: &Value) -> Vec<Value> {
        if let Some(tools_arr) = openai_schema.as_array() {
            tools_arr
                .iter()
                .filter_map(|tool| {
                    let func = tool.get("function")?;
                    let name = func.get("name")?.as_str()?;
                    let description = func.get("description")?.as_str()?;
                    let parameters = func.get("parameters")?;

                    Some(json!({
                        "name": name,
                        "description": description,
                        "parameters": parameters
                    }))
                })
                .collect()
        } else {
            vec![]
        }
    }
}

#[async_trait]
impl AiProvider for GeminiProvider {
    async fn ask(&self, message: &str) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let system_prompt = load_system_prompt();
        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
            self.model, self.api_key
        );
        let payload = json!({
            "contents": [{"parts": [{"text": format!("{}\n\nUsuario: {}", system_prompt, message)}]}]
        });
        let response = self
            .client
            .post(&url)
            .json(&payload)
            .send()
            .await?
            .json::<Value>()
            .await?;

        if response.get("candidates").is_none() {
            error!("❌ Respuesta inesperada de Gemini: {}", response);
        }

        Ok(response["candidates"][0]["content"]["parts"][0]["text"]
            .as_str()
            .unwrap_or("No pude procesar eso, jefe.")
            .to_string())
    }

    async fn ask_with_tools(
        &self,
        user_message: &str,
        messages_history: &[Value],
        tool_registry: &ToolRegistry,
    ) -> Result<LLmResponse, Box<dyn std::error::Error + Send + Sync>> {
        let system_prompt = load_system_prompt();
        let tools_schema = tool_registry.to_openai_schema();
        let gemini_tools = self.convert_to_gemini_tools(&tools_schema);

        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
            self.model, self.api_key
        );

        let payload = json!({
            "system_instruction": {"parts": [{"text": system_prompt}]},
            "contents": [{"parts": [{"text": user_message}]}],
            "tools": [{"function_declarations": gemini_tools}]
        });

        info!("📤 Enviando a Gemini con tools: {:?}", payload);

        let response = self
            .client
            .post(&url)
            .json(&payload)
            .send()
            .await?
            .json::<Value>()
            .await?;

        info!("📥 Respuesta de Gemini: {:?}", response);

        // Parsear respuesta: puede ser function_call o text
        if let Some(candidates) = response.get("candidates").and_then(|v| v.as_array()) {
            if let Some(candidate) = candidates.first() {
                if let Some(content) = candidate.get("content") {
                    if let Some(parts) = content.get("parts").and_then(|v| v.as_array()) {
                        // Checa si hay function_call
                        for part in parts {
                            if let Some(fn_call) = part.get("functionCall") {
                                let name = fn_call
                                    .get("name")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let args =
                                    fn_call.get("args").cloned().unwrap_or_else(|| json!({}));

                                let tool_call = ToolCall {
                                    id: Uuid::new_v4().to_string(),
                                    name,
                                    arguments: args,
                                };

                                info!("🛠️  Gemini quiere ejecutar tool: {:?}", tool_call);
                                return Ok(LLmResponse::ToolCalls(vec![tool_call]));
                            }
                        }

                        // Si no hay function_call, busca texto
                        for part in parts {
                            if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                                return Ok(LLmResponse::FinalText(text.to_string()));
                            }
                        }
                    }
                }
            }
        }

        Err("No se pudo parsear respuesta de Gemini".into())
    }
}

// ============================================================================
// OPENAI PROVIDER
// ============================================================================

pub struct OpenAiProvider {
    client: Client,
    api_key: String,
    model: String,
}

impl OpenAiProvider {
    pub fn new(api_key: String, model: String) -> Self {
        Self {
            client: Client::new(),
            api_key,
            model,
        }
    }
}

#[async_trait]
impl AiProvider for OpenAiProvider {
    async fn ask(&self, message: &str) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let system_prompt = load_system_prompt();
        let url = "https://api.openai.com/v1/chat/completions";
        let payload = json!({
            "model": self.model,
            "messages": [
                {"role": "system", "content": system_prompt},
                {"role": "user", "content": message}
            ]
        });
        let response = self
            .client
            .post(url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&payload)
            .send()
            .await?
            .json::<Value>()
            .await?;

        Ok(response["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("No pude procesar eso, jefe.")
            .to_string())
    }

    async fn ask_with_tools(
        &self,
        user_message: &str,
        messages_history: &[Value],
        tool_registry: &ToolRegistry,
    ) -> Result<LLmResponse, Box<dyn std::error::Error + Send + Sync>> {
        let system_prompt = load_system_prompt();
        let tools_schema = tool_registry.to_openai_schema();

        let mut messages = vec![json!({"role": "system", "content": system_prompt})];
        messages.extend_from_slice(messages_history);

        let url = "https://api.openai.com/v1/chat/completions";
        let payload = json!({
            "model": self.model,
            "messages": messages,
            "tools": tools_schema,
            "tool_choice": "auto"
        });

        info!("📤 Enviando a OpenAI con tools");

        let response = self
            .client
            .post(url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&payload)
            .send()
            .await?
            .json::<Value>()
            .await?;

        if let Some(choices) = response.get("choices").and_then(|v| v.as_array()) {
            if let Some(choice) = choices.first() {
                if let Some(message) = choice.get("message") {
                    // Checa si hay tool_calls
                    if let Some(tool_calls) = message.get("tool_calls").and_then(|v| v.as_array()) {
                        let mut calls = vec![];
                        for tc in tool_calls {
                            let id = tc
                                .get("id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let name = tc
                                .get("function")
                                .and_then(|f| f.get("name"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();

                            let args_str = tc
                                .get("function")
                                .and_then(|f| f.get("arguments"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("{}");

                            let arguments: Value =
                                serde_json::from_str(args_str).unwrap_or_else(|_| json!({}));

                            calls.push(ToolCall {
                                id,
                                name,
                                arguments,
                            });
                        }

                        if !calls.is_empty() {
                            info!("🛠️  OpenAI quiere ejecutar {} tools", calls.len());
                            return Ok(LLmResponse::ToolCalls(calls));
                        }
                    }

                    // Si no hay tool_calls, busca texto
                    if let Some(text) = message.get("content").and_then(|v| v.as_str()) {
                        return Ok(LLmResponse::FinalText(text.to_string()));
                    }
                }
            }
        }

        Err("No se pudo parsear respuesta de OpenAI".into())
    }
}

// ============================================================================
// DEEPSEEK PROVIDER
// ============================================================================

pub struct DeepSeekProvider {
    client: Client,
    api_key: String,
    model: String,
}

impl DeepSeekProvider {
    pub fn new(api_key: String, model: String) -> Self {
        Self {
            client: Client::new(),
            api_key,
            model,
        }
    }
}

#[async_trait]
impl AiProvider for DeepSeekProvider {
    async fn ask(&self, message: &str) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let system_prompt = load_system_prompt();
        let url = "https://api.deepseek.com/chat/completions";
        let payload = json!({
            "model": self.model,
            "messages": [
                {"role": "system", "content": system_prompt},
                {"role": "user", "content": message}
            ]
        });
        let response = self
            .client
            .post(url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&payload)
            .send()
            .await?
            .json::<Value>()
            .await?;

        Ok(response["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("No pude procesar eso, jefe.")
            .to_string())
    }

    async fn ask_with_tools(
        &self,
        user_message: &str,
        messages_history: &[Value],
        tool_registry: &ToolRegistry,
    ) -> Result<LLmResponse, Box<dyn std::error::Error + Send + Sync>> {
        let system_prompt = load_system_prompt();
        let tools_schema = tool_registry.to_openai_schema();

        let mut messages = vec![json!({"role": "system", "content": system_prompt})];
        messages.extend_from_slice(messages_history);

        let url = "https://api.deepseek.com/chat/completions";
        let payload = json!({
            "model": self.model,
            "messages": messages,
            "tools": tools_schema,
            "tool_choice": "auto"
        });

        info!("📤 Enviando a DeepSeek con tools");

        let response = self
            .client
            .post(url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&payload)
            .send()
            .await?
            .json::<Value>()
            .await?;

        if let Some(choices) = response.get("choices").and_then(|v| v.as_array()) {
            if let Some(choice) = choices.first() {
                if let Some(message) = choice.get("message") {
                    // DeepSeek usa el mismo formato que OpenAI
                    if let Some(tool_calls) = message.get("tool_calls").and_then(|v| v.as_array()) {
                        let mut calls = vec![];
                        for tc in tool_calls {
                            let id = tc
                                .get("id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let name = tc
                                .get("function")
                                .and_then(|f| f.get("name"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();

                            let args_str = tc
                                .get("function")
                                .and_then(|f| f.get("arguments"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("{}");

                            let arguments: Value =
                                serde_json::from_str(args_str).unwrap_or_else(|_| json!({}));

                            calls.push(ToolCall {
                                id,
                                name,
                                arguments,
                            });
                        }

                        if !calls.is_empty() {
                            info!("🛠️  DeepSeek quiere ejecutar {} tools", calls.len());
                            return Ok(LLmResponse::ToolCalls(calls));
                        }
                    }

                    // Si no hay tool_calls, busca texto
                    if let Some(text) = message.get("content").and_then(|v| v.as_str()) {
                        return Ok(LLmResponse::FinalText(text.to_string()));
                    }
                }
            }
        }

        Err("No se pudo parsear respuesta de DeepSeek".into())
    }
}

// ============================================================================
// FACTORY
// ============================================================================

pub fn create_provider(config: &AiConfig) -> Arc<dyn AiProvider> {
    match config.provider.to_lowercase().as_str() {
        "gemini" => Arc::new(GeminiProvider::new(
            config.api_key.clone(),
            config.model.clone(),
        )),
        "openai" => Arc::new(OpenAiProvider::new(
            config.api_key.clone(),
            config.model.clone(),
        )),
        "deepseek" => Arc::new(DeepSeekProvider::new(
            config.api_key.clone(),
            config.model.clone(),
        )),
        _ => {
            error!(
                "Proveedor desconocido: {}. Usando DeepSeek.",
                config.provider
            );
            Arc::new(DeepSeekProvider::new(
                config.api_key.clone(),
                config.model.clone(),
            ))
        }
    }
}
