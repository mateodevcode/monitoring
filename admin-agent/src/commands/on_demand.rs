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
        // Usar log_analyzer para analizar access.log
        let log_path = "/var/log/nginx/access.log";
        let window_secs = 2;
        let limit = 50;

        // Obtener whitelist actual (se pasará desde el llamador, pero aquí no tenemos acceso a la DB)
        // Para simplificar, usaremos una lista vacía y el llamador (main.rs) filtrará
        // o mejor: devolvemos todos los datos y main.rs los procesa con whitelist.
        // Pero como execute_action no tiene acceso a la DB, devolvemos los datos sin filtrar.
        let whitelist: Vec<String> = Vec::new();
        let threats = crate::commands::log_analyzer::analyze_logs_with_whitelist(
            log_path,
            window_secs,
            limit,
            &whitelist,
        );
        (true, json!({ "threats": threats }).to_string())
    } else if action == "set_admin_ip" {
        if let Some(ip) = payload.get("ip").and_then(|v| v.as_str()) {
            (
                true,
                json!({"ip": ip, "message": "IP agregada a whitelist"}).to_string(),
            )
        } else {
            (false, "No se proporcionó IP".to_string())
        }
    } else if action == "get_top_attackers" {
        // Este se maneja en main.rs, pero por si acaso devolvemos placeholder
        (true, "NEED_DB_ACCESS".to_string())
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
