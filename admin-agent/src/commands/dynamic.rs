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

            let root_disk = disks
                .iter()
                .find(|d| d.mount_point() == std::path::Path::new("/"));

            let (total, available) = if let Some(disk) = root_disk {
                (disk.total_space(), disk.available_space())
            } else {
                let mut t: u64 = 0;
                let mut a: u64 = 0;
                for disk in disks.iter() {
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
        "docker_info" => {
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
            // Script nativo sin Docker
            let script = r#"
echo '=== SITES ===';
grep -rh '^\s*server_name\s' /etc/nginx/ 2>/dev/null | awk '{print $2}' | tr -d ';' | sort -u;

echo '=== CERTS ===';
sudo certbot certificates --text --noninteractive 2>/dev/null |
    awk '/Certificate Name:/ {domain=$3} /Expiry Date:/ {print domain "|" $4 " " $5 " " $6 " " $7}';

echo '=== PORTS ===';
ss -tlnp 2>/dev/null | awk '/:(80|443)/ {print $4}' | sort -u;

echo '=== ERRORS ===';
tail -20 /var/log/nginx/error.log 2>/dev/null | grep -iE '\[error\]|\[warn\]' | tail -10;

echo '=== NGINX ACTIVE ===';
systemctl is-active nginx 2>/dev/null || echo "inactive";
    "#;

            let output = std::process::Command::new("sh")
                .arg("-c")
                .arg(script)
                .output();

            match output {
                Ok(out) => {
                    let stdout = String::from_utf8_lossy(&out.stdout);
                    let mut sites = Vec::new();
                    let mut certs = Vec::new();
                    let mut ports = Vec::new();
                    let mut errors = Vec::new();
                    let mut nginx_active = "unknown".to_string();
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
                        if line.starts_with("=== NGINX ACTIVE ===") {
                            section = "nginx_active";
                            continue;
                        }

                        match section {
                            "sites" => {
                                let site = line.trim().to_string();
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
                            "nginx_active" => {
                                nginx_active = line.trim().to_string();
                            }
                            _ => {}
                        }
                    }

                    let status = if ports.iter().any(|p| p.contains(":443"))
                        || ports.iter().any(|p| p.contains(":80"))
                    {
                        "healthy"
                    } else {
                        "warning"
                    };

                    let response = serde_json::json!({
                        "sites": sites,
                        "certs": certs,
                        "ports": ports,
                        "errors": errors,
                        "status": status,
                        "nginx_active": nginx_active
                    });

                    (true, response.to_string())
                }
                Err(e) => (false, format!("Error ejecutando script nativo: {}", e)),
            }
        }
        "uptime_check" => {
            let sites_file = std::fs::read_to_string("monitored_sites.json")
                .unwrap_or_else(|_| "[]".to_string());

            tracing::info!("🌐 Sitios a verificar: {}", sites_file);

            let urls: Vec<String> =
                serde_json::from_str(&sites_file).unwrap_or_else(|_| Vec::new());
            let mut results = Vec::new();

            for url in urls {
                let start = std::time::Instant::now();
                let mut status_code = 0;
                let mut is_up = false;
                let mut error_msg = String::new();

                match ureq::head(&url)
                    .timeout(std::time::Duration::from_secs(5))
                    .call()
                {
                    Ok(res) => {
                        status_code = res.status();
                        is_up = (200..300).contains(&status_code);
                    }
                    Err(e) => {
                        if let ureq::Error::Status(code, _response) = e {
                            status_code = code;
                            is_up = false;
                            error_msg = format!("HTTP {}", code);
                        } else {
                            error_msg = e.to_string();
                        }
                    }
                }

                let response_time_ms = start.elapsed().as_millis();

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
        "get_active_connections" => get_active_connections_impl(),
        _ => (false, "No es una acción dinámica".to_string()),
    }
}

// ============================================
// FUNCIONES HELPER PARA GET_ACTIVE_CONNECTIONS
// ============================================

fn cstr_from_bytes(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).trim().to_string()
}

struct UtmpRecord {
    ut_type: i16,
    user: String,
    line: String,
    host: String,
    tv_sec: i64,
}

fn parse_utmp(path: &str) -> Vec<UtmpRecord> {
    const UTMP_RECORD_SIZE: usize = 384;
    let mut records = Vec::new();
    let data = match std::fs::read(path) {
        Ok(d) => d,
        Err(e) => {
            tracing::error!("❌ No se pudo leer {}: {:?}", path, e);
            return records;
        }
    };

    if data.len() % UTMP_RECORD_SIZE != 0 {
        tracing::error!(
            "⚠️ {} tiene tamaño inesperado ({} bytes, no es múltiplo de {})",
            path,
            data.len(),
            UTMP_RECORD_SIZE
        );
    }

    for chunk in data.chunks_exact(UTMP_RECORD_SIZE) {
        let ut_type = i16::from_ne_bytes([chunk[0], chunk[1]]);

        let line = cstr_from_bytes(&chunk[8..40]);
        let user = cstr_from_bytes(&chunk[44..76]);
        let host = cstr_from_bytes(&chunk[76..332]);

        let tv_sec = i32::from_ne_bytes([chunk[340], chunk[341], chunk[342], chunk[343]]) as i64;

        records.push(UtmpRecord {
            ut_type,
            user,
            line,
            host,
            tv_sec,
        });
    }

    records
}

fn parse_proc_net_tcp(content: &str, is_v6: bool) -> Vec<(u16, String, u8)> {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    let mut entries = Vec::new();

    for line in content.lines().skip(1) {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 4 {
            continue;
        }

        let local_addr = parts[1];
        let remote_addr = parts[2];
        let state = parts[3];

        let (_local_ip, local_port) = {
            let addr_parts: Vec<&str> = local_addr.split(':').collect();
            if addr_parts.len() != 2 {
                continue;
            }
            let port = u16::from_str_radix(addr_parts[1], 16).unwrap_or(0);

            if is_v6 {
                let ip_hex = addr_parts[0];
                if ip_hex.len() != 32 {
                    continue;
                }
                let parts: Vec<&str> = (0..8).map(|i| &ip_hex[i * 4..(i + 1) * 4]).collect();
                let segments: Vec<u16> = parts
                    .iter()
                    .filter_map(|s| u16::from_str_radix(s, 16).ok())
                    .collect();
                if segments.len() != 8 {
                    continue;
                }
                let addr = IpAddr::V6(Ipv6Addr::new(
                    segments[0],
                    segments[1],
                    segments[2],
                    segments[3],
                    segments[4],
                    segments[5],
                    segments[6],
                    segments[7],
                ));
                (addr.to_string(), port)
            } else {
                let bytes: Vec<u8> = (0..4)
                    .map(|i| {
                        u8::from_str_radix(&addr_parts[0][i * 2..(i + 1) * 2], 16).unwrap_or(0)
                    })
                    .collect();
                if bytes.len() != 4 {
                    continue;
                }
                let addr = IpAddr::V4(Ipv4Addr::new(bytes[3], bytes[2], bytes[1], bytes[0]));
                (addr.to_string(), port)
            }
        };

        let (remote_ip, _remote_port) = {
            let addr_parts: Vec<&str> = remote_addr.split(':').collect();
            if addr_parts.len() != 2 {
                continue;
            }

            if is_v6 {
                let ip_hex = addr_parts[0];
                if ip_hex.len() != 32 {
                    continue;
                }
                let parts: Vec<&str> = (0..8).map(|i| &ip_hex[i * 4..(i + 1) * 4]).collect();
                let segments: Vec<u16> = parts
                    .iter()
                    .filter_map(|s| u16::from_str_radix(s, 16).ok())
                    .collect();
                if segments.len() != 8 {
                    continue;
                }
                let addr = IpAddr::V6(Ipv6Addr::new(
                    segments[0],
                    segments[1],
                    segments[2],
                    segments[3],
                    segments[4],
                    segments[5],
                    segments[6],
                    segments[7],
                ));
                (addr.to_string(), 0)
            } else {
                let bytes: Vec<u8> = (0..4)
                    .map(|i| {
                        u8::from_str_radix(&addr_parts[0][i * 2..(i + 1) * 2], 16).unwrap_or(0)
                    })
                    .collect();
                if bytes.len() != 4 {
                    continue;
                }
                let addr = IpAddr::V4(Ipv4Addr::new(bytes[3], bytes[2], bytes[1], bytes[0]));
                (addr.to_string(), 0)
            }
        };

        let state_byte = u8::from_str_radix(state, 16).unwrap_or(0);

        entries.push((local_port, remote_ip, state_byte));
    }

    entries
}

fn get_active_connections_impl() -> (bool, String) {
    use nix::sched::{setns, CloneFlags};
    use std::collections::HashMap;
    use std::time::Duration;

    let is_private_ip = |ip: &str| -> bool {
        ip.starts_with("127.")
            || ip.starts_with("10.")
            || ip.starts_with("192.168.")
            || ip.starts_with("172.16.")
            || ip.starts_with("172.17.")
            || ip.starts_with("172.18.")
            || ip.starts_with("172.19.")
            || ip.starts_with("172.20.")
            || ip.starts_with("172.21.")
            || ip.starts_with("172.22.")
            || ip.starts_with("172.23.")
            || ip.starts_with("172.24.")
            || ip.starts_with("172.25.")
            || ip.starts_with("172.26.")
            || ip.starts_with("172.27.")
            || ip.starts_with("172.28.")
            || ip.starts_with("172.29.")
            || ip.starts_with("172.30.")
            || ip.starts_with("172.31.")
            || ip == "::1"
            || ip.starts_with("fd")
            || ip.starts_with("fe80")
    };

    let expected_users = ["root", "admin", "deploy", "ubuntu"];
    let known_ips = ["79.117.90.148", "1.2.3.4"];
    let _suspicious_patterns = [
        "bash", "nc ", "nohup", "python", "perl", "sh -i", "socat", "telnet",
    ];

    let mut ssh_sessions = Vec::new();
    let mut external_ips_to_geolocate = Vec::new();

    // PASO 1: SSH Sessions vía /var/run/utmp
    const USER_PROCESS: i16 = 7;

    let utmp_records = parse_utmp("/var/run/utmp");
    tracing::info!(
        "📊 /var/run/utmp -> {} registros totales",
        utmp_records.len()
    );

    for rec in &utmp_records {
        if rec.ut_type != USER_PROCESS || rec.user.is_empty() {
            continue;
        }

        let from = if rec.host.is_empty() {
            "local".to_string()
        } else {
            rec.host.clone()
        };

        let user_status = if expected_users.contains(&rec.user.as_str()) {
            "EXPECTED"
        } else {
            "SUSPICIOUS"
        };

        let ip_status = if from == "local" {
            "INTERNAL"
        } else if is_private_ip(&from) {
            "INTERNAL"
        } else if known_ips.contains(&from.as_str()) {
            "KNOWN_EXTERNAL"
        } else {
            "EXTERNAL"
        };

        if ip_status != "INTERNAL" {
            external_ips_to_geolocate.push(from.clone());
        }

        let login_time = chrono::DateTime::from_timestamp(rec.tv_sec, 0)
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_else(|| "unknown".to_string());

        ssh_sessions.push(serde_json::json!({
            "user": rec.user,
            "from": from,
            "login": login_time,
            "idle": "n/a",
            "what": rec.line,
            "user_status": user_status,
            "ip_status": ip_status,
            "suspicious_command": false,
            "country": "XX"
        }));
    }

    tracing::info!("📊 ssh_sessions final: {} sesiones", ssh_sessions.len());

    // PASO 2: Web connections — con setns nativo
    let web_connections_map: HashMap<String, (u32, u32)> =
        std::thread::spawn(|| -> HashMap<String, (u32, u32)> {
            let mut map = HashMap::new();

            match std::fs::File::open("/proc/1/ns/net") {
                Ok(f) => match setns(f, CloneFlags::CLONE_NEWNET) {
                    Ok(_) => tracing::info!("✅ setns OK: entramos al netns del host"),
                    Err(e) => tracing::error!("❌ setns FALLÓ: {:?}", e),
                },
                Err(e) => tracing::error!("❌ No se pudo abrir /proc/1/ns/net: {:?}", e),
            }

            let mut entries = Vec::new();
            for (path, is_v6) in [
                ("/proc/thread-self/net/tcp", false),
                ("/proc/thread-self/net/tcp6", true),
            ] {
                match std::fs::read_to_string(path) {
                    Ok(content) => {
                        let parsed = parse_proc_net_tcp(&content, is_v6);
                        tracing::info!("📊 {} -> {} entradas", path, parsed.len());
                        entries.extend(parsed);
                    }
                    Err(e) => tracing::error!("❌ No se pudo leer {}: {:?}", path, e),
                }
            }

            tracing::info!("📊 Total entradas (thread-self): {}", entries.len());

            for (local_port, peer_ip, state) in entries {
                if state != 0x01 {
                    continue;
                }
                if local_port != 80 && local_port != 443 {
                    continue;
                }
                let key = format!("{}|{}", peer_ip, local_port);
                map.entry(key)
                    .and_modify(|(_, c): &mut (u32, u32)| *c += 1)
                    .or_insert((local_port as u32, 1));
            }

            tracing::info!("📊 web_connections_map final: {} entradas", map.len());
            map
        })
        .join()
        .unwrap_or_default();

    for key in web_connections_map.keys() {
        if let Some(peer_ip) = key.split('|').next() {
            if !is_private_ip(peer_ip) {
                external_ips_to_geolocate.push(peer_ip.to_string());
            }
        }
    }

    // PASO 3: Geolocalización
    external_ips_to_geolocate.sort();
    external_ips_to_geolocate.dedup();

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(1))
        .build()
        .unwrap_or_default();

    let mut geo_results = HashMap::new();
    for ip in &external_ips_to_geolocate {
        let url = format!("http://ip-api.com/csv/{}?fields=countryCode", ip);
        if let Ok(resp) = client.get(&url).send() {
            if let Ok(text) = resp.text() {
                let country = text.trim().to_uppercase();
                geo_results.insert(
                    ip.clone(),
                    if country.len() == 2 {
                        country
                    } else {
                        "XX".to_string()
                    },
                );
            }
        } else {
            geo_results.insert(ip.clone(), "XX".to_string());
        }
    }

    // PASO 4: Ensamblar respuesta
    for session in &mut ssh_sessions {
        if let Some(ip) = session["from"].as_str() {
            if let Some(country) = geo_results.get(ip) {
                session["country"] = serde_json::Value::String(country.clone());
            }
        }
    }

    let mut final_web_connections = Vec::new();
    for (key, (port, count)) in web_connections_map {
        let ip = key.split('|').next().unwrap_or("");
        let country = geo_results
            .get(ip)
            .cloned()
            .unwrap_or_else(|| "XX".to_string());
        let status = if known_ips.contains(&ip) {
            "ADMIN"
        } else {
            "EXTERNAL"
        };

        final_web_connections.push(serde_json::json!({
            "peer_ip": ip,
            "port": port,
            "count": count,
            "country": country,
            "status": status
        }));
    }

    final_web_connections.sort_by(|a, b| b["count"].as_u64().cmp(&a["count"].as_u64()));

    let response = serde_json::json!({
        "action": "get_active_connections",
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "ssh_sessions": ssh_sessions,
        "web_connections": final_web_connections
    });

    (true, response.to_string())
}
