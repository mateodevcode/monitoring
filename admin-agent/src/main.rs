mod auth;
mod commands;
mod db;

use auth::{login, require_auth, AuthState};
use axum::{
    extract::State,
    middleware,
    response::Json,
    routing::{get, post},
    Router,
};
use futures_util::{SinkExt, StreamExt};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::env;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{interval, Duration};
use tokio_tungstenite::connect_async;
use tower_governor::{governor::GovernorConfigBuilder, GovernorLayer};
use tracing::{error, info, warn};

// ============================================
// ESTRUCTURAS EXISTENTES
// ============================================
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct Event {
    id: String,
    channel: String,
    source: String,
    targets: Vec<String>,
    payload: serde_json::Value,
    timestamp: i64,
}

#[derive(Debug, Serialize)]
struct EmitEventRequest {
    source: String,
    targets: Vec<String>,
    payload: serde_json::Value,
}

type DbConnection = Arc<Mutex<Connection>>;

const AGENT_CLIENT_ID: &str = "admin-agent-01";
const LISTEN_CHANNEL_NAME: &str = "admin-actions";
const RESPONSE_CHANNEL_NAME: &str = "admin-events";
const DASHBOARD_INTERVAL_SECS: u64 = 5;

// ============================================
// FUNCIONES DE CONFIGURACIÓN
// ============================================
fn get_core_ws_url() -> String {
    env::var("CORE_WS_URL").unwrap_or_else(|_| "ws://localhost:3005/ws".to_string())
}

fn get_core_rest_url() -> String {
    env::var("CORE_REST_URL").unwrap_or_else(|_| "http://localhost:3005".to_string())
}

// ============================================
// HANDLERS PARA RUTAS PROTEGIDAS
// ============================================
async fn exec_handler(
    State(_db): State<DbConnection>,
    Json(payload): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let action = payload.get("action").and_then(|v| v.as_str());

    match action {
        Some(cmd) => {
            let (success, result) = commands::execute_action(cmd, &payload);
            Json(json!({
                "success": success,
                "output": result,
                "action": cmd
            }))
        }
        None => Json(json!({
            "success": false,
            "error": "Falta campo 'action'"
        })),
    }
}

async fn kill_handler(Json(_payload): Json<serde_json::Value>) -> Json<serde_json::Value> {
    Json(json!({
        "success": true,
        "message": "Process killed"
    }))
}

async fn files_handler(Json(_payload): Json<serde_json::Value>) -> Json<serde_json::Value> {
    Json(json!({
        "success": true,
        "files": []
    }))
}

async fn docker_handler(Json(_payload): Json<serde_json::Value>) -> Json<serde_json::Value> {
    Json(json!({
        "success": true,
        "containers": []
    }))
}

async fn health_handler() -> Json<serde_json::Value> {
    Json(json!({
        "status": "ok",
        "agent": AGENT_CLIENT_ID,
        "version": env!("CARGO_PKG_VERSION")
    }))
}

// ============================================
// FUNCIONES AUXILIARES (WebSocket + Core)
// ============================================
async fn get_or_create_channel(
    client: &reqwest::Client,
    core_rest_url: &str,
    channel_name: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let res = client
        .get(format!("{}/channels", core_rest_url))
        .send()
        .await?;
    let json: serde_json::Value = res.json().await?;

    if let Some(channels) = json["data"].as_array() {
        if let Some(ch) = channels.iter().find(|c| c["name"] == channel_name) {
            if let Some(id) = ch["id"].as_str() {
                info!("✅ Canal '{}' ya existe con ID: {}", channel_name, id);
                return Ok(id.to_string());
            }
        }
    }

    info!("🛠️ Creando canal '{}'...", channel_name);
    let create_res = client
        .post(format!("{}/channels", core_rest_url))
        .json(&json!({
            "name": channel_name,
            "description": format!("Canal auto-gestionado para {}", channel_name)
        }))
        .send()
        .await?;
    let create_json: serde_json::Value = create_res.json().await?;
    let new_id = create_json["data"]["id"]
        .as_str()
        .ok_or("No se pudo obtener el ID")?
        .to_string();

    info!("✅ Canal '{}' creado con ID: {}", channel_name, new_id);
    Ok(new_id)
}

