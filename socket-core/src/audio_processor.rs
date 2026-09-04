use base64::{engine::general_purpose, Engine as _};
use std::io::Write;
use std::process::Command;

pub fn webm_to_f32_samples(audio_base64: &str) -> Result<Vec<f32>, String> {
    // Decodificar base64
    let audio_data = general_purpose::STANDARD
        .decode(audio_base64)
        .map_err(|e| format!("Base64 decode failed: {}", e))?;

    // Crear directorio temporal
    let tmp_dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let input_path = tmp_dir.path().join("input.webm");
    let output_path = tmp_dir.path().join("output.wav");

    // Escribir audio a archivo temporal
    let mut file = std::fs::File::create(&input_path).map_err(|e| e.to_string())?;
    file.write_all(&audio_data).map_err(|e| e.to_string())?;

    // Convertir webm → WAV 16kHz mono con ffmpeg
    let output = Command::new("ffmpeg")
        .args([
            "-i",
            input_path.to_str().unwrap(),
            "-ar",
            "16000",
            "-ac",
            "1",
            "-f",
            "wav",
            "-loglevel",
            "error",
            output_path.to_str().unwrap(),
            "-y",
        ])
        .output()
        .map_err(|e| format!("ffmpeg execution failed: {}. Is ffmpeg installed?", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("ffmpeg failed: {}", stderr));
    }

    // Leer WAV y convertir a f32 samples
    let reader =
        hound::WavReader::open(&output_path).map_err(|e| format!("WAV read failed: {}", e))?;

    let samples: Vec<f32> = reader
        .into_samples::<i16>()
        .filter_map(|s| s.ok())
        .map(|s| s as f32 / 32768.0)
        .collect();

    tracing::debug!(
        "Audio converted: {} samples ({:.1}s)",
        samples.len(),
        samples.len() as f32 / 16000.0
    );

    Ok(samples)
}
