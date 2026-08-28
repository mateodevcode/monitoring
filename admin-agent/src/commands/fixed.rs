use serde_json::{json, Value};
use sysinfo::System;

pub fn execute_action(action: &str, _payload: &Value) -> (bool, String) {
    match action {
        "os_info" => {
            let sys = System::new_all();
            (true, json!({
                "os_name": System::name().unwrap_or_else(|| "Unknown".to_string()),
                "os_version": System::os_version().unwrap_or_else(|| "Unknown".to_string()),
                "kernel_version": System::kernel_version().unwrap_or_else(|| "Unknown".to_string()),
                "cpu_brand": sys.cpus().first().map(|c| c.brand().to_string()).unwrap_or_else(|| "Unknown".to_string()),
                "cpu_cores": sys.cpus().len()
            }).to_string())
        }
        "ip_info" => {
            let output = std::process::Command::new("curl")
                .args(&["-s", "ifconfig.me"])
                .output();

            match output {
                Ok(out) => {
                    let ip = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    if ip.is_empty() {
                        (false, "La respuesta de curl está vacía".to_string())
                    } else {
                        (true, ip) // ¡Éxito! Devuelve la IP limpia
                    }
                }
                Err(e) => (false, format!("Fallo al ejecutar curl: {}", e)),
            }
        }
        _ => (false, "No es una acción fija".to_string()),
    }
}
