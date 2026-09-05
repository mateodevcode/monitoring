use serde::Deserialize;
use std::collections::HashMap;
use std::fs;

#[derive(Debug, Deserialize, Clone)]
pub struct VpsEntry {
    pub id: String,
    pub host: String,
    pub user: String,
    pub ssh_key_path: String,
    #[serde(default = "default_port")]
    pub port: u16,
    /// Mapa: nombre amigable del comando -> comando real de shell.
    /// El LLM solo ve los nombres amigables, nunca el comando real.
    pub commands: HashMap<String, String>,
}

fn default_port() -> u16 {
    22
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct VpsConfig {
    #[serde(rename = "vps", default)]
    pub entries: Vec<VpsEntry>,
}

impl VpsConfig {
    pub fn load(path: &str) -> Result<Self, String> {
        let content =
            fs::read_to_string(path).map_err(|e| format!("No se pudo leer {}: {}", path, e))?;
        toml::from_str(&content).map_err(|e| format!("Error parseando {}: {}", path, e))
    }

    pub fn find(&self, id: &str) -> Option<&VpsEntry> {
        self.entries.iter().find(|v| v.id == id)
    }

    pub fn vps_ids(&self) -> Vec<String> {
        self.entries.iter().map(|v| v.id.clone()).collect()
    }
}

impl VpsEntry {
    pub fn command_names(&self) -> Vec<String> {
        self.commands.keys().cloned().collect()
    }
}
