use std::fs;
use std::process::Command;

pub fn webm_to_f32_samples(audio_bytes: &[u8]) -> Result<Vec<f32>, String> {
    // Crear directorio temporal
    let tmp_dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let input_path = tmp_dir.path().join("input.webm");
    let output_path = tmp_dir.path().join("output.wav");

    // Escribir los bytes directamente (sin decodificación Base64)
    fs::write(&input_path, audio_bytes)
        .map_err(|e| format!("Failed to write input file: {}", e))?;

    // Convertir webm → WAV 16kHz mono con ffmpeg
    let output = Command::new("ffmpeg")
        .args([
            "-threads",
            "1",
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
