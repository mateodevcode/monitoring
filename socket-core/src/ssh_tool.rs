use crate::tool_registry::{Tool, ToolError};
use crate::vps_config::VpsConfig;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;
use tracing::{info, warn};

/// Una sola tool genérica para TODOS los VPS registrados — no una tool por VPS.
/// El LLM elige vps_id + command_name; el comando real de shell nunca lo ve ni lo decide.
pub struct SshCommandTool {
    config: VpsConfig,
}

impl SshCommandTool {
    pub fn new(config: VpsConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Tool for SshCommandTool {
    fn name(&self) -> &str {
        "run_vps_command"
    }

    fn description(&self) -> &str {
        "Ejecuta un comando predefinido y seguro en uno de los VPS registrados (por ejemplo, \
         consultar RAM, espacio en disco o uptime). Solo se pueden usar los vps_id y \
         command_name que ya existen en la configuración; no se pueden ejecutar comandos \
         arbitrarios ni inventados."
    }

    fn parameters_schema(&self) -> Value {
        let vps_ids = self.config.vps_ids();
        json!({
            "type": "object",
            "properties": {
                "vps_id": {
                    "type": "string",
                    "description": "Identificador del VPS registrado sobre el que ejecutar el comando",
                    "enum": vps_ids
                },
                "command_name": {
                    "type": "string",
                    "description": "Nombre del comando predefinido a ejecutar (ej. 'ram', 'disk', 'uptime'). Debe existir en la configuración de ese VPS."
                }
            },
            "required": ["vps_id", "command_name"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String, ToolError> {
        let vps_id = args
            .get("vps_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError("Falta el parámetro 'vps_id'".into()))?;
        let command_name = args
            .get("command_name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError("Falta el parámetro 'command_name'".into()))?;

        let vps = self
            .config
            .find(vps_id)
            .ok_or_else(|| ToolError(format!("El VPS '{}' no está registrado", vps_id)))?;

        let command = vps.commands.get(command_name).ok_or_else(|| {
            ToolError(format!(
                "El comando '{}' no está permitido en '{}'. Comandos disponibles: {:?}",
                command_name,
                vps_id,
                vps.command_names()
            ))
        })?;

        info!(
            "🔌 SSH → {}@{}:{} :: {} ({})",
            vps.user, vps.host, vps.port, command_name, command
        );

        let target = format!("{}@{}", vps.user, vps.host);
        let port_str = vps.port.to_string();

        let result = timeout(
            Duration::from_secs(15),
            Command::new("ssh")
                .args([
                    "-i",
                    &vps.ssh_key_path,
                    "-p",
                    &port_str,
                    "-o",
                    "BatchMode=yes",
                    "-o",
                    "StrictHostKeyChecking=accept-new",
                    "-o",
                    "ConnectTimeout=8",
                    &target,
                    command,
                ])
                .output(),
        )
        .await
        .map_err(|_| ToolError(format!("Timeout conectando a '{}'", vps_id)))?
        .map_err(|e| ToolError(format!("Error ejecutando ssh: {}", e)))?;

        if !result.status.success() {
            let stderr = String::from_utf8_lossy(&result.stderr);
            warn!("⚠️ Comando SSH falló en {}: {}", vps_id, stderr);
            return Err(ToolError(format!(
                "El comando falló en '{}': {}",
                vps_id,
                stderr.trim()
            )));
        }

        let stdout = String::from_utf8_lossy(&result.stdout).trim().to_string();
        Ok(stdout)
    }
}
