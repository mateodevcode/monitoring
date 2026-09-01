// src/commands/log_analyzer.rs
use chrono::{DateTime, FixedOffset, NaiveDateTime, Utc};
use regex::Regex;
use serde_json::json;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

// Estructura para una línea de log parseada
#[derive(Debug, Clone)]
pub struct LogEntry {
    pub ip: String,
    pub method: String,
    pub url: String,
    pub status: u16,
    pub user_agent: String,
    pub timestamp: DateTime<FixedOffset>,
}

// Patrón regex para el log combinado estándar
// Ejemplo: 172.18.0.4 - - [01/Sep/2026:00:00:09 +0200] "HEAD / HTTP/1.1" 200 0 "-" "ureq/2.12.1"
lazy_static::lazy_static! {
    static ref LOG_RE: Regex = Regex::new(
        r#"^(\S+) - - \[([^\]]+)\] "([^"]*)" (\d{3}) \d+ "([^"]*)" "([^"]*)""#
    ).unwrap();
}

/// Parsea una línea del access.log en un LogEntry
pub fn parse_log_line(line: &str) -> Option<LogEntry> {
    let caps = LOG_RE.captures(line)?;
    let ip = caps[1].to_string();
    let time_str = &caps[2];
    let request = &caps[3];
    let status = caps[4].parse::<u16>().ok()?;
    let referer = caps[5].to_string();
    let user_agent = caps[6].to_string();

    // Parsear timestamp (ej: "01/Sep/2026:00:00:09 +0200")
    let ts = parse_nginx_timestamp(time_str)?;

    // Separar método y URL del request
    let parts: Vec<&str> = request.split_whitespace().collect();
    let (method, url) = if parts.len() >= 2 {
        (parts[0].to_string(), parts[1].to_string())
    } else {
        ("UNKNOWN".to_string(), "/".to_string())
    };

    Some(LogEntry {
        ip,
        method,
        url,
        status,
        user_agent,
        timestamp: ts,
    })
}

/// Convierte el timestamp de Nginx a DateTime<FixedOffset>
fn parse_nginx_timestamp(ts: &str) -> Option<DateTime<FixedOffset>> {
    // Formato: "01/Sep/2026:00:00:09 +0200"
    let parts: Vec<&str> = ts.splitn(2, ' ').collect();
    if parts.len() != 2 {
        return None;
    }
    let date_part = parts[0]; // "01/Sep/2026:00:00:09"
    let offset_part = parts[1]; // "+0200"

    // Reemplazar el ':' entre fecha y hora por un espacio
    let date_time_str = date_part.replace(':', " ", 1);
    let fmt = "%d/%b/%Y %H:%M:%S %z";
    match chrono::DateTime::parse_from_str(&format!("{} {}", date_time_str, offset_part), fmt) {
        Ok(dt) => Some(dt),
        Err(_) => None,
    }
}

/// Lee las últimas N líneas del archivo de log
pub fn read_last_lines(path: &str, n: usize) -> Vec<String> {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };
    let reader = BufReader::new(file);
    let mut lines = Vec::with_capacity(n);
    for line in reader.lines().take(n).map(|l| l.unwrap_or_default()) {
        lines.push(line);
    }
    // Si hay menos de n líneas, devolvemos todas
    // Si hay más, necesitamos leer desde el final, pero para simplicidad usaremos tail
    // Aunque en este caso, como el archivo es grande, mejor usar un comando tail
    // Pero aquí asumimos que el archivo es pequeño o leemos todo
    // Realmente recomiendo usar `tail -n 200` desde el script, pero lo haremos en Rust.
    // Sin embargo, por eficiencia, leeremos usando BufReader y nos quedamos con las últimas n.
    // Una mejor estrategia: usar seek para leer desde el final, pero es más complejo.
    // Para simplificar, usaré `tail` desde el script bash o usaré un enfoque simple.
    // Para este caso, como es un demo, leeré todas las líneas y tomaré las últimas n.
    let all_lines: Vec<String> = reader.lines().filter_map(|l| l.ok()).collect();
    if all_lines.len() <= n {
        all_lines
    } else {
        all_lines[all_lines.len() - n..].to_vec()
    }
}

/// Analiza el access.log y devuelve un mapa IP -> conteo y metadatos
pub fn analyze_logs(path: &str, window_secs: i64, limit: usize) -> Vec<serde_json::Value> {
    let now = Utc::now();
    let cutoff = now - chrono::Duration::seconds(window_secs);

    // Leer últimas 200 líneas (suficiente para 5 segundos de tráfico normal)
    let lines = read_last_lines(path, 200);
    let mut ip_entries: HashMap<String, Vec<LogEntry>> = HashMap::new();

    for line in lines {
        if let Some(entry) = parse_log_line(&line) {
            // Solo considerar entradas dentro de la ventana de tiempo
            if entry.timestamp.with_timezone(&Utc) >= cutoff {
                ip_entries
                    .entry(entry.ip.clone())
                    .or_insert_with(Vec::new)
                    .push(entry);
            }
        }
    }

    // Construir resultados agregados
    let mut results = Vec::new();
    for (ip, entries) in ip_entries {
        // Filtrar IPs privadas y localhost
        if is_private_ip(&ip) {
            continue;
        }

        let count = entries.len();
        let methods: Vec<String> = entries.iter().map(|e| e.method.clone()).collect();
        let urls: Vec<String> = entries.iter().map(|e| e.url.clone()).collect();
        let statuses: Vec<u16> = entries.iter().map(|e| e.status).collect();
        let user_agents: Vec<String> = entries.iter().map(|e| e.user_agent.clone()).collect();

        // Clasificar nivel
        let level = classify_threat(&ip, count, &methods, &urls, &statuses, &user_agents);

        results.push(json!({
            "ip": ip,
            "connections": count,
            "methods": methods,
            "urls": urls,
            "statuses": statuses,
            "user_agents": user_agents,
            "level": level,
            "timestamp": now.to_rfc3339()
        }));
    }

    // Ordenar por número de conexiones descendente
    results.sort_by(|a, b| b["connections"].as_u64().cmp(&a["connections"].as_u64()));
    results.truncate(limit);
    results
}

