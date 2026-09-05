// El prompt se incrusta en el binario en tiempo de COMPILACIÓN.
// Esto elimina el problema de que el Dockerfile no copie el .txt al runtime:
// ya no hace falta copiar nada, el texto vive dentro del ejecutable.
const SYSTEM_PROMPT: &str = include_str!("system_prompt.txt");

pub fn load_system_prompt() -> String {
    SYSTEM_PROMPT.to_string()
}
