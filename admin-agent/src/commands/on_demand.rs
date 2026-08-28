use serde_json::{json, Value};

const ALLOWED_COMMANDS: &[(&str, &str, &[&str])] = &[
    ("os_info", "uname", &["-a"]),
    ("docker_down", "docker", &["compose", "down"]),
    ("docker_up", "docker", &["compose", "up", "-d"]),
    ("docker_restart", "docker", &["compose", "restart"]),
    (
        "docker_ps",
        "docker",
        &[
            "ps",
            "-a",
            "--format",
            "table {{.Names}}\t{{.Status}}\t{{.Image}}\t{{.Ports}}",
        ],
    ),
    (
        "nginx_status",
        "curl",
        &[
            "-s",
            "-o",
            "/dev/null",
            "-w",
            "%{http_code}",
            "http://127.0.0.1",
        ],
    ),
    ("ports_info", "ss", &["-tunlp"]),
];

pub fn execute_action(action: &str, payload: &Value) -> (bool, String) {
    if action == "ip_info" {
        let output = std::process::Command::new("curl")
            .args(&["-s", "ifconfig.me"])
            .output();

        match output {
            Ok(out) => {
                let ip = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if ip.is_empty() {
                    (false, "La respuesta de curl está vacía".to_string())
                } else {
                    (true, ip)
                }
            }
            Err(e) => (false, format!("Fallo al ejecutar curl: {}", e)),
        }
    } else if action == "ports_info" {
        let output = std::process::Command::new("docker")
            .args([
                "run",
                "--rm",
                "--net=host",
                "--pid=host",
                "alpine",
                "sh",
                "-c",
                "apk add --no-cache iproute2 procps > /dev/null 2>&1; ss -tunlp",
            ])
            .output();

        match output {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let mut ports_data = Vec::new();

                for line in stdout.lines().skip(1) {
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() >= 5 {
                        let proto = parts[0].to_uppercase();
                        let local_addr = parts[4].to_string();
                        let process = if let Some(users_idx) = line.find("users:") {
                            let users_str = &line[users_idx..];
                            if let Some(start) = users_str.find('\"') {
                                if let Some(end) = users_str[start + 1..].find('\"') {
                                    users_str[start + 1..start + 1 + end].to_string()
                                } else {
                                    "unknown".to_string()
                                }
                            } else {
                                "unknown".to_string()
                            }
                        } else {
                            "unknown".to_string()
                        };

                        ports_data.push(json!({
                            "protocol": proto, "address": local_addr, "process": process
                        }));
                    }
                }
                (true, json!({"ports": ports_data}).to_string())
            }
            Err(e) => (false, format!("Error ejecutando docker: {}", e)),
        }
    } else if action == "docker_df" {
        match std::process::Command::new("docker")
            .args(["system", "df"])
            .output()
        {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let mut df_data = Vec::new();
                for line in stdout.lines().skip(1) {
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() >= 5 {
                        df_data.push(json!({
                            "type": parts[0], "total": parts[1], "active": parts[2],
                            "size": parts[3], "reclaimable": parts[4..].join(" ")
                        }));
                    }
                }
                (true, json!({"df": df_data}).to_string())
            }
            Err(e) => (false, format!("Error ejecutando docker df: {}", e)),
        }
    } else if action == "docker_prune" {
        match std::process::Command::new("docker")
            .args(["system", "prune", "-f"])
            .output()
        {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
                let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                let message = if !stdout.is_empty() { stdout } else { stderr };
                (
                    true,
                    json!({"message": message, "success": true}).to_string(),
                )
            }
            Err(e) => (false, format!("Error ejecutando docker prune: {}", e)),
        }
    } else if action == "installed_tools" {
        let output = std::process::Command::new("docker")
            .args([
                "run",
                "--rm",
                "--pid=host",
                "-v",
                "/:/host:ro",
                "alpine",
                "sh",
                "-c",
                r#"
apk add --no-cache procps > /dev/null 2>&1

# 1. Extraer versión de Rust/Cargo desde el manifiesto
get_rust_version() {
    local tc_dir=$(ls -d /host/root/.rustup/toolchains/* 2>/dev/null | head -1)
    if [ -n "$tc_dir" ]; then
        local manifest=$(ls "$tc_dir"/lib/rustlib/manifest-rustc-* 2>/dev/null | head -1)
        if [ -f "$manifest" ]; then
            grep -oE 'rust-[0-9]+\.[0-9]+\.[0-9]+' "$manifest" | head -1 | sed 's/rust-//'
            return
        fi
    fi
    echo "N/A"
}

# 2. Obtener versión desde APT (dpkg)
get_dpkg_ver() {
    local pkg=""
    case "$1" in
        python3) pkg="python3" ;; node) pkg="nodejs" ;; nginx) pkg="nginx" ;;
        apache2) pkg="apache2" ;; psql) pkg="postgresql-client" ;;
        mysql) pkg="mysql-client" ;; redis-server) pkg="redis-server" ;;
        docker) pkg="docker-ce" ;; docker-compose) pkg="docker-compose-plugin" ;;
        git) pkg="git" ;; curl) pkg="curl" ;; wget) pkg="wget" ;;
        ssh) pkg="openssh-client" ;; make) pkg="make" ;; gcc) pkg="gcc" ;;
        netdata) pkg="netdata" ;; htop) pkg="htop" ;; tmux) pkg="tmux" ;;
        vim) pkg="vim" ;; containerd) pkg="containerd.io" ;;
        *) pkg="" ;;
    esac
    
    if [ -n "$pkg" ]; then
        awk -v pkg="$pkg" '
            /^Package: / { if ($2 == pkg) found=1; else found=0 }
            found && /^Version: / { print $2; exit }
        ' /host/var/lib/dpkg/status | sed 's/^[^:]*://' | sed 's/-.*//'
    fi
}

# 3. Obtener fecha de modificación del binario
get_date() {
    for d in /host/usr/bin /host/usr/sbin /host/usr/local/bin /host/root/.cargo/bin; do
        if [ -x "$d/$1" ]; then
            stat -c %y "$d/$1" 2>/dev/null | cut -d. -f1
            return
        fi
    done
    echo "N/A"
}

# 4. VERIFICAR ESTADO (Solo para servicios systemd, N/A para herramientas)
get_status() {
    local bin=$1
    local service_name=""
    
    # Mapeo exclusivo para servicios que corren en segundo plano
    case "$bin" in
        nginx) service_name="nginx" ;;
        docker) service_name="docker" ;;
        containerd) service_name="containerd" ;;
        ssh) service_name="ssh" ;;
        netdata) service_name="netdata" ;;
        apache2) service_name="apache2" ;;
        *) service_name="" ;; # El resto son herramientas, no servicios
    esac

    if [ -n "$service_name" ]; then
        # Preguntar a systemd si el servicio está activo
        if systemctl is-active "$service_name" >/dev/null 2>&1; then
            echo "running"
        else
            echo "stopped"
        fi
    else
        # Para herramientas como git, curl, rustc, make, etc.
        echo "N/A"
    fi
}

# 5. Función principal de detección
check() {
    local bin=$1 name=$2 cat=$3
    local path=""
    
    for d in /host/usr/bin /host/usr/sbin /host/usr/local/bin /host/root/.cargo/bin; do
        if [ -x "$d/$bin" ]; then
            path="$d/$bin"
            break
        fi
    done
    
    [ -z "$path" ] && return

    local ver="N/A"
    case "$bin" in
        rustc|cargo) ver=$(get_rust_version) ;;
        *) ver=$(get_dpkg_ver "$bin") ;;
    esac
    [ -z "$ver" ] && ver="N/A"

    echo "${cat}|${name}|${bin}|${ver}|$(get_date $bin)|$(get_status $bin)"
}

# Lista de herramientas
check rustc Rust languages
check cargo Cargo languages
check python3 Python languages
check node Node.js languages
check nginx Nginx web_servers
check apache2 Apache web_servers
check docker Docker containers
check docker-compose "Docker Compose" containers
check containerd containerd containers
check git Git dev_tools
check curl cURL dev_tools
check wget Wget dev_tools
check ssh OpenSSH dev_tools
check make Make dev_tools
check gcc GCC dev_tools
check netdata Netdata monitoring
check htop htop monitoring
check tmux tmux utilities
check vim Vim utilities
check psql PostgreSQL databases
check mysql MySQL databases
check redis-server Redis databases
                "#,
            ])
            .output();

        match output {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let mut detected_tools = Vec::new();

                for line in stdout.lines() {
                    let parts: Vec<&str> = line.split('|').collect();
                    if parts.len() == 6 {
                        detected_tools.push(serde_json::json!({
                            "category": parts[0],
                            "name": parts[1],
                            "binary": parts[2],
                            "version": parts[3].trim().to_string(),
                            "install_date": parts[4].trim().to_string(),
                            "status": parts[5].trim().to_string()
                        }));
                    }
                }
                (
                    true,
                    serde_json::json!({"tools": detected_tools}).to_string(),
                )
            }
            Err(e) => (false, format!("Error ejecutando docker: {}", e)),
        }
    } else if action == "network_threats" {
        let output = std::process::Command::new("docker")
            .args([
                "run", "--rm", "--net=host", "-v", "/var/log:/host/var/log:ro", "alpine", "sh", "-c",
                r#"
apk add --no-cache iproute2 curl grep awk > /dev/null 2>&1

# ============================================
# FUENTE 1: Conexiones ACTIVAS (estado established)
# ============================================
ACTIVE_CONNECTIONS=$(ss -tun state established 2>/dev/null | awk '
NR>1 {
    local_addr = $4;
    peer_addr = $5;

    if (match(local_addr, /:[0-9]+$/)) {
        local_port = substr(local_addr, RSTART+1) + 0;
    } else {
        next;
    }

    if (local_port > 32768) next;

    sub(/:[0-9]+$/, "", peer_addr);
    gsub(/[\[\]]/, "", peer_addr);
    remote_ip = peer_addr;

    if (remote_ip ~ /^127\./) next;
    if (remote_ip ~ /^10\./) next;
    if (remote_ip ~ /^192\.168\./) next;
    if (remote_ip ~ /^172\.(1[6-9]|2[0-9]|3[01])\./) next;
    if (remote_ip == "::1") next;
    if (remote_ip ~ /^fd/) next;
    if (remote_ip ~ /^fe80/) next;

    count[remote_ip]++;
    if (index(ports[remote_ip], local_port",") == 0) {
        ports[remote_ip] = ports[remote_ip] local_port ",";
    }
}
END {
    for (ip in count) {
        sub(/,$/, "", ports[ip]);
        print ip "|" count[ip] "|" ports[ip] "|ACTIVE";
    }
}' | sort -t'|' -k2 -nr | head -n 10)

# ============================================
# FUENTE 2: Logs de autenticación SSH (intentos fallidos)
# ============================================
# Buscamos en /host/var/log/auth.log (montado desde el host)
FAILED_LOGINS=$(grep "Failed password" /host/var/log/auth.log 2>/dev/null | awk '{
    # Extraer IP del log
    for (i=1; i<=NF; i++) {
        if ($i == "from") {
            ip = $(i+1);
            break;
        }
    }
    if (ip != "" && ip !~ /^127\./ && ip !~ /^10\./ && ip !~ /^192\.168\./) {
        count[ip]++;
    }
}
END {
    for (ip in count) {
        print ip "|" count[ip] "|22|FAILED_LOGIN";
    }
}' | sort -t'|' -k2 -nr | head -n 10)

# ============================================
# COMBINAR AMBAS FUENTES
# ============================================
# Prioridad: FAILED_LOGIN > ACTIVE (un intento fallido es más sospechoso)
COMBINED_DATA=$(echo -e "$FAILED_LOGINS\n$ACTIVE_CONNECTIONS" | awk -F'|' '
{
    ip = $1;
    count = $2;
    ports = $3;
    source = $4;
    
    # Si la IP ya existe, sumar conteos
    if (ip in total_count) {
        total_count[ip] += count;
        if (source == "FAILED_LOGIN") {
            sources[ip] = "FAILED_LOGIN"; # Priorizar failed logins
        }
    } else {
        total_count[ip] = count;
        ports_list[ip] = ports;
        sources[ip] = source;
    }
}
END {
    for (ip in total_count) {
        print ip "|" total_count[ip] "|" ports_list[ip] "|" sources[ip];
    }
}' | sort -t'|' -k2 -nr | head -n 10)

if [ -z "$COMBINED_DATA" ]; then
    exit 0
fi

# ============================================
# ENRIQUECER CON NIVEL Y PAÍS
# ============================================
echo "$COMBINED_DATA" | while IFS='|' read -r ip count ports source; do
    if [ -n "$ip" ]; then
        level="SAFE"
        
        # Si hay intentos fallidos, es más sospechoso
        if [ "$source" = "FAILED_LOGIN" ]; then
            if [ "$count" -gt 3 ]; then level="WARNING"; fi
            if [ "$count" -gt 10 ]; then level="CRITICAL"; fi
        else
            # Solo conexiones activas
            if [ "$count" -gt 15 ]; then level="WARNING"; fi
            if [ "$count" -gt 50 ]; then level="CRITICAL"; fi
        fi

        country=$(curl -s --max-time 1.5 "http://ip-api.com/csv/$ip?fields=countryCode" 2>/dev/null | tr -d '[:space:]' | tr '[:lower:]' '[:upper:]')
        
        if [ ${#country} -ne 2 ]; then
            country="XX"
        fi
        
        echo "${ip}|${count}|${ports}|${level}|${country}"
    fi
done
                "#,
            ])
            .output();

        match output {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let mut threats = Vec::new();

                for line in stdout.lines() {
                    let parts: Vec<&str> = line.split('|').collect();
                    if parts.len() == 5 {
                        threats.push(serde_json::json!({
                            "ip": parts[0],
                            "connections": parts[1].trim().parse::<u32>().unwrap_or(0),
                            "ports": parts[2],
                            "level": parts[3].trim(),
                            "country": parts[4].trim().to_uppercase()
                        }));
                    }
                }
                (true, serde_json::json!({"threats": threats}).to_string())
            }
            Err(e) => (false, format!("Error ejecutando radar de red: {}", e)),
        }
    } else if action == "set_admin_ip" {
        // Este comando se llama desde el frontend para marcar la IP propia
        if let Some(ip) = payload.get("ip").and_then(|v| v.as_str()) {
            // Necesitamos acceso a la DB, pero execute_action no la tiene
            // Solución: lo manejamos en main.rs directamente
            (
                true,
                json!({"ip": ip, "message": "IP agregada a whitelist"}).to_string(),
            )
        } else {
            (false, "No se proporcionó IP".to_string())
        }
    } else if action == "get_top_attackers" {
        // Similar, necesitamos acceso a la DB
        (true, "NEED_DB_ACCESS".to_string()) // Placeholder, lo manejamos en main.rs
    } else if action == "get_active_connections" {
        let output = std::process::Command::new("docker")
        .args([
            "run",
            "--rm",
            "--net=host",
            "--pid=host",
            "-v",
            "/proc:/proc:ro",
            "alpine",
            "sh",
            "-c",
            r#"
apk add --no-cache iproute2 procps curl net-tools > /dev/null 2>&1

SERVER_IPV4=$(curl -s --max-time 2 -4 ifconfig.me 2>/dev/null)
[ -z "$SERVER_IPV4" ] && SERVER_IPV4="unknown"

echo "SERVER_IPV4:$SERVER_IPV4"

netstat -tnp 2>/dev/null | grep ESTABLISHED | while IFS= read -r line; do
    [ -z "$line" ] && continue
    
    local_addr=$(echo "$line" | awk '{print $4}')
    peer_addr=$(echo "$line" | awk '{print $5}')
    pid_proc=$(echo "$line" | awk '{print $7}')
    
    [ -z "$local_addr" ] || [ -z "$peer_addr" ] && continue
    
    local_ip=$(echo "$local_addr" | cut -d: -f1)
    local_port=$(echo "$local_addr" | cut -d: -f2)
    
    peer_ip=$(echo "$peer_addr" | cut -d: -f1)
    peer_port=$(echo "$peer_addr" | cut -d: -f2)
    
    # Solo IPv4
    if echo "$peer_ip" | grep -q ':'; then
        continue
    fi
    
    # Filtrar IPs privadas
    if echo "$peer_ip" | grep -qE '^(127\.|10\.|192\.168\.|172\.(1[6-9]|2[0-9]|3[01])\.)'; then
        continue
    fi
    
    # Extraer PID
    pid=$(echo "$pid_proc" | cut -d/ -f1)
    
    # Si no hay PID, usar ss
    if [ -z "$pid" ] || [ "$pid" = "-" ]; then
        pid=$(ss -tnp state established 2>/dev/null | grep ":$peer_port " | grep -oE 'pid=[0-9]+' | head -1 | cut -d= -f2)
    fi
    
    [ -z "$pid" ] && pid=0
    
    # Consultar país
    country=$(curl -s --max-time 1.5 "http://ip-api.com/csv/$peer_ip?fields=countryCode" 2>/dev/null | tr -d '[:space:]' | tr '[:lower:]' '[:upper:]')
    [ ${#country} -ne 2 ] && country="XX"
    
    echo "CONN:$local_ip|$local_port|$peer_ip|$peer_port|$pid|$country"
done
"#,
        ])
        .output();

        match output {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let mut server_ipv4 = "unknown".to_string();
                let mut connections = Vec::new();

                for line in stdout.lines() {
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }

                    if line.starts_with("SERVER_IPV4:") {
                        server_ipv4 = line[12..].to_string();
                        continue;
                    }

                    if line.starts_with("CONN:") {
                        let parts: Vec<&str> = line[5..].split('|').collect();
                        if parts.len() == 6 {
                            let local_port = parts[1].parse::<u32>().unwrap_or(0);
                            let peer_port = parts[3].parse::<u32>().unwrap_or(0);
                            let pid = parts[4].parse::<u32>().unwrap_or(0);

                            if local_port > 0 && local_port < 32768 && peer_port > 0 {
                                connections.push(serde_json::json!({
                                    "local_ip": parts[0],
                                    "local_port": local_port,
                                    "peer_ip": parts[2],
                                    "peer_port": peer_port,
                                    "pid": pid,
                                    "country": parts[5].to_uppercase()
                                }));
                            }
                        }
                    }
                }

                (
                    true,
                    serde_json::json!({
                        "server_ip": server_ipv4,
                        "connections": connections
                    })
                    .to_string(),
                )
            }
            Err(e) => {
                eprintln!("Error ejecutando docker para get_active_connections: {}", e);
                (false, format!("Error ejecutando docker: {}", e))
            }
        }
    } else if let Some(&(_, program, args)) =
        ALLOWED_COMMANDS.iter().find(|&&(cmd, _, _)| cmd == action)
    {
        crate::commands::execute_safe_command(program, args)
    } else {
        (
            false,
            json!({"error": format!("Comando '{}' no permitido por seguridad", action)})
                .to_string(),
        )
    }
}
