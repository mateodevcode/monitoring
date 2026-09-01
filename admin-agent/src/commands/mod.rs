pub mod dynamic;
pub mod fixed;
pub mod log_analyzer;
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

pub fn execute_action(action: &str, payload: &Value) -> (bool, String) {
    if let res @ (true, _) = dynamic::execute_action(action, payload) {
        return res;
    }
    if let res @ (true, _) = fixed::execute_action(action, payload) {
        return res;
    }
    on_demand::execute_action(action, payload)
}

pub const DYNAMIC_ACTIONS: &[&str] = &[
    "ram_info",
    "disk_space",
    "uptime_check",
    "get_active_connections",
    "network_threats",
];
