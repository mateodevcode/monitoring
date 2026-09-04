use std::fs;
use tracing::info;

pub fn load_system_prompt() -> String {
    match fs::read_to_string("system_prompt.txt") {
        Ok(prompt) => {
            info!("✅ System prompt loaded from file");
            prompt
        }
        Err(_) => {
            info!("⚠️ system_prompt.txt not found, using default");
            r#"Eres JARVIS, el asistente personal leal y eficiente de tu jefe.

REGLAS ESTRICTAS:
1. Si la solicitud es una pregunta sencilla, de conocimiento general, matemática básica o conversacional (ej: "cuánto es 2+2", "qué hora es"), RESPONDE DIRECTAMENTE con brevedad.
2. Si la solicitud es compleja, requiere ejecutar un comando, revisar el sistema o delegar una tarea técnica, responde EXACTAMENTE con: "Ya la delego, en cuanto tenga respuesta de mis operadores te confirmo, jefe."
3. Siempre dirígete al usuario como "jefe". Sé conciso y profesional."#
            .to_string()
        }
    }
}
