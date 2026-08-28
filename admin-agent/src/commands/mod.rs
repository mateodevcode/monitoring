pub mod dynamic;
pub mod fixed;
pub mod on_demand;

use serde_json::{json, Value};
use std::process::Command;

// ==========================================
// UTILIDADES COMPARTIDAS
// ==========================================

pub fn format_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit_idx = 0;
    while size >= 1024.0 && unit_idx < UNITS.len() - 1 {
        size /= 1024.0;
        unit_idx += 1;
    }
    format!("{:.2} {}", size, UNITS[unit_idx])
}

pub fn execute_safe_command(program: &str, args: &[&str]) -> (bool, String) {
    match Command::new(program).args(args).output() {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            if output.status.success() {
                (true, json!({
                    "success": true,
                    "program": program,
                    "output": if stdout.is_empty() { "Comando ejecutado sin salida de texto." } else { &stdout }
                }).to_string())
            } else {
                (false, json!({
                    "success": false,
                    "program": program,
                    "error": if stderr.is_empty() { "El comando falló sin mensaje de error." } else { &stderr }
                }).to_string())
            }
        }
        Err(e) => (
            false,
            json!({
                "success": false,
                "error": format!("No se pudo ejecutar '{}': {}", program, e)
            })
            .to_string(),
        ),
    }
}

// ==========================================
// ROUTER UNIFICADO (Para compatibilidad con WebSocket manual)
// ==========================================

pub fn execute_action(action: &str, payload: &Value) -> (bool, String) {
    // 1. Intentar en dinámicos
    if let res @ (true, _) = dynamic::execute_action(action, payload) {
        return res;
    }
    // 2. Intentar en fijos
    if let res @ (true, _) = fixed::execute_action(action, payload) {
        return res;
    }
    // 3. Intentar en bajo demanda (whitelist)
    on_demand::execute_action(action, payload)
}

// Listas para que main.rs sepa qué iterar
pub const DYNAMIC_ACTIONS: &[&str] = &[
    "ram_info",
    "disk_space",
    "cpu_info",
    "docker_info",
    "uptime_check",
    "network_threats",
];
