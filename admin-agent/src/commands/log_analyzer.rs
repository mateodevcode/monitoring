// src/commands/log_analyzer.rs
use chrono::{DateTime, FixedOffset, Utc};
use regex::Regex;
use serde_json::json;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};

#[derive(Debug, Clone)]
pub struct LogEntry {
    pub ip: String,
    pub method: String,
    pub url: String,
    pub status: u16,
    pub user_agent: String,
    pub timestamp: DateTime<FixedOffset>,
}

lazy_static::lazy_static! {
    static ref LOG_RE: Regex = Regex::new(
        r#"^(\S+) - - \[([^\]]+)\] "([^"]*)" (\d{3}) \d+ "([^"]*)" "([^"]*)""#
    ).unwrap();
}

pub fn parse_log_line(line: &str) -> Option<LogEntry> {
    let caps = LOG_RE.captures(line)?;
    let ip = caps[1].to_string();
    let time_str = &caps[2];
    let request = &caps[3];
    let status = caps[4].parse::<u16>().ok()?;
    let _referer = caps[5].to_string();
    let user_agent = caps[6].to_string();

    let ts = parse_nginx_timestamp(time_str)?;

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

fn parse_nginx_timestamp(ts: &str) -> Option<DateTime<FixedOffset>> {
    // Formato: "01/Sep/2026:00:00:09 +0200"
    let fmt = "%d/%b/%Y:%H:%M:%S %z";
    match chrono::DateTime::parse_from_str(ts, fmt) {
        Ok(dt) => Some(dt),
        Err(_) => None,
    }
}

pub fn read_last_lines(path: &str, n: usize) -> Vec<String> {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };
    let reader = BufReader::new(file);
    let all_lines: Vec<String> = reader.lines().filter_map(|l| l.ok()).collect();
    if all_lines.len() <= n {
        all_lines
    } else {
        all_lines[all_lines.len() - n..].to_vec()
    }
}

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
        if is_private_ip(&ip) {
            continue;
        }

        let count = entries.len();
        let methods: Vec<String> = entries.iter().map(|e| e.method.clone()).collect();
        let urls: Vec<String> = entries.iter().map(|e| e.url.clone()).collect();
        let statuses: Vec<u16> = entries.iter().map(|e| e.status).collect();
        let user_agents: Vec<String> = entries.iter().map(|e| e.user_agent.clone()).collect();

        let is_whitelisted = whitelist.contains(&ip);
        let level = if is_whitelisted {
            "ADMIN".to_string()
        } else {
            classify_threat(&ip, count, &methods, &urls, &statuses, &user_agents)
        };

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

fn classify_threat(
    _ip: &str,
    count: usize,
    methods: &[String],
    urls: &[String],
    statuses: &[u16],
    user_agents: &[String],
) -> String {
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
        "/xmlrpc.php",
        "/wp-content/uploads",
    ];
    let suspicious_uas = [
        "curl",
        "python-requests",
        "go-http-client",
        "java",
        "wget",
        "nikto",
        "sqlmap",
        "gobuster",
        "ffuf",
        "dirb",
        "masscan",
        "nmap",
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
    let has_scan_pattern = urls.iter().any(|u| u.contains("?") && u.contains("=")) && count > 10;

    if count > 30 || (has_suspicious_method && count > 10) || (has_suspicious_url && count > 5) {
        "CRITICAL".to_string()
    } else if count > 15
        || has_suspicious_ua
        || has_auth_failure
        || has_server_error
        || has_scan_pattern
    {
        "WARNING".to_string()
    } else {
        "SAFE".to_string()
    }
}

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
