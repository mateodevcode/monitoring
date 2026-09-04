use crate::prompts::load_system_prompt;
use async_trait::async_trait;
use reqwest::Client;
use serde_json::json;
use std::env;
use std::sync::Arc;
use tracing::error;

#[async_trait]
pub trait AiProvider: Send + Sync {
    async fn ask(&self, message: &str) -> Result<String, Box<dyn std::error::Error + Send + Sync>>;
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
            provider: env::var("AI_PROVIDER").unwrap_or_else(|_| "gemini".to_string()),
            api_key: env::var("AI_API_KEY").unwrap_or_else(|_| String::new()),
            model: env::var("AI_MODEL").unwrap_or_else(|_| "gemini-2.5-flash".to_string()),
        }
    }
}

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
            .json::<serde_json::Value>()
            .await?;

        // 👇 AÑADIR ESTO para depurar
        if response.get("candidates").is_none() {
            tracing::error!("❌ Respuesta inesperada de Gemini: {}", response);
        }

        Ok(response["candidates"][0]["content"]["parts"][0]["text"]
            .as_str()
            .unwrap_or("No pude procesar eso, jefe.")
            .to_string())
    }
}

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
            .json::<serde_json::Value>()
            .await?;
        Ok(response["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("No pude procesar eso, jefe.")
            .to_string())
    }
}

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
            .json::<serde_json::Value>()
            .await?;
        Ok(response["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("No pude procesar eso, jefe.")
            .to_string())
    }
}

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
            error!("Proveedor desconocido: {}. Usando Gemini.", config.provider);
            Arc::new(GeminiProvider::new(
                config.api_key.clone(),
                config.model.clone(),
            ))
        }
    }
}
