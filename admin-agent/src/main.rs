mod commands;
mod db; // Asegúrate de que este archivo exista en src/db.rs

use futures_util::{SinkExt, StreamExt};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::env;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{interval, Duration};
use tokio_tungstenite::connect_async;
use tracing::{error, info, warn};

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

const AGENT_CLIENT_ID: &str = "rust-admin-agent-01";
const LISTEN_CHANNEL_NAME: &str = "admin-actions";
const RESPONSE_CHANNEL_NAME: &str = "admin-events";
const DASHBOARD_INTERVAL_SECS: u64 = 5;

fn get_core_ws_url() -> String {
    env::var("CORE_WS_URL").unwrap_or_else(|_| "ws://localhost:3005/ws".to_string())
}

fn get_core_rest_url() -> String {
    env::var("CORE_REST_URL").unwrap_or_else(|_| "http://localhost:3005".to_string())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();
    info!("🚀 Iniciando Rust Admin Agent...");

    // ✅ CORREGIDO 1: Inicializar la base de datos ANTES de usarla
    let db_path = env::var("DB_PATH").unwrap_or_else(|_| "threats.db".to_string());
    let conn = db::init_db(&db_path).expect("❌ Error fatal al inicializar la base de datos");
    let db_conn = Arc::new(Mutex::new(conn));
    info!("💾 Base de datos SQLite inicializada en: {}", db_path);

    let core_rest_url = get_core_rest_url();
    let core_ws_url = get_core_ws_url();
    info!(
        "🔗 Configuración: REST={}, WS={}",
        core_rest_url, core_ws_url
    );

    let client = reqwest::Client::new();

    info!("🔍 Verificando canales necesarios...");
    let actions_channel_id =
        get_or_create_channel(&client, &core_rest_url, LISTEN_CHANNEL_NAME).await?;
    let events_channel_id =
        get_or_create_channel(&client, &core_rest_url, RESPONSE_CHANNEL_NAME).await?;

    // ==========================================
    // TAREA 1: Loop de Dashboard DINÁMICO (cada 5 segundos)
    // ==========================================
    let events_channel_id_clone = events_channel_id.clone();
    let core_rest_url_clone = core_rest_url.clone();
    let db_conn_clone = db_conn.clone(); // ✅ Ahora sí existe y se puede clonar

    tokio::spawn(async move {
        info!(
            "📊 Iniciando loop dinámico (cada {} segundos)...",
            DASHBOARD_INTERVAL_SECS
        );
        let mut interval = interval(Duration::from_secs(DASHBOARD_INTERVAL_SECS));

        loop {
            interval.tick().await;
            for action in commands::DYNAMIC_ACTIONS {
                let (success, result) = commands::execute_action(action, &serde_json::Value::Null);

                // Si es network_threats, guardar en DB
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

    // ==========================================
    // TAREA 2: Conexión WebSocket
    // ==========================================
    info!("🔌 Conectando a {}...", core_ws_url);
    let (ws_stream, _) = connect_async(&core_ws_url).await?;
    info!("✅ Conectado al Core WebSocket");

    let (mut sender, mut receiver) = ws_stream.split();
    let init_msg = json!({ "channel": actions_channel_id, "client_id": AGENT_CLIENT_ID });

    sender
        .send(tokio_tungstenite::tungstenite::Message::Text(
            init_msg.to_string(),
        ))
        .await?;
    info!(
        "📩 Suscrito exitosamente al canal ID: '{}'",
        actions_channel_id
    );

    // ==========================================
    // 📌 TAREA 2.5: Ejecutar Nginx INMEDIATAMENTE al conectar
    // ==========================================
    info!("📌 Ejecutando chequeo inicial de Nginx (INMEDIATO)...");
    let (success, result) = commands::execute_action("nginx_full", &serde_json::Value::Null);
    let _ = publish_to_core(
        &core_rest_url,
        &events_channel_id,
        json!({
            "type": "dashboard",
            "action": "nginx_full",
            "success": success,
            "output": result,
            "agent": AGENT_CLIENT_ID
        }),
    )
    .await;

    // Tarea dedicada para Nginx cada 5 minutos (300 segundos)
    let events_channel_id_clone_nginx = events_channel_id.clone();
    let core_rest_url_clone_nginx = core_rest_url.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(300));
        loop {
            interval.tick().await;
            info!("🔄 Ejecutando chequeo programado de Nginx...");
            let (success, result) =
                commands::execute_action("nginx_full", &serde_json::Value::Null);
            let _ = publish_to_core(
                &core_rest_url_clone_nginx,
                &events_channel_id_clone_nginx,
                json!({
                    "type": "dashboard",
                    "action": "nginx_full",
                    "success": success,
                    "output": result,
                    "agent": AGENT_CLIENT_ID
                }),
            )
            .await;
        }
    });

    // ==========================================
    // TAREA 3: Loop de Comandos Manuales (WebSocket listener)
    // ==========================================
    while let Some(msg) = receiver.next().await {
        match msg {
            Ok(tokio_tungstenite::tungstenite::Message::Text(text)) => {
                if let Ok(event) = serde_json::from_str::<Event>(&text) {
                    info!(
                        "📥 Evento recibido del canal: {} (source: {})",
                        event.channel, event.source
                    );

                    // ✅ CORREGIDO 2: Pasamos &db_conn como argumento
                    process_manual_payload(
                        &event.payload,
                        &event.id,
                        &events_channel_id,
                        &core_rest_url,
                        &db_conn,
                    )
                    .await?;
                } else {
                    warn!("⚠️ Mensaje no estándar recibido: {}", text);
                }
            }
            Ok(tokio_tungstenite::tungstenite::Message::Close(_)) => {
                info!("🔌 El Core cerró la conexión.");
                break;
            }
            Err(e) => {
                error!("❌ Error en el WebSocket: {}", e);
                break;
            }
            _ => {}
        }
    }
    Ok(())
}

// --- FUNCIONES AUXILIARES ---
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
    info!(
        "✅ Canal '{}' creado exitosamente con ID: {}",
        channel_name, new_id
    );
    Ok(new_id)
}

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

        // 1. COMANDO ESPECIAL: Marcar IP propia (NUEVO)
        if cmd == "set_admin_ip" {
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
                    Err(e) => error!("Error agregando IP a whitelist: {}", e),
                }
            }
            return Ok(());
        }

        // 2. COMANDO ESPECIAL: Obtener Top Atacantes (NUEVO)
        if cmd == "get_top_attackers" {
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
                Err(e) => error!("Error obteniendo top atacantes: {}", e),
            }
            return Ok(());
        }

        // 3. COMANDO: Limpiar DB (EXISTENTE)
        if cmd == "clear_threats_db" {
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

        // 4. COMANDO: Obtener historial (EXISTENTE)
        if cmd == "get_threats_history" {
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

        // 5. COMANDOS DINÁMICOS GENERALES (EXISTENTE)
        let (success, result) = commands::execute_action(cmd, payload);

        let msg_type =
            if cmd == "os_info" || cmd == "ip_info" || cmd == "ports_info" || cmd == "nginx_full" {
                "dashboard"
            } else {
                "manual"
            };

        let response_payload = json!({
            "type": msg_type, "action": cmd, "success": success, "output": result,
            "original_event_id": original_event_id, "agent": AGENT_CLIENT_ID
        });
        publish_to_core(core_rest_url, events_channel_id, response_payload).await?;
    } else {
        warn!("⚠️ Payload no contiene 'action' ni 'cmd'. Ignorando.");
    }
    Ok(())
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
