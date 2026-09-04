use std::sync::Arc;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext};

pub struct WhisperEngine {
    ctx: Arc<WhisperContext>,
}

impl WhisperEngine {
    pub fn new(model_path: &str) -> Result<Self, String> {
        tracing::info!("Loading whisper model: {}", model_path);
        let ctx =
            WhisperContext::new(model_path).map_err(|e| format!("Failed to load model: {}", e))?;
        tracing::info!("✅ Whisper model loaded successfully");
        Ok(Self { ctx: Arc::new(ctx) })
    }

    pub fn transcribe(&self, samples: &[f32], lang: &str) -> Result<String, String> {
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });

        params.set_translate(false);
        params.set_language(lang);
        params.set_n_threads(4);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);

        let mut state = self
            .ctx
            .create_state()
            .map_err(|e| format!("Failed to create state: {}", e))?;

        state
            .full(params, samples)
            .map_err(|e| format!("Transcription failed: {}", e))?;

        let n_segments = state
            .full_n_segments()
            .map_err(|e| format!("Failed to get segments: {}", e))?;

        let mut text = String::new();
        for i in 0..n_segments {
            if let Ok(segment) = state.full_get_segment_text(i) {
                text.push_str(&segment);
            }
        }

        Ok(text.trim().to_string())
    }
}
