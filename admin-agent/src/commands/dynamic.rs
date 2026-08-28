use crate::commands::format_bytes;
use serde_json::{json, Value};
use sysinfo::{Disks, System};

pub fn execute_action(action: &str, _payload: &Value) -> (bool, String) {
    match action {
        "ram_info" => {
            let sys = System::new_all();
            let total = sys.total_memory();
            let available = sys.available_memory();
            let used = total.saturating_sub(available);
            let percent = if total > 0 {
                (used as f64 / total as f64) * 100.0
            } else {
                0.0
            };

            (
                true,
                json!({
                    "total_human": format_bytes(total),
                    "used_human": format_bytes(used),
                    "available_human": format_bytes(available),
                    "percent": (percent.round() as u64).min(100)
                })
                .to_string(),
            )
        }
        "disk_space" => {
            let disks = Disks::new_with_refreshed_list();

            // Buscar el disco montado en "/" (el disco raíz real)
            let root_disk = disks
                .iter()
                .find(|d| d.mount_point() == std::path::Path::new("/"));

            let (total, available) = if let Some(disk) = root_disk {
                (disk.total_space(), disk.available_space())
            } else {
                // Fallback: si no encuentra "/", suma solo discos físicos (no tmpfs ni overlays)
                let mut t: u64 = 0;
                let mut a: u64 = 0;
                for disk in disks.iter() {
                    // Filtrar: solo discos reales (no tmpfs, devtmpfs, etc.)
                    let mount_str = disk.mount_point().to_string_lossy().to_string();
                    if !mount_str.starts_with("/run")
                        && !mount_str.starts_with("/dev/shm")
                        && !mount_str.starts_with("/sys")
                        && !mount_str.starts_with("/proc")
                        && !mount_str.contains("overlay")
                    {
                        t += disk.total_space();
                        a += disk.available_space();
                    }
                }
                (t, a)
            };

            let used = total.saturating_sub(available);
            let percent = if total > 0 {
                (used as f64 / total as f64) * 100.0
            } else {
                0.0
            };

            let response = json!({
                "total_human": format_bytes(total),
                "used_human": format_bytes(used),
                "available_human": format_bytes(available),
                "percent": (percent.round() as u64).min(100)
            });
            (true, response.to_string())
        }
        "cpu_info" => {
            let sys = System::new_all();
            let cpu_usage = sys.global_cpu_usage() as f32;
            (true, json!({
                "percent": (cpu_usage.round() as u64).min(100),
                "cores": sys.cpus().len(),
                "brand": sys.cpus().first().map(|c| c.brand().to_string()).unwrap_or_else(|| "Unknown".to_string())
            }).to_string())
        }
        "docker_info" => {
            // Usamos el formato JSON nativo de Docker para un parseo 100% seguro
            let output = std::process::Command::new("docker")
                .args(["ps", "-a", "--format", "{{json .}}"])
                .output();

            match output {
                Ok(out) => {
                    let stdout = String::from_utf8_lossy(&out.stdout);
                    let mut containers: Vec<serde_json::Value> = Vec::new();

                    for line in stdout.lines() {
                        if line.trim().is_empty() {
                            continue;
                        }
                        // Parseamos cada línea como un objeto JSON independiente
                        if let Ok(json_val) = serde_json::from_str::<serde_json::Value>(line) {
                            let clean_obj = serde_json::json!({
                                "name": json_val["Names"],
                                "status": json_val["Status"],
                                "size": json_val["Size"]
                            });
                            containers.push(clean_obj);
                        }
                    }

                    (
                        true,
                        serde_json::json!({"containers": containers}).to_string(),
                    )
                }
                Err(e) => (false, format!("Error ejecutando docker: {}", e)),
            }
        }
        "nginx_full" => {
            let output = std::process::Command::new("docker")
                .args([
                    "run",
                    "--rm",
                    "--net=host",
                    "-v",
                    "/etc/nginx:/etc/nginx:ro",
                    "-v",
                    "/etc/letsencrypt:/etc/letsencrypt:ro",
                    "-v",
                    "/var/log/nginx:/var/log/nginx:ro",
                    "alpine",
                    "sh",
                    "-c",
                    r#"
apk add --no-cache openssl curl iproute2 > /dev/null 2>&1;

echo '=== SITES ===';
grep -rh '^\s*server_name\s' /etc/nginx/ 2>/dev/null | awk '{print $2}' | tr -d ';' | sort -u;

echo '=== CERTS ===';
for f in /etc/letsencrypt/live/*/fullchain.pem; do
    if [ -f "$f" ]; then
        domain=$(basename $(dirname "$f"));
        expiry=$(openssl x509 -in "$f" -noout -enddate 2>/dev/null | cut -d= -f2);
        echo "$domain|$expiry";
    fi;
done;

echo '=== PORTS ===';
ss -tlnp 2>/dev/null | awk '/:(80|443)/ {print $4}' | sort -u;

echo '=== ERRORS ===';
tail -20 /var/log/nginx/error.log 2>/dev/null | grep -iE '\[error\]|\[warn\]' | tail -10;
                    "#,
                ])
                .output();

            match output {
                Ok(out) => {
                    let stdout = String::from_utf8_lossy(&out.stdout);
                    let mut sites = Vec::new();
                    let mut certs = Vec::new();
                    let mut ports = Vec::new();
                    let mut errors = Vec::new();
                    let mut section = "";

                    for line in stdout.lines() {
                        if line.starts_with("=== SITES ===") {
                            section = "sites";
                            continue;
                        }
                        if line.starts_with("=== CERTS ===") {
                            section = "certs";
                            continue;
                        }
                        if line.starts_with("=== PORTS ===") {
                            section = "ports";
                            continue;
                        }
                        if line.starts_with("=== ERRORS ===") {
                            section = "errors";
                            continue;
                        }

                        match section {
                            "sites" => {
                                let site = line.trim().to_string();
                                // Filtramos el catch-all "_" de nginx y líneas vacías
                                if !site.is_empty() && site != "_" {
                                    sites.push(site);
                                }
                            }
                            "certs" => {
                                let parts: Vec<&str> = line.split('|').collect();
                                if parts.len() == 2 {
                                    certs.push(serde_json::json!({
                                        "domain": parts[0],
                                        "expiry": parts[1]
                                    }));
                                }
                            }
                            "ports" => {
                                if !line.trim().is_empty() {
                                    ports.push(line.trim().to_string());
                                }
                            }
                            "errors" => {
                                if !line.trim().is_empty() {
                                    errors.push(line.trim().to_string());
                                }
                            }
                            _ => {}
                        }
                    }

                    let response = serde_json::json!({
                        "sites": sites,
                        "certs": certs,
                        "ports": ports,
                        "errors": errors,
                        "status": if ports.iter().any(|p| p.contains(":443")) || ports.iter().any(|p| p.contains(":80")) { "healthy" } else { "warning" }
                    });

                    (true, response.to_string())
                }
                Err(e) => (false, format!("Error ejecutando docker: {}", e)),
            }
        }
        "uptime_check" => {
            // 1. Leer el archivo de configuración (si no existe, usa lista vacía)
            let sites_file = std::fs::read_to_string("monitored_sites.json")
                .unwrap_or_else(|_| "[]".to_string());

            tracing::info!("🌐 Sitios a verificar: {}", sites_file);

            let urls: Vec<String> =
                serde_json::from_str(&sites_file).unwrap_or_else(|_| Vec::new());
            let mut results = Vec::new();

            // 2. Verificar cada URL de forma síncrona (ureq es bloqueante pero muy rápido para pocas URLs)
            for url in urls {
                let start = std::time::Instant::now();
                let mut status_code = 0;
                let mut is_up = false;
                let mut error_msg = String::new();

                // Petición HEAD síncrona con timeout de 5 segundos
                match ureq::head(&url)
                    .timeout(std::time::Duration::from_secs(5))
                    .call()
                {
                    Ok(res) => {
                        status_code = res.status();
                        is_up = (200..300).contains(&status_code);
                    }
                    Err(e) => {
                        // ureq devuelve Error::Status para 4xx y 5xx, lo capturamos para saber que el servidor respondió
                        if let ureq::Error::Status(code, _response) = e {
                            status_code = code;
                            is_up = false; // No es 2xx, pero el servidor está "arriba" respondiendo
                            error_msg = format!("HTTP {}", code);
                        } else {
                            error_msg = e.to_string(); // Error de red, DNS, timeout, etc.
                        }
                    }
                }

                let response_time_ms = start.elapsed().as_millis();

                // Limpiar la URL para mostrar solo el dominio
                let domain = url
                    .replace("https://", "")
                    .replace("http://", "")
                    .trim_end_matches('/')
                    .to_string();

                let now = chrono::Utc::now().format("%H:%M:%S").to_string();

                results.push(serde_json::json!({
                    "url": url,
                    "domain": domain,
                    "status_code": status_code,
                    "is_up": is_up || (status_code >= 400 && status_code < 600),
                    "response_time_ms": response_time_ms,
                    "error": if error_msg.is_empty() { "OK".to_string() } else { error_msg },
                    "last_checked": now
                }));
            }

            (true, serde_json::json!({"sites": results}).to_string())
        }
        _ => (false, "No es una acción dinámica".to_string()),
    }
}
