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
# WHITELIST DE USUARIOS E IPS CONOCIDAS
# ============================================
EXPECTED_USERS="root|admin|deploy|ubuntu"
KNOWN_IPS="79.117.90.148|1.2.3.4"  # Ajusta con tus IPs confiables

# ============================================
# 1. IP DEL SERVIDOR
# ============================================
SERVER_IP=$(ip -4 addr show scope global | awk '/inet / {print $2}' | cut -d/ -f1 | head -n1)
[ -z "$SERVER_IP" ] && SERVER_IP="unknown"

# ============================================
# 2. SESIONES SSH (usuarios autenticados)
# ============================================
# Usamos 'w' para sesiones reales y 'ss' para conexiones SSH establecidas
SSH_RAW=$(w -hs 2>/dev/null | awk '{
    user=$1; from=$3; login=$4; idle=$5;
    for(i=6;i<=NF;i++) what=what" "$i;
    print user "|" from "|" login "|" idle "|" what;
    what="";
}')
SSH_IPS=$(echo "$SSH_RAW" | awk -F'|' '{print $2}' | sort -u)
# Añadir IPs de conexiones SSH (por si no aparecen en w)
SSH_CONN_IPS=$(ss -tn state established dport :22 2>/dev/null | awk 'NR>1 {split($5,peer,":"); gsub(/[\[\]]/,"",peer[1]); print peer[1]}' | sort -u)
ALL_SSH_IPS=$(echo -e "$SSH_IPS\n$SSH_CONN_IPS" | sort -u | grep -v '^$')

