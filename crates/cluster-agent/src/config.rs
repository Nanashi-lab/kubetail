// Copyright 2024-2025 Andres Morey
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Main configuration structure for the Kubetail Cluster Agent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub grpc: GrpcConfig,
    pub file_watcher: FileWatcherConfig,
    pub kubernetes: KubernetesConfig,
    pub logging: LoggingConfig,
    #[serde(rename = "allowed-namespaces")]
    pub allowed_namespaces: Vec<String>,
    #[serde(rename = "container-logs-dir")]
    pub container_logs_dir: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrpcConfig {
    pub address: String,
    pub tls: TlsConfig,
    pub health_check: bool,
    pub reflection: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsConfig {
    pub enabled: bool,
    #[serde(rename = "cert-file")]
    pub cert_file: String,
    #[serde(rename = "key-file")]
    pub key_file: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileWatcherConfig {
    #[serde(rename = "debounce-ms")]
    pub debounce_ms: u64,
    #[serde(rename = "buffer-size")]
    pub buffer_size: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KubernetesConfig {
    #[serde(rename = "in-cluster")]
    pub in_cluster: bool,
    #[serde(rename = "kubeconfig-path")]
    pub kubeconfig_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    pub enabled: bool,
    pub level: String,
    pub format: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            grpc: GrpcConfig {
                address: ":50051".to_string(),
                tls: TlsConfig {
                    enabled: false,
                    cert_file: "".to_string(),
                    key_file: "".to_string(),
                },
                health_check: true,
                reflection: false,
            },
            file_watcher: FileWatcherConfig {
                debounce_ms: 2000, // 2 seconds, matching Go implementation
                buffer_size: 1000,
            },
            kubernetes: KubernetesConfig {
                in_cluster: true,
                kubeconfig_path: None,
            },
            logging: LoggingConfig {
                enabled: true,
                level: "info".to_string(),
                format: "json".to_string(),
            },
            allowed_namespaces: vec![],
            container_logs_dir: "/var/log/containers".to_string(),
        }
    }
}

impl Config {
    /// Load configuration from file, environment variables, and CLI parameters
    /// This replaces the Go Viper configuration system
    pub fn load(
        config_file: Option<&str>,
        params: &[String],
        addr_override: &str,
    ) -> Result<Self> {
        let mut config = Config::default();

        // Override default address if provided
        if addr_override != ":50051" {
            config.grpc.address = addr_override.to_string();
        }

        // Load from config file if provided
        if let Some(config_path) = config_file {
            config = Self::load_from_file(config_path)
                .with_context(|| format!("Failed to load config from {}", config_path))?;
        }

        // Apply CLI parameter overrides (equivalent to Go's --param functionality)
        for param in params {
            Self::apply_param(&mut config, param)
                .with_context(|| format!("Failed to apply parameter: {}", param))?;
        }

        // Apply environment variable overrides
        Self::apply_env_vars(&mut config)?;

        Ok(config)
    }

    /// Load configuration from a YAML file
    fn load_from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let content = std::fs::read_to_string(path.as_ref())
            .with_context(|| format!("Failed to read config file: {:?}", path.as_ref()))?;

        serde_yaml::from_str(&content)
            .with_context(|| "Failed to parse YAML configuration")
    }

    /// Apply a single parameter in "key:value" format
    fn apply_param(config: &mut Config, param: &str) -> Result<()> {
        let parts: Vec<&str> = param.splitn(2, ':').collect();
        if parts.len() != 2 {
            return Err(anyhow::anyhow!("Invalid parameter format. Expected 'key:value', got '{}'", param));
        }

        let key = parts[0];
        let value = parts[1];

        // Map CLI parameters to config fields (matching Go implementation)
        match key {
            "cluster-agent.addr" => config.grpc.address = value.to_string(),
            "cluster-agent.logging.enabled" => {
                config.logging.enabled = value.parse()
                    .with_context(|| "Invalid boolean value for logging.enabled")?;
            }
            "cluster-agent.logging.level" => config.logging.level = value.to_string(),
            "cluster-agent.logging.format" => config.logging.format = value.to_string(),
            "cluster-agent.tls.enabled" => {
                config.grpc.tls.enabled = value.parse()
                    .with_context(|| "Invalid boolean value for tls.enabled")?;
            }
            "cluster-agent.tls.cert-file" => config.grpc.tls.cert_file = value.to_string(),
            "cluster-agent.tls.key-file" => config.grpc.tls.key_file = value.to_string(),
            "container-logs-dir" => config.container_logs_dir = value.to_string(),
            _ => return Err(anyhow::anyhow!("Unknown configuration parameter: {}", key)),
        }

        Ok(())
    }

    /// Apply environment variable overrides
    fn apply_env_vars(config: &mut Config) -> Result<()> {
        // Check for common environment variables
        if let Ok(addr) = std::env::var("CLUSTER_AGENT_ADDR") {
            config.grpc.address = addr;
        }

        if let Ok(logs_dir) = std::env::var("CONTAINER_LOGS_DIR") {
            config.container_logs_dir = logs_dir;
        }

        if let Ok(node_name) = std::env::var("NODE_NAME") {
            // Store node name for metadata service (matching Go implementation)
            // This will be used by the gRPC services
        }

        Ok(())
    }

    /// Validate the configuration
    pub fn validate(&self) -> Result<()> {
        // Validate gRPC address format
        if self.grpc.address.is_empty() {
            return Err(anyhow::anyhow!("gRPC address cannot be empty"));
        }

        // Validate TLS configuration
        if self.grpc.tls.enabled {
            if self.grpc.tls.cert_file.is_empty() || self.grpc.tls.key_file.is_empty() {
                return Err(anyhow::anyhow!("TLS cert-file and key-file must be specified when TLS is enabled"));
            }
        }

        // Validate container logs directory
        if !Path::new(&self.container_logs_dir).exists() {
            return Err(anyhow::anyhow!("Container logs directory does not exist: {}", self.container_logs_dir));
        }

        // Validate logging level
        match self.logging.level.to_lowercase().as_str() {
            "trace" | "debug" | "info" | "warn" | "error" => {}
            _ => return Err(anyhow::anyhow!("Invalid logging level: {}", self.logging.level)),
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::NamedTempFile;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.grpc.address, ":50051");
        assert_eq!(config.container_logs_dir, "/var/log/containers");
        assert!(!config.grpc.tls.enabled);
        assert!(config.logging.enabled);
    }

    #[test]
    fn test_load_from_yaml() {
        let yaml_content = r#"
grpc:
  address: ":8080"
  tls:
    enabled: true
    cert-file: "/path/to/cert"
    key-file: "/path/to/key"
logging:
  level: "debug"
container-logs-dir: "/custom/logs"
"#;

        let mut temp_file = NamedTempFile::new().unwrap();
        fs::write(&temp_file, yaml_content).unwrap();

        let config = Config::load_from_file(temp_file.path()).unwrap();
        assert_eq!(config.grpc.address, ":8080");
        assert!(config.grpc.tls.enabled);
        assert_eq!(config.logging.level, "debug");
        assert_eq!(config.container_logs_dir, "/custom/logs");
    }

    #[test]
    fn test_apply_param() {
        let mut config = Config::default();
        
        Config::apply_param(&mut config, "cluster-agent.addr::9090").unwrap();
        assert_eq!(config.grpc.address, ":9090");

        Config::apply_param(&mut config, "cluster-agent.logging.level:warn").unwrap();
        assert_eq!(config.logging.level, "warn");
    }

    #[test]
    fn test_invalid_param_format() {
        let mut config = Config::default();
        let result = Config::apply_param(&mut config, "invalid-format");
        assert!(result.is_err());
    }
} 