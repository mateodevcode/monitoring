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
apk add --no-cache iproute2 curl grep awk whois coreutils util-linux > /dev/null 2>&1

# ============================================
# WHITELIST DE USUARIOS E IPS ESPERADAS
# ============================================
EXPECTED_USERS="root|admin|deploy|ubuntu"
EXPECTED_IPS="127.0.0.1|::1|10.0.0.0/8|192.168.0.0/16|172.16.0.0/12"

# ============================================
# FUENTE 1: SESIONES VIVAS (w - usuarios REALMENTE conectados)
# ============================================
echo "=== SESIONES_ACTIVAS ==="
w -hs 2>/dev/null | while read line; do
    USER=$(echo "$line" | awk '{print $1}')
    FROM=$(echo "$line" | awk '{print $3}')
    LOGIN=$(echo "$line" | awk '{print $4}')
    IDLE=$(echo "$line" | awk '{print $5}')
    WHAT=$(echo "$line" | awk '{$1=$2=$3=$4=$5=$6=$7=$8=""; print $0}' | sed 's/^ *//')
    
    # Verificar si el usuario es esperado
    if echo "$EXPECTED_USERS" | grep -q "$USER"; then
        USER_STATUS="EXPECTED"
    else
        USER_STATUS="SUSPICIOUS"
    fi
    
    # Verificar si la IP es esperada
    if echo "$FROM" | grep -qE '^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$'; then
        if echo "$FROM" | grep -qE '^(127\.|10\.|192\.168\.|172\.(1[6-9]|2[0-9]|3[01])\.)'; then
            IP_STATUS="INTERNAL"
        else
            IP_STATUS="EXTERNAL"
            # Verificar contra whitelist de IPs externas conocidas
            if [ "$FROM" = "79.117.90.148" ] || [ "$FROM" = "1.2.3.4" ]; then
                IP_STATUS="KNOWN"
            fi
        fi
    else
        IP_STATUS="UNKNOWN"
    fi
    
    # Detectar reverse shells / comandos sospechosos
    SUSPICIOUS_CMD=0
    if echo "$WHAT" | grep -qE 'bash|nc |nohup|python -m|perl -e|ruby -e|sh -i|socat|telnet'; then
        if [ "$USER" != "root" ] && [ "$USER" != "admin" ]; then
            SUSPICIOUS_CMD=1
        fi
    fi
    
    echo "${USER}|${FROM}|${LOGIN}|${IDLE}|${WHAT}|${USER_STATUS}|${IP_STATUS}|${SUSPICIOUS_CMD}"
done

# ============================================
# FUENTE 2: LOGINS EXITOSOS RECIENTES (last)
# ============================================
echo "=== LOGINS_RECIENTES ==="
last -f /host/var/log/wtmp 2>/dev/null | head -20 | grep -v "^$\|^wtmp\|reboot" | while read line; do
    USER=$(echo "$line" | awk '{print $1}')
    TTY=$(echo "$line" | awk '{print $2}')
    FROM=$(echo "$line" | awk '{print $3}')
    DATE=$(echo "$line" | awk '{print $4, $5, $6, $7, $8}')
    
    echo "${USER}|${TTY}|${FROM}|${DATE}"
done

# ============================================
# FUENTE 3: PUERTOS ABIERTOS (para contexto)
# ============================================
echo "=== PUERTOS_ABIERTOS ==="
ss -tlnp 2>/dev/null | grep LISTEN | awk '
{
    split($4, addr, ":");
    port = addr[length(addr)];
    process = $NF;
    
    # Extraer nombre del proceso
    if (match(process, /\([^)]+\)/)) {
        proc_name = substr(process, RSTART+1, RLENGTH-2);
    } else {
        proc_name = process;
    }
    
    print port "|" proc_name;
}' | sort -u

# ============================================
# FUENTE 4: CONEXIONES SSH ESTABLECIDAS (SOLO para confirmar)
# ============================================
echo "=== CONEXIONES_SSH ==="
ss -tn state established dport :22 2>/dev/null | awk '
NR>1 {
    split($5, peer, ":");
    remote_ip = peer[1];
    gsub(/[\[\]]/, "", remote_ip);
    
    if (remote_ip !~ /^127\./ && remote_ip !~ /^10\./ && 
        remote_ip !~ /^192\.168\./ && remote_ip !~ /^172\.(1[6-9]|2[0-9]|3[01])\./) {
        print remote_ip;
    }
}' | sort -u

# ============================================
# COMBINAR Y GENERAR INFORME
# ============================================
echo "=== INFORME_FINAL ==="