async fn publish_to_core(
    core_rest_url: &str,
    channel_id: &str,
    payload: serde_json::Value,
) -> Result<(), Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();
    let req_body = EmitEventRequest {
        source: AGENT_CLIENT_ID.to_string(),
        targets: vec!["*".to_string()],
        payload,
    };
    let res = client
        .post(format!("{}/channels/{}/events", core_rest_url, channel_id))
        .json(&req_body)
        .send()
        .await?;

    if res.status().is_success() {
        Ok(())
    } else {
        Err(format!("HTTP Error: {}", res.status()).into())
    }
}

fn calculate_docker_delta(
    last: &Option<serde_json::Value>,
    current: &serde_json::Value,
) -> serde_json::Value {
    use std::collections::HashMap;

    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut changed = Vec::new();

    // Obtener arrays de contenedores
    let current_containers = current["containers"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .filter_map(|c| {
            let name = c["name"].as_str().unwrap_or("unknown");
            Some((name.to_string(), c.clone()))
        })
        .collect::<HashMap<_, _>>();

    let last_containers = if let Some(last_val) = last {
        last_val["containers"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .filter_map(|c| {
                let name = c["name"].as_str().unwrap_or("unknown");
                Some((name.to_string(), c.clone()))
            })
            .collect::<HashMap<_, _>>()
    } else {
        HashMap::new()
    };

    // Detectar AGREGADOS y CAMBIOS
    for (name, current_container) in &current_containers {
        if let Some(last_container) = last_containers.get(name) {
            // Comparar status
            let current_status = current_container["status"].as_str().unwrap_or("");
            let last_status = last_container["status"].as_str().unwrap_or("");

            if current_status != last_status {
                changed.push(json!({
                    "name": name,
                    "status_before": last_status,
                    "status_after": current_status,
                    "size": current_container["size"].as_str().unwrap_or("")
                }));
            }
        } else {
            // NUEVO contenedor
            added.push(json!({
                "name": name,
                "status": current_container["status"].as_str().unwrap_or(""),
                "size": current_container["size"].as_str().unwrap_or(""),
                "timestamp": chrono::Utc::now().to_rfc3339()
            }));

            tracing::warn!("🚨 NUEVO CONTENEDOR DETECTADO: {}", name);
        }
    }

    // Detectar ELIMINADOS
    for (name, _) in &last_containers {
        if !current_containers.contains_key(name) {
            removed.push(json!({
                "name": name,
                "timestamp": chrono::Utc::now().to_rfc3339()
            }));

            tracing::warn!("⚠️  CONTENEDOR ELIMINADO: {}", name);
        }
    }

    json!({
        "added": added,
        "removed": removed,
        "changed": changed
    })
}

// ============================================
// FUNCIÓN PRINCIPAL - INTEGRADA
// ============================================
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();
    dotenv::dotenv().ok();

    info!("🚀 Iniciando Rust Admin Agent con Autenticación...");

    // ==========================================
    // 1. INICIALIZAR AUTENTICACIÓN
    // ==========================================
    let auth_state = Arc::new(AuthState::from_env());
    info!("🔐 Sistema de autenticación inicializado");

    // ==========================================
    // 2. CONFIGURAR RATE LIMITING (Protección anti-fuerza bruta)
    // ==========================================
    // Permite un pico (burst) de 5 intentos de login inmediatos.
    // Luego, repone 1 intento cada 60 segundos.
    let governor_conf = Arc::new(
        GovernorConfigBuilder::default()
            .per_second(60)
            .burst_size(5)
            .finish()
            .unwrap(),
    );
    info!("🛡️ Rate limiting configurado: 5 intentos/60s por IP");

    // ==========================================
    // 3. INICIALIZAR BASE DE DATOS
    // ==========================================
    let db_path = env::var("DB_PATH").unwrap_or_else(|_| "threats.db".to_string());
    let conn = db::init_db(&db_path).expect("❌ Error fatal al inicializar la base de datos");
    let db_conn = Arc::new(Mutex::new(conn));
    info!("💾 Base de datos SQLite: {}", db_path);

    let core_rest_url = get_core_rest_url();
    let core_ws_url = get_core_ws_url();
    info!(
        "🔗 Configuración: REST={}, WS={}",
        core_rest_url, core_ws_url
    );

    let client = reqwest::Client::new();

    // ==========================================
    // 4. VERIFICAR CANALES DEL CORE
    // ==========================================
    info!(" Verificando canales...");
    let actions_channel_id =
        get_or_create_channel(&client, &core_rest_url, LISTEN_CHANNEL_NAME).await?;
    let events_channel_id =
        get_or_create_channel(&client, &core_rest_url, RESPONSE_CHANNEL_NAME).await?;

    // ==========================================
    // 5. CONFIGURAR ROUTER DE AXUM
    // ==========================================

    // 🔓 Rutas PÚBLICAS (con protección de Rate Limiting)
    let public_routes = Router::new()
        .route("/auth/login", post(login))
        .route("/health", get(health_handler))
        .layer(GovernorLayer {
            config: governor_conf.clone(),
        })
        .with_state(auth_state.clone());

    // 🛡️ Rutas PROTEGIDAS (requieren JWT)
    let protected_routes = Router::new()
        .route("/exec", post(exec_handler))
        .route("/kill", post(kill_handler))
        .route("/files", post(files_handler))
        .route("/docker", post(docker_handler))
        .layer(middleware::from_fn_with_state(
            auth_state.clone(),
            require_auth,
        ))
        .with_state(db_conn.clone());

    // 🚀 Router principal: Anidamos TODO bajo el prefijo "/api"
    // Esto convierte /auth/login en /api/auth/login, /exec en /api/exec, etc.
    let app = Router::new()
        .nest("/api", public_routes)
        .nest("/api", protected_routes);

    // ==========================================
    // 6. INICIAR TAREAS EN BACKGROUND
    // ==========================================

    // TAREA: Loop de Dashboard (cada 5 segundos)
    // TAREA: Loop de Dashboard (cada 5 segundos)
    // TAREA: Loop de Dashboard (cada 5 segundos)
    // TAREA: Loop de Dashboard (cada 5 segundos)
    let events_channel_id_clone = events_channel_id.clone();
    let core_rest_url_clone = core_rest_url.clone();
    let db_conn_clone = db_conn.clone();

    tokio::spawn(async move {
        info!(
            "📊 Iniciando loop dinámico (cada {} segundos)...",
            DASHBOARD_INTERVAL_SECS
        );
        let mut interval = interval(Duration::from_secs(DASHBOARD_INTERVAL_SECS));
        let mut last_results: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();

        loop {
            interval.tick().await;

            for action in commands::DYNAMIC_ACTIONS {
                let (success, result) = commands::execute_action(action, &serde_json::Value::Null);

                let should_publish = match last_results.get(*action) {
                    Some(prev) => prev != &result,
                    None => true,
                };

                last_results.insert(action.to_string(), result.clone());

                if !should_publish {
                    info!("⏭️  '{}' sin cambios, omitiendo publicación", action);
                    continue;
                }

                if *action == "network_threats" && success {
                    if let Ok(threats_json) = serde_json::from_str::<serde_json::Value>(&result) {
                        if let Some(threats) = threats_json["threats"].as_array() {
                            let db = db_conn_clone.lock().await;
                            for threat in threats {
                                let record = db::ThreatRecord {
                                    ip: threat["ip"].as_str().unwrap_or("").to_string(),
                                    country: threat["country"].as_str().unwrap_or("XX").to_string(),
                                    connections: threat["connections"].as_u64().unwrap_or(0) as u32,
                                    ports: threat["ports"].as_str().unwrap_or("").to_string(),
                                    level: threat["level"].as_str().unwrap_or("SAFE").to_string(),
                                    timestamp: chrono::Utc::now(),
                                };
                                let _ = db::insert_threat(&db, &record);
                            }
                        }
                    }
                }

                let response_payload = json!({
                    "type": "dashboard",
                    "action": action,
                    "success": success,
                    "output": result,
                    "agent": AGENT_CLIENT_ID
                });

                if let Err(e) = publish_to_core(
                    &core_rest_url_clone,
                    &events_channel_id_clone,
                    response_payload,
                )
                .await
                {
                    error!("❌ Error publicando dashboard '{}': {}", action, e);
                }
            }
        }
    });

    // 👇 TAREA: Docker Info (SPAWN SEPARADO, no dentro del dashboard)
    let events_channel_id_clone_docker = events_channel_id.clone();
    let core_rest_url_clone_docker = core_rest_url.clone();

    tokio::spawn(async move {
        info!("🐳 Iniciando loop de Docker Info (cada 5s, async)...");
        let mut interval_docker = interval(Duration::from_secs(5)); // 👈 NOMBRE DIFERENTE
        let mut last_docker_result: Option<String> = None;

        loop {
            interval_docker.tick().await;

            let docker_result = tokio::task::spawn_blocking(|| {
                commands::execute_action("docker_info", &serde_json::Value::Null)
            })
            .await;

            if let Ok((success, result)) = docker_result {
                let should_publish = match &last_docker_result {
                    Some(prev) => prev != &result,
                    None => true,
                };

                last_docker_result = Some(result.clone());

                if !should_publish {
                    info!("⏭️  docker_info sin cambios");
                    continue;
                }

                let response_payload = json!({
                    "type": "dashboard",
                    "action": "docker_info",
                    "success": success,
                    "output": result,
                    "agent": AGENT_CLIENT_ID
                });

                if let Err(e) = publish_to_core(
                    &core_rest_url_clone_docker,
                    &events_channel_id_clone_docker,
                    response_payload,
                )
                .await
                {
                    error!("❌ Error publicando docker_info: {}", e);
                }
            }
        }
    });
    // TAREA: Nginx programado (cada 5 minutos)
    // let events_channel_id_clone_nginx = events_channel_id.clone();
    // let core_rest_url_clone_nginx = core_rest_url.clone();
    // tokio::spawn(async move {
    //     let mut interval = interval(Duration::from_secs(300));
    //     loop {
    //         interval.tick().await;
    //         info!("🔄 Ejecutando chequeo programado de Nginx...");
    //         let (success, result) =
    //             commands::execute_action("nginx_full", &serde_json::Value::Null);
    //         let _ = publish_to_core(
    //             &core_rest_url_clone_nginx,
    //             &events_channel_id_clone_nginx,
    //             json!({
    //                 "type": "dashboard",
    //                 "action": "nginx_full",
    //                 "success": success,
    //                 "output": result,
    //                 "agent": AGENT_CLIENT_ID
    //             }),
    //         )
    //         .await;
    //     }
    // });

    // TAREA: WebSocket Listener (comandos manuales)
    let events_channel_id_clone_ws = events_channel_id.clone();
    let core_rest_url_clone_ws = core_rest_url.clone();
    let db_conn_clone_ws = db_conn.clone();

    tokio::spawn(async move {
        info!("🔌 Conectando a {}...", core_ws_url);
        match connect_async(&core_ws_url).await {
            Ok((ws_stream, _)) => {
                info!("✅ Conectado al Core WebSocket");
                let (mut sender, mut receiver) = ws_stream.split();
                let init_msg =
                    json!({ "channel": actions_channel_id, "client_id": AGENT_CLIENT_ID });

                if let Err(e) = sender
                    .send(tokio_tungstenite::tungstenite::Message::Text(
                        init_msg.to_string(),
                    ))
                    .await
                {
                    error!(" Error suscribiendo: {}", e);
                    return;
                }
                info!("📩 Suscrito al canal: {}", actions_channel_id);

                while let Some(msg) = receiver.next().await {
                    match msg {
                        Ok(tokio_tungstenite::tungstenite::Message::Text(text)) => {
                            if let Ok(event) = serde_json::from_str::<Event>(&text) {
                                info!("📥 Evento recibido: {}", event.id);

                                let _ = process_manual_payload(
                                    &event.payload,
                                    &event.id,
                                    &events_channel_id_clone_ws,
                                    &core_rest_url_clone_ws,
                                    &db_conn_clone_ws,
                                )
                                .await;
                            }
                        }
                        Ok(tokio_tungstenite::tungstenite::Message::Close(_)) => {
                            info!("🔌 Core cerró la conexión.");
                            break;
                        }
                        Err(e) => {
                            error!("❌ Error en WebSocket: {}", e);
                            break;
                        }
                        _ => {}
                    }
                }
            }
            Err(e) => {
                error!("❌ No se pudo conectar al WebSocket: {}", e);
            }
        }
    });

    // ==========================================
    // 7. EJECUTAR SERVICIO HTTP
    // ==========================================
    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await?;
    info!("🌐 Servidor HTTP escuchando en http://0.0.0.0:8080");
    info!("🔓 Ruta pública: /auth/login (Rate Limit: 5 intentos/60s)");
    info!(" Rutas protegidas: /exec, /kill, /files, /docker");
    info!("✅ Sistema completamente inicializado");

    // CRÍTICO: into_make_service_with_connect_info es OBLIGATORIO para que
    // tower_governor pueda extraer la IP del cliente (PeerIpKeyExtractor).
    let make_svc = app.into_make_service_with_connect_info::<SocketAddr>();
    axum::serve(listener, make_svc).await?;

    Ok(())
}

// ============================================
// PROCESAMIENTO DE PAYLOADS (WebSocket)
// ============================================
async fn process_manual_payload(
    payload: &serde_json::Value,
    original_event_id: &str,
    events_channel_id: &str,
    core_rest_url: &str,
    db_conn: &DbConnection,
) -> Result<(), Box<dyn std::error::Error>> {
    let action = payload
        .get("action")
        .or_else(|| payload.get("cmd"))
        .and_then(|v| v.as_str());

    if let Some(cmd) = action {
        info!("⚙️ Ejecutando acción MANUAL: '{}'", cmd);

        match cmd {
            "set_admin_ip" => {
                if let Some(ip) = payload.get("ip").and_then(|v| v.as_str()) {
                    let db = db_conn.lock().await;
                    match db::add_to_whitelist(&db, ip) {
                        Ok(_) => {
                            let response = json!({
                                "type": "manual",
                                "action": cmd,
                                "success": true,
                                "output": format!("IP {} agregada a whitelist", ip),
                                "original_event_id": original_event_id,
                                "agent": AGENT_CLIENT_ID
                            });
                            publish_to_core(core_rest_url, events_channel_id, response).await?;
                        }
                        Err(e) => error!("Error agregando IP: {}", e),
                    }
                }
                return Ok(());
            }
            "get_top_attackers" => {
                let db = db_conn.lock().await;
                match db::get_top_attackers(&db, 20) {
                    Ok(attackers) => {
                        let response = json!({
                            "type": "manual",
                            "action": cmd,
                            "success": true,
                            "output": json!({ "attackers": attackers }),
                            "original_event_id": original_event_id,
                            "agent": AGENT_CLIENT_ID
                        });
                        publish_to_core(core_rest_url, events_channel_id, response).await?;
                    }
                    Err(e) => error!("Error obteniendo atacantes: {}", e),
                }
                return Ok(());
            }
            "clear_threats_db" => {
                let db = db_conn.lock().await;
                match db::clear_threats(&db) {
                    Ok(count) => {
                        let response = json!({
                            "type": "manual",
                            "action": cmd,
                            "success": true,
                            "output": format!("{} registros eliminados", count),
                            "original_event_id": original_event_id,
                            "agent": AGENT_CLIENT_ID
                        });
                        publish_to_core(core_rest_url, events_channel_id, response).await?;
                    }
                    Err(e) => error!("Error limpiando DB: {}", e),
                }
                return Ok(());
            }
            "get_threats_history" => {
                let db = db_conn.lock().await;
                match db::get_latest_threats(&db, 5) {
                    Ok(history) => {
                        let history_json: Vec<serde_json::Value> = history
                            .into_iter()
                            .map(|t| {
                                json!({
                                    "ip": t.ip,
                                    "country": t.country,
                                    "connections": t.connections,
                                    "ports": t.ports,
                                    "level": t.level,
                                    "timestamp": t.timestamp.to_rfc3339()
                                })
                            })
                            .collect();

                        let response = json!({
                            "type": "manual",
                            "action": cmd,
                            "success": true,
                            "output": json!({ "history": history_json }),
                            "original_event_id": original_event_id,
                            "agent": AGENT_CLIENT_ID
                        });
                        publish_to_core(core_rest_url, events_channel_id, response).await?;
                    }
                    Err(e) => error!("Error obteniendo historial: {}", e),
                }
                return Ok(());
            }
            _ => {}
        }

        let (success, result) = commands::execute_action(cmd, payload);
        let msg_type =
            if cmd == "os_info" || cmd == "ip_info" || cmd == "ports_info" || cmd == "nginx_full" {
                "dashboard"
            } else {
                "manual"
            };

        let response_payload = json!({
            "type": msg_type,
            "action": cmd,
            "success": success,
            "output": result,
            "original_event_id": original_event_id,
            "agent": AGENT_CLIENT_ID
        });
        publish_to_core(core_rest_url, events_channel_id, response_payload).await?;
    } else {
        warn!("️ Payload no contiene 'action' ni 'cmd'");
    }
    Ok(())
}
