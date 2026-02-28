use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::config::config_dir;

/// Manifest describing a plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub name: String,
    pub command: String,
    #[serde(default = "default_task_type")]
    pub task_type: String,
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
}

fn default_task_type() -> String {
    "plugin".into()
}

fn default_timeout() -> u64 {
    600_000
}

/// Discover all plugins from `~/.config/architect/plugins/*/plugin.toml`.
pub fn discover_plugins() -> HashMap<String, PluginManifest> {
    let mut plugins = HashMap::new();
    let plugins_dir = plugins_dir();

    if !plugins_dir.exists() {
        return plugins;
    }

    let entries = match std::fs::read_dir(&plugins_dir) {
        Ok(e) => e,
        Err(_) => return plugins,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let manifest_path = path.join("plugin.toml");
        if !manifest_path.exists() {
            continue;
        }
        match load_manifest(&manifest_path) {
            Ok(m) => {
                debug!("Loaded plugin '{}' from {:?}", m.name, manifest_path);
                plugins.insert(m.name.clone(), m);
            }
            Err(e) => {
                warn!("Failed to load plugin manifest {:?}: {}", manifest_path, e);
            }
        }
    }

    plugins
}

fn load_manifest(path: &PathBuf) -> anyhow::Result<PluginManifest> {
    let content = std::fs::read_to_string(path)?;
    let manifest: PluginManifest = toml::from_str(&content)?;
    Ok(manifest)
}

/// Directory where plugins are stored.
pub fn plugins_dir() -> PathBuf {
    config_dir().join("plugins")
}