# Contar sesiones activas
ACTIVE_SESSIONS=$(w -hs 2>/dev/null | wc -l)
SUSPICIOUS_SESSIONS=$(w -hs 2>/dev/null | awk -v users="$EXPECTED_USERS" '
    BEGIN { split(users, u, "|"); suspicious=0; }
    {
        user=$1;
        is_expected=0;
        for (i in u) {
            if (u[i] == user) { is_expected=1; break; }
        }
        if (!is_expected) suspicious++;
    }
    END { print suspicious; }
')

echo "SESIONES_TOTAL=${ACTIVE_SESSIONS}"
echo "SESIONES_SOSPECHOSAS=${SUSPICIOUS_SESSIONS}"

# Verificar si hay usuarios root desde IPs externas
ROOT_EXTERNAL=$(w -hs 2>/dev/null | awk '$1=="root" && $3 !~ /^(127\.|10\.|192\.168\.|172\.(1[6-9]|2[0-9]|3[01])\.)/ {print $3}')

if [ -n "$ROOT_EXTERNAL" ]; then
    echo "ROOT_EXTERNO=1"
    echo "IP_ROOT_EXTERNO=${ROOT_EXTERNAL}"
else
    echo "ROOT_EXTERNO=0"
fi

echo "TIMESTAMP=$(date +%s)"
                "#,
            ])
            .output();

        match output {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let mut sessions = Vec::new();
                let mut suspicious = Vec::new();
                let mut logins = Vec::new();
                let mut ports = Vec::new();
                let mut ssh_connections = Vec::new();

                let mut current_section = "";
                let mut root_external = 0;
                let mut root_external_ip = "";

                for line in stdout.lines() {
                    if line.starts_with("=== SESIONES_ACTIVAS ===") {
                        current_section = "sessions";
                        continue;
                    } else if line.starts_with("=== LOGINS_RECIENTES ===") {
                        current_section = "logins";
                        continue;
                    } else if line.starts_with("=== PUERTOS_ABIERTOS ===") {
                        current_section = "ports";
                        continue;
                    } else if line.starts_with("=== CONEXIONES_SSH ===") {
                        current_section = "ssh";
                        continue;
                    } else if line.starts_with("=== INFORME_FINAL ===") {
                        current_section = "report";
                        continue;
                    }

                    if line.trim().is_empty() {
                        continue;
                    }

                    match current_section {
                        "sessions" => {
                            let parts: Vec<&str> = line.split('|').collect();
                            if parts.len() == 8 {
                                let user = parts[0];
                                let from = parts[1];
                                let login = parts[2];
                                let idle = parts[3];
                                let what = parts[4];
                                let user_status = parts[5];
                                let ip_status = parts[6];
                                let suspicious_cmd = parts[7].trim() == "1";

                                let session = serde_json::json!({
                                    "user": user,
                                    "from": from,
                                    "login": login,
                                    "idle": idle,
                                    "what": what,
                                    "user_status": user_status,
                                    "ip_status": ip_status,
                                    "suspicious_command": suspicious_cmd
                                });

                                sessions.push(session);

                                // Si es sospechoso, añadir a lista de sospechosos
                                if user_status == "SUSPICIOUS"
                                    || ip_status == "EXTERNAL"
                                    || suspicious_cmd
                                {
                                    suspicious.push(serde_json::json!({
                                        "user": user,
                                        "from": from,
                                        "reason": if user_status == "SUSPICIOUS" {
                                            "Usuario no esperado"
                                        } else if suspicious_cmd {
                                            "Comando sospechoso"
                                        } else {
                                            "IP externa no conocida"
                                        }
                                    }));
                                }
                            }
                        }
                        "logins" => {
                            let parts: Vec<&str> = line.split('|').collect();
                            if parts.len() == 4 {
                                logins.push(serde_json::json!({
                                    "user": parts[0],
                                    "tty": parts[1],
                                    "from": parts[2],
                                    "date": parts[3]
                                }));
                            }
                        }
                        "ports" => {
                            let parts: Vec<&str> = line.split('|').collect();
                            if parts.len() == 2 {
                                ports.push(serde_json::json!({
                                    "port": parts[0],
                                    "process": parts[1]
                                }));
                            }
                        }
                        "ssh" => {
                            ssh_connections.push(line);
                        }
                        "report" => {
                            if line.starts_with("ROOT_EXTERNO=") {
                                root_external = line
                                    .split('=')
                                    .nth(1)
                                    .unwrap_or("0")
                                    .parse::<i32>()
                                    .unwrap_or(0);
                            } else if line.starts_with("IP_ROOT_EXTERNO=") {
                                root_external_ip = line.split('=').nth(1).unwrap_or("");
                            }
                        }
                        _ => {}
                    }
                }

                // Determinar nivel de riesgo
                let risk_level = if root_external == 1 && !suspicious.is_empty() {
                    "CRITICAL"
                } else if !suspicious.is_empty() {
                    "WARNING"
                } else if sessions.len() > 5 {
                    "INFO"
                } else {
                    "SAFE"
                };

                let response = serde_json::json!({
                    "risk_level": risk_level,
                    "summary": {
                        "total_sessions": sessions.len(),
                        "suspicious_count": suspicious.len(),
                        "recent_logins": logins.len(),
                        "open_ports": ports.len(),
                        "ssh_connections": ssh_connections.len(),
                        "root_external": root_external == 1,
                        "root_external_ip": root_external_ip
                    },
                    "sessions": sessions,
                    "suspicious": suspicious,
                    "recent_logins": logins,
                    "open_ports": ports,
                    "ssh_connections": ssh_connections,
                    "timestamp": chrono::Utc::now().to_rfc3339()
                });

                (true, response.to_string())
            }
            Err(e) => (
                false,
                format!("Error ejecutando auditoría de sesiones: {}", e),
            ),
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