/// Clasifica el nivel de amenaza
fn classify_threat(
    ip: &str,
    count: usize,
    methods: &[String],
    urls: &[String],
    statuses: &[u16],
    user_agents: &[String],
) -> String {
    // 1. Si está en whitelist, siempre ADMIN
    if is_whitelisted(ip) {
        return "ADMIN".to_string();
    }

    // 2. Detectar patrones de ataque
    let suspicious_methods = ["POST", "PUT", "DELETE", "CONNECT"];
    let suspicious_urls = [
        "/wp-admin",
        "/wp-login",
        "/phpmyadmin",
        "/cgi-bin",
        "/.env",
        "/config.php",
        "/.git",
        "/admin",
        "/login",
        "/auth",
        "/api/v1/admin",
        "/shell",
        "/cmd",
        "/exec",
    ];
    let suspicious_uas = [
        "curl",
        "python-requests",
        "go-http-client",
        "java",
        "wget",
        "nikto",
        "sqlmap",
    ];

    let has_suspicious_method = methods
        .iter()
        .any(|m| suspicious_methods.contains(&m.as_str()));
    let has_suspicious_url = urls
        .iter()
        .any(|u| suspicious_urls.iter().any(|s| u.contains(s)));
    let has_suspicious_ua = user_agents
        .iter()
        .any(|u| suspicious_uas.iter().any(|s| u.contains(s)));
    let has_auth_failure = statuses.iter().any(|&s| s == 401 || s == 403);
    let has_server_error = statuses.iter().any(|&s| s >= 500);

    // 3. Clasificación por umbrales
    if count > 30 || (has_suspicious_method && count > 10) || (has_suspicious_url && count > 5) {
        "CRITICAL".to_string()
    } else if count > 15 || has_suspicious_ua || has_auth_failure || has_server_error {
        "WARNING".to_string()
    } else {
        "SAFE".to_string()
    }
}

/// Verifica si una IP está en la whitelist (cargada desde DB)
fn is_whitelisted(ip: &str) -> bool {
    // Esta función se conectará a la DB para verificar admin_whitelist
    // Para evitar dependencias circulares, usaremos una función global o lazy_static
    // Por simplicidad, en esta implementación se pasará como parámetro.
    // Mejor lo hacemos dinámicamente: cuando se llama a analyze_logs, se pasa un closure o la lista de whitelist.
    // Para este ejemplo, lo dejamos como placeholder.
    false // Será reemplazado por una consulta real desde el llamador
}

/// Verifica si una IP es privada o local
fn is_private_ip(ip: &str) -> bool {
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
}

// Para whitelist, necesitamos acceso a la DB, lo haremos desde el llamador.
// Por lo tanto, expondremos una función que recibe la lista de IPs whitelist.
pub fn analyze_logs_with_whitelist(
    path: &str,
    window_secs: i64,
    limit: usize,
    whitelist: &[String],
) -> Vec<serde_json::Value> {
    let now = Utc::now();
    let cutoff = now - chrono::Duration::seconds(window_secs);

    let lines = read_last_lines(path, 200);
    let mut ip_entries: HashMap<String, Vec<LogEntry>> = HashMap::new();

    for line in lines {
        if let Some(entry) = parse_log_line(&line) {
            if entry.timestamp.with_timezone(&Utc) >= cutoff {
                ip_entries
                    .entry(entry.ip.clone())
                    .or_insert_with(Vec::new)
                    .push(entry);
            }
        }
    }

    let mut results = Vec::new();
    for (ip, entries) in ip_entries {
        if is_private_ip(&ip) || whitelist.contains(&ip) {
            // Si está en whitelist, lo marcamos como ADMIN pero igual lo mostramos?
            // Mejor lo mostramos pero con level ADMIN y no lo guardamos en DB de amenazas.
            let count = entries.len();
            let methods: Vec<String> = entries.iter().map(|e| e.method.clone()).collect();
            let urls: Vec<String> = entries.iter().map(|e| e.url.clone()).collect();
            let statuses: Vec<u16> = entries.iter().map(|e| e.status).collect();
            let user_agents: Vec<String> = entries.iter().map(|e| e.user_agent.clone()).collect();

            results.push(json!({
                "ip": ip,
                "connections": count,
                "methods": methods,
                "urls": urls,
                "statuses": statuses,
                "user_agents": user_agents,
                "level": "ADMIN",
                "timestamp": now.to_rfc3339()
            }));
            continue;
        }

        let count = entries.len();
        let methods: Vec<String> = entries.iter().map(|e| e.method.clone()).collect();
        let urls: Vec<String> = entries.iter().map(|e| e.url.clone()).collect();
        let statuses: Vec<u16> = entries.iter().map(|e| e.status).collect();
        let user_agents: Vec<String> = entries.iter().map(|e| e.user_agent.clone()).collect();

        let level = classify_threat(&ip, count, &methods, &urls, &statuses, &user_agents);

        results.push(json!({
            "ip": ip,
            "connections": count,
            "methods": methods,
            "urls": urls,
            "statuses": statuses,
            "user_agents": user_agents,
            "level": level,
            "timestamp": now.to_rfc3339()
        }));
    }

    results.sort_by(|a, b| b["connections"].as_u64().cmp(&a["connections"].as_u64()));
    results.truncate(limit);
    results
}
