use serde::Deserialize;
use std::path::Path;

use crate::protocol::DEFAULT_MAX_PAYLOAD_SIZE;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub websocket: WebSocketConfig,
    pub dds: DdsConfig,
    pub bridge: BridgeConfig,
    pub logging: LoggingConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct WebSocketConfig {
    pub addr: String,
    pub port: u16,
    pub max_connections: usize,
    pub idle_timeout_secs: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct DdsConfig {
    pub domain_id: u32,
    pub cyclonedds_uri: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct BridgeConfig {
    pub max_payload_size: usize,
    pub session_buffer_size: usize,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct LoggingConfig {
    pub level: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            websocket: WebSocketConfig::default(),
            dds: DdsConfig::default(),
            bridge: BridgeConfig::default(),
            logging: LoggingConfig::default(),
        }
    }
}

impl Default for WebSocketConfig {
    fn default() -> Self {
        Self {
            addr: "127.0.0.1".to_string(),
            port: 9876,
            max_connections: 64,
            idle_timeout_secs: 30,
        }
    }
}

impl Default for DdsConfig {
    fn default() -> Self {
        Self {
            domain_id: 0,
            cyclonedds_uri: None,
        }
    }
}

impl Default for BridgeConfig {
    fn default() -> Self {
        Self {
            max_payload_size: DEFAULT_MAX_PAYLOAD_SIZE,
            session_buffer_size: 256,
        }
    }
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let contents = std::fs::read_to_string(path)
            .map_err(|e| ConfigError::FileRead(path.display().to_string(), e))?;
        let mut config: Config =
            toml::from_str(&contents).map_err(|e| ConfigError::Parse(e.to_string()))?;
        config.apply_env_overrides()?;
        Ok(config)
    }

    pub fn from_defaults() -> Result<Self, ConfigError> {
        let mut config = Self::default();
        config.apply_env_overrides()?;
        Ok(config)
    }

    fn apply_env_overrides(&mut self) -> Result<(), ConfigError> {
        if let Ok(val) = std::env::var("DDS_BRIDGE_WS_ADDR") {
            self.websocket.addr = val;
        }
        if let Ok(val) = std::env::var("DDS_BRIDGE_WS_PORT") {
            self.websocket.port = val
                .parse()
                .map_err(|_| ConfigError::InvalidEnvVar("DDS_BRIDGE_WS_PORT".into(), val))?;
        }
        if let Ok(val) = std::env::var("DDS_BRIDGE_DDS_DOMAIN") {
            self.dds.domain_id = val
                .parse()
                .map_err(|_| ConfigError::InvalidEnvVar("DDS_BRIDGE_DDS_DOMAIN".into(), val))?;
        }
        if let Ok(val) = std::env::var("CYCLONEDDS_URI") {
            self.dds.cyclonedds_uri = Some(val);
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to read config file '{0}': {1}")]
    FileRead(String, std::io::Error),
    #[error("failed to parse config: {0}")]
    Parse(String),
    #[error("invalid value for environment variable {0}: '{1}'")]
    InvalidEnvVar(String, String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_defaults() {
        let config = Config::default();
        assert_eq!(config.websocket.addr, "127.0.0.1");
        assert_eq!(config.websocket.port, 9876);
        assert_eq!(config.dds.domain_id, 0);
        assert_eq!(config.bridge.max_payload_size, 4 * 1024 * 1024);
        assert_eq!(config.bridge.session_buffer_size, 256);
    }

    #[test]
    fn test_parse_toml() {
        let toml_str = r#"
[websocket]
addr = "0.0.0.0"
port = 8080

[dds]
domain_id = 42

[bridge]
max_payload_size = 1048576
session_buffer_size = 128

[logging]
level = "debug"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.websocket.addr, "0.0.0.0");
        assert_eq!(config.websocket.port, 8080);
        assert_eq!(config.dds.domain_id, 42);
        assert_eq!(config.bridge.max_payload_size, 1048576);
        assert_eq!(config.logging.level, "debug");
    }

    #[test]
    fn test_partial_toml_uses_defaults() {
        let toml_str = r#"
[websocket]
port = 1234
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.websocket.port, 1234);
        assert_eq!(config.websocket.addr, "127.0.0.1");
        assert_eq!(config.dds.domain_id, 0);
    }

    #[test]
    fn test_empty_toml_uses_defaults() {
        let config: Config = toml::from_str("").unwrap();
        assert_eq!(config.websocket.port, 9876);
        assert_eq!(config.bridge.max_payload_size, DEFAULT_MAX_PAYLOAD_SIZE);
    }

    #[test]
    fn test_env_override_addr() {
        // Use a unique env var prefix to avoid test interference
        std::env::set_var("DDS_BRIDGE_WS_ADDR", "0.0.0.0");
        let config = Config::from_defaults().unwrap();
        assert_eq!(config.websocket.addr, "0.0.0.0");
        std::env::remove_var("DDS_BRIDGE_WS_ADDR");
    }

    #[test]
    fn test_env_override_invalid_port() {
        std::env::set_var("DDS_BRIDGE_WS_PORT", "banana");
        let err = Config::from_defaults().unwrap_err();
        assert!(matches!(err, ConfigError::InvalidEnvVar(_, _)));
        std::env::remove_var("DDS_BRIDGE_WS_PORT");
    }

    #[test]
    fn test_env_override_invalid_domain() {
        std::env::set_var("DDS_BRIDGE_DDS_DOMAIN", "not_a_number");
        let err = Config::from_defaults().unwrap_err();
        assert!(matches!(err, ConfigError::InvalidEnvVar(_, _)));
        std::env::remove_var("DDS_BRIDGE_DDS_DOMAIN");
    }
}
