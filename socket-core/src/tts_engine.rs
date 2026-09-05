use base64::{engine::general_purpose::STANDARD, Engine as _};
use reqwest::Client;
use serde_json::json;
use std::env;

pub struct TtsEngine {
    client: Client,
    api_key: String,
    model: String,
    voice: String,
}

impl TtsEngine {
    pub fn from_env() -> Self {
        Self {
            client: Client::new(),
            // Reutilizamos la misma API key de Gemini que ya usa el Heart Agent
            api_key: env::var("AI_API_KEY").unwrap_or_default(),
            model: env::var("TTS_MODEL").unwrap_or_else(|_| "gemini-2.5-flash-preview-tts".to_string()),
            // Voces disponibles (entre otras): Kore, Puck, Leda, Achernar, Zephyr...
            voice: env::var("TTS_VOICE").unwrap_or_else(|_| "Kore".to_string()),
        }
    }

    /// Convierte texto a un WAV (bytes) listo para reproducir en el navegador.
    pub async fn synthesize(
        &self,
        text: &str,
    ) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
            self.model, self.api_key
        );

        let payload = json!({
            "contents": [{ "parts": [{ "text": text }] }],
            "generationConfig": {
                "responseModalities": ["AUDIO"],
                "speechConfig": {
                    "voiceConfig": {
                        "prebuiltVoiceConfig": { "voiceName": self.voice }
                    }
                }
            }
        });

        let response = self
            .client
            .post(&url)
            .json(&payload)
            .send()
            .await?
            .json::<serde_json::Value>()
            .await?;

        let part = &response["candidates"][0]["content"]["parts"][0]["inlineData"];

        let b64_data = part["data"].as_str().ok_or_else(|| -> Box<dyn std::error::Error + Send + Sync> {
            format!("Respuesta TTS inesperada de Gemini: {}", response).into()
        })?;

        let mime_type = part["mimeType"].as_str().unwrap_or("audio/L16;rate=24000");
        let sample_rate = parse_sample_rate(mime_type).unwrap_or(24000);

        let pcm_bytes = STANDARD.decode(b64_data)?;
        Ok(pcm_to_wav(&pcm_bytes, sample_rate, 1, 16))
    }
}

/// Extrae la sample rate del mimeType que devuelve Gemini, p.ej. "audio/L16;rate=24000"
fn parse_sample_rate(mime_type: &str) -> Option<u32> {
    mime_type
        .split(';')
        .find_map(|segment| segment.trim().strip_prefix("rate="))
        .and_then(|rate| rate.parse().ok())
}

/// Envuelve PCM crudo (16-bit signed, little-endian) en un contenedor WAV válido,
/// para que el navegador pueda reproducirlo directamente con <audio> o Web Audio API.
fn pcm_to_wav(pcm: &[u8], sample_rate: u32, channels: u16, bits_per_sample: u16) -> Vec<u8> {
    let byte_rate = sample_rate * channels as u32 * (bits_per_sample as u32 / 8);
    let block_align = channels * (bits_per_sample / 8);
    let data_len = pcm.len() as u32;

    let mut wav = Vec::with_capacity(44 + pcm.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes()); // tamaño del bloque fmt
    wav.extend_from_slice(&1u16.to_le_bytes()); // formato PCM
    wav.extend_from_slice(&channels.to_le_bytes());
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&byte_rate.to_le_bytes());
    wav.extend_from_slice(&block_align.to_le_bytes());
    wav.extend_from_slice(&bits_per_sample.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    wav.extend_from_slice(pcm);
    wav
}