# Construir array SSH
SSH_SESSIONS=""
for ip in $ALL_SSH_IPS; do
    # Buscar en 'w' (prioridad)
    SESSION=$(echo "$SSH_RAW" | grep "|$ip|" | head -1)
    if [ -n "$SESSION" ]; then
        USER=$(echo "$SESSION" | cut -d'|' -f1)
        LOGIN=$(echo "$SESSION" | cut -d'|' -f3)
        IDLE=$(echo "$SESSION" | cut -d'|' -f4)
        WHAT=$(echo "$SESSION" | cut -d'|' -f5-)
    else
        USER="unknown"
        LOGIN="N/A"
        IDLE="N/A"
        WHAT="ssh-connection"
    fi

    # Estados
    if echo "$EXPECTED_USERS" | grep -q "$USER"; then
        USER_STATUS="EXPECTED"
    else
        USER_STATUS="SUSPICIOUS"
    fi

    if echo "$ip" | grep -qE '^127\.|^10\.|^192\.168\.|^172\.(1[6-9]|2[0-9]|3[01])\.'; then
        IP_STATUS="INTERNAL"
    elif echo "$KNOWN_IPS" | grep -q "$ip"; then
        IP_STATUS="KNOWN_EXTERNAL"
    else
        IP_STATUS="EXTERNAL"
    fi

    SUSPICIOUS_CMD=0
    if echo "$WHAT" | grep -qE 'bash|nc |nohup|python -m|perl -e|ruby -e|sh -i|socat|telnet'; then
        if [ "$USER" != "root" ] && [ "$USER" != "admin" ]; then
            SUSPICIOUS_CMD=1
        fi
    fi

    # País (solo externas)
    COUNTRY="XX"
    if [ "$IP_STATUS" = "EXTERNAL" ] || [ "$IP_STATUS" = "KNOWN_EXTERNAL" ]; then
        COUNTRY=$(curl -s --max-time 1.5 "http://ip-api.com/csv/$ip?fields=countryCode" 2>/dev/null | tr -d '[:space:]' | tr '[:lower:]' '[:upper:]')
        [ ${#COUNTRY} -ne 2 ] && COUNTRY="XX"
    fi

    SSH_SESSIONS="${SSH_SESSIONS}${USER}|${ip}|${LOGIN}|${IDLE}|${WHAT}|${USER_STATUS}|${IP_STATUS}|${SUSPICIOUS_CMD}|${COUNTRY}\n"
done

# ============================================
# 3. CONEXIONES WEB (puertos 80 y 443)
# ============================================
WEB_RAW=$(ss -tn state established 2>/dev/null | awk '
NR>1 {
    split($4, local, ":");
    local_port = local[length(local)];
    if (local_port != 80 && local_port != 443) next;
    split($5, peer, ":");
    remote_ip = peer[1];
    gsub(/[\[\]]/, "", remote_ip);
    if (remote_ip ~ /^127\.|^10\.|^192\.168\.|^172\.(1[6-9]|2[0-9]|3[01])\.|::1/) next;
    key = remote_ip "|" local_port;
    count[key]++;
}
END {
    for (k in count) {
        split(k, arr, "|");
        print arr[1] "|" arr[2] "|" count[k];
    }
}')

WEB_SESSIONS=""
for line in $WEB_RAW; do
    ip=$(echo "$line" | cut -d'|' -f1)
    port=$(echo "$line" | cut -d'|' -f2)
    cnt=$(echo "$line" | cut -d'|' -f3)
    COUNTRY=$(curl -s --max-time 1.5 "http://ip-api.com/csv/$ip?fields=countryCode" 2>/dev/null | tr -d '[:space:]' | tr '[:lower:]' '[:upper:]')
    [ ${#COUNTRY} -ne 2 ] && COUNTRY="XX"
    WEB_SESSIONS="${WEB_SESSIONS}${ip}|${port}|${cnt}|${COUNTRY}\n"
done

# ============================================
# 4. GENERAR JSON FINAL
# ============================================
escape() {
    echo "$1" | sed 's/\\/\\\\/g; s/"/\\"/g; s/\n/\\n/g'
}

build_json() {
    echo "{"
    echo "  \"server_ip\":\"$SERVER_IP\","
    echo "  \"ssh_sessions\":["
    first=true
    if [ -n "$SSH_SESSIONS" ]; then
        echo "$SSH_SESSIONS" | while IFS='|' read -r user from login idle what user_status ip_status suspicious_cmd country; do
            [ -z "$user" ] && continue
            if [ "$first" = true ]; then first=false; else echo ","; fi
            echo "    {"
            echo "      \"user\":\"$(escape "$user")\","
            echo "      \"from\":\"$(escape "$from")\","
            echo "      \"login\":\"$(escape "$login")\","
            echo "      \"idle\":\"$(escape "$idle")\","
            echo "      \"what\":\"$(escape "$what")\","
            echo "      \"user_status\":\"$(escape "$user_status")\","
            echo "      \"ip_status\":\"$(escape "$ip_status")\","
            echo "      \"suspicious_command\":$suspicious_cmd,"
            echo "      \"country\":\"$(escape "$country")\""
            echo -n "    }"
        done
    fi
    echo ""
    echo "  ],"
    echo "  \"web_connections\":["
    first=true
    if [ -n "$WEB_SESSIONS" ]; then
        echo "$WEB_SESSIONS" | while IFS='|' read -r ip port cnt country; do
            [ -z "$ip" ] && continue
            if [ "$first" = true ]; then first=false; else echo ","; fi
            echo "    {"
            echo "      \"peer_ip\":\"$(escape "$ip")\","
            echo "      \"port\":$port,"
            echo "      \"count\":$cnt,"
            echo "      \"country\":\"$(escape "$country")\""
            echo -n "    }"
        done
    fi
    echo ""
    echo "  ]"
    echo "}"
}

build_json
                "#,
            ])
            .output();

        match output {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                if stdout.trim().is_empty() || stdout.trim() == "{}" {
                    (
                        true,
                        r#"{"server_ip":"unknown","ssh_sessions":[],"web_connections":[]}"#
                            .to_string(),
                    )
                } else {
                    (true, stdout.to_string())
                }
            }
            Err(e) => (false, format!("Error: {}", e)),
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
                "run", "--rm", "--net=host", "-v", "/var/log:/host/var/log:ro", "alpine", "sh", "-c",
                r#"
apk add --no-cache iproute2 curl grep awk whois coreutils util-linux > /dev/null 2>&1

# ============================================
# WHITELIST
# ============================================
EXPECTED_USERS="root|admin|deploy|ubuntu"
KNOWN_IPS="79.117.90.148"  # Ajusta con tus IPs

# ============================================
# 1. IP DEL SERVIDOR
# ============================================
SERVER_IP=$(ip -4 addr show scope global | awk '/inet / {print $2}' | cut -d/ -f1 | head -n1)
[ -z "$SERVER_IP" ] && SERVER_IP="unknown"

# ============================================
# 2. SESIONES SSH (usuarios autenticados)
# ============================================
SSH_RAW=$(w -hs 2>/dev/null | awk '{
    user=$1; from=$3; login=$4; idle=$5;
    for(i=6;i<=NF;i++) what=what" "$i;
    print user "|" from "|" login "|" idle "|" what;
    what="";
}')
SSH_IPS=$(echo "$SSH_RAW" | awk -F'|' '{print $2}' | sort -u)
SSH_CONN_IPS=$(ss -tn state established dport :22 2>/dev/null | awk 'NR>1 {split($5,peer,":"); gsub(/[\[\]]/,"",peer[1]); print peer[1]}' | sort -u)
ALL_SSH_IPS=$(echo -e "$SSH_IPS\n$SSH_CONN_IPS" | sort -u | grep -v '^$')

SSH_SESSIONS=""
for ip in $ALL_SSH_IPS; do
    SESSION=$(echo "$SSH_RAW" | grep "|$ip|" | head -1)
    if [ -n "$SESSION" ]; then
        USER=$(echo "$SESSION" | cut -d'|' -f1)
        LOGIN=$(echo "$SESSION" | cut -d'|' -f3)
        IDLE=$(echo "$SESSION" | cut -d'|' -f4)
        WHAT=$(echo "$SESSION" | cut -d'|' -f5-)
    else
        USER="unknown"
        LOGIN="N/A"
        IDLE="N/A"
        WHAT="ssh-connection"
    fi

    if echo "$EXPECTED_USERS" | grep -q "$USER"; then
        USER_STATUS="EXPECTED"
    else
        USER_STATUS="SUSPICIOUS"
    fi

    if echo "$ip" | grep -qE '^127\.|^10\.|^192\.168\.|^172\.(1[6-9]|2[0-9]|3[01])\.'; then
        IP_STATUS="INTERNAL"
    elif echo "$KNOWN_IPS" | grep -q "$ip"; then
        IP_STATUS="KNOWN_EXTERNAL"
    else
        IP_STATUS="EXTERNAL"
    fi

    SUSPICIOUS_CMD=0
    if echo "$WHAT" | grep -qE 'bash|nc |nohup|python -m|perl -e|ruby -e|sh -i|socat|telnet'; then
        if [ "$USER" != "root" ] && [ "$USER" != "admin" ]; then
            SUSPICIOUS_CMD=1
        fi
    fi

    COUNTRY="XX"
    if [ "$IP_STATUS" = "EXTERNAL" ] || [ "$IP_STATUS" = "KNOWN_EXTERNAL" ]; then
        COUNTRY=$(curl -s --max-time 1.5 "http://ip-api.com/csv/$ip?fields=countryCode" 2>/dev/null | tr -d '[:space:]' | tr '[:lower:]' '[:upper:]')
        [ ${#COUNTRY} -ne 2 ] && COUNTRY="XX"
    fi

    SSH_SESSIONS="${SSH_SESSIONS}${USER}|${ip}|${LOGIN}|${IDLE}|${WHAT}|${USER_STATUS}|${IP_STATUS}|${SUSPICIOUS_CMD}|${COUNTRY}\n"
done

# ============================================
# 3. CONEXIONES WEB (puertos 80 y 443)
# ============================================
WEB_RAW=$(ss -tn state established 2>/dev/null | awk '
NR>1 {
    split($4, local, ":");
    local_port = local[length(local)];
    if (local_port != 80 && local_port != 443) next;
    split($5, peer, ":");
    remote_ip = peer[1];
    gsub(/[\[\]]/, "", remote_ip);
    if (remote_ip ~ /^127\.|^10\.|^192\.168\.|^172\.(1[6-9]|2[0-9]|3[01])\.|::1/) next;
    key = remote_ip "|" local_port;
    count[key]++;
}
END {
    for (k in count) {
        split(k, arr, "|");
        print arr[1] "|" arr[2] "|" count[k];
    }
}')

WEB_SESSIONS=""
for line in $WEB_RAW; do
    ip=$(echo "$line" | cut -d'|' -f1)
    port=$(echo "$line" | cut -d'|' -f2)
    cnt=$(echo "$line" | cut -d'|' -f3)
    COUNTRY=$(curl -s --max-time 1.5 "http://ip-api.com/csv/$ip?fields=countryCode" 2>/dev/null | tr -d '[:space:]' | tr '[:lower:]' '[:upper:]')
    [ ${#COUNTRY} -ne 2 ] && COUNTRY="XX"
    WEB_SESSIONS="${WEB_SESSIONS}${ip}|${port}|${cnt}|${COUNTRY}\n"
done

# ============================================
# 4. GENERAR JSON FINAL (usando jq si existe, o manual)
# ============================================
# Intentar usar jq si está instalado
if command -v jq >/dev/null 2>&1; then
    # Construir arrays con jq
    SSH_JSON="[]"
    if [ -n "$SSH_SESSIONS" ]; then
        SSH_JSON=$(echo "$SSH_SESSIONS" | while IFS='|' read -r user from login idle what user_status ip_status suspicious_cmd country; do
            [ -z "$user" ] && continue
            jq -n \
                --arg user "$user" \
                --arg from "$from" \
                --arg login "$login" \
                --arg idle "$idle" \
                --arg what "$what" \
                --arg user_status "$user_status" \
                --arg ip_status "$ip_status" \
                --argjson suspicious_cmd "$suspicious_cmd" \
                --arg country "$country" \
                '{user:$user, from:$from, login:$login, idle:$idle, what:$what, user_status:$user_status, ip_status:$ip_status, suspicious_command:$suspicious_cmd, country:$country}'
        done | jq -s '.')
    fi

    WEB_JSON="[]"
    if [ -n "$WEB_SESSIONS" ]; then
        WEB_JSON=$(echo "$WEB_SESSIONS" | while IFS='|' read -r ip port cnt country; do
            [ -z "$ip" ] && continue
            jq -n \
                --arg peer_ip "$ip" \
                --argjson port "$port" \
                --argjson count "$cnt" \
                --arg country "$country" \
                '{peer_ip:$peer_ip, port:$port, count:$count, country:$country}'
        done | jq -s '.')
    fi

    jq -n \
        --arg server_ip "$SERVER_IP" \
        --argjson ssh_sessions "$SSH_JSON" \
        --argjson web_connections "$WEB_JSON" \
        '{server_ip:$server_ip, ssh_sessions:$ssh_sessions, web_connections:$web_connections}'
else
    # Fallback: construir JSON manualmente (solo para casos sin jq)
    echo "{\"server_ip\":\"$SERVER_IP\",\"ssh_sessions\":["
    first=true
    if [ -n "$SSH_SESSIONS" ]; then
        echo "$SSH_SESSIONS" | while IFS='|' read -r user from login idle what user_status ip_status suspicious_cmd country; do
            [ -z "$user" ] && continue
            if [ "$first" = true ]; then first=false; else echo ","; fi
            # Escapar caracteres especiales para JSON
            user=$(echo "$user" | sed 's/\\/\\\\/g; s/"/\\"/g')
            from=$(echo "$from" | sed 's/\\/\\\\/g; s/"/\\"/g')
            login=$(echo "$login" | sed 's/\\/\\\\/g; s/"/\\"/g')
            idle=$(echo "$idle" | sed 's/\\/\\\\/g; s/"/\\"/g')
            what=$(echo "$what" | sed 's/\\/\\\\/g; s/"/\\"/g')
            user_status=$(echo "$user_status" | sed 's/\\/\\\\/g; s/"/\\"/g')
            ip_status=$(echo "$ip_status" | sed 's/\\/\\\\/g; s/"/\\"/g')
            country=$(echo "$country" | sed 's/\\/\\\\/g; s/"/\\"/g')
            echo "{\"user\":\"$user\",\"from\":\"$from\",\"login\":\"$login\",\"idle\":\"$idle\",\"what\":\"$what\",\"user_status\":\"$user_status\",\"ip_status\":\"$ip_status\",\"suspicious_command\":$suspicious_cmd,\"country\":\"$country\"}"
        done
    fi
    echo "],\"web_connections\":["
    first=true
    if [ -n "$WEB_SESSIONS" ]; then
        echo "$WEB_SESSIONS" | while IFS='|' read -r ip port cnt country; do
            [ -z "$ip" ] && continue
            if [ "$first" = true ]; then first=false; else echo ","; fi
            ip=$(echo "$ip" | sed 's/\\/\\\\/g; s/"/\\"/g')
            country=$(echo "$country" | sed 's/\\/\\\\/g; s/"/\\"/g')
            echo "{\"peer_ip\":\"$ip\",\"port\":$port,\"count\":$cnt,\"country\":\"$country\"}"
        done
    fi
    echo "]}"
fi
                "#,
            ])
            .output();

        match output {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                if stdout.trim().is_empty() || stdout.trim() == "{}" {
                    (
                        true,
                        r#"{"server_ip":"unknown","ssh_sessions":[],"web_connections":[]}"#
                            .to_string(),
                    )
                } else {
                    (true, stdout.to_string())
                }
            }
            Err(e) => (false, format!("Error: {}", e)),
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
