//! Configuration module for the load-aware proxy
//! Handles loading and validating configuration from JSON file

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fs;
use tracing::warn;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Config {
    pub verbose: bool,
    pub http_port: u16,
    pub is_static: bool,
    pub enable_discovery: bool,
    pub discovery_interval_sec: u64,
    pub health_check_interval_sec: u64,
    pub health_failure_threshold: u32,
    pub unhealthy_removal_threshold: u32,
    pub request_retry_count: u32,
    pub ema_alpha: f64,
    pub request_timeout_ms: u64,
    pub expected_json_rpc_port: u16,
    pub static_nodes: Vec<StaticNode>,
    pub discoverable_nodes: Vec<DiscoverableNode>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct StaticNode {
    pub comet_rpc_url: String,
    pub json_rpc_url: String,
    #[serde(default)]
    pub is_dns: bool,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct DiscoverableNode {
    pub comet_rpc_url: String,
    #[serde(default)]
    pub is_dns: bool,
}

impl Config {
    /// Load configuration from JSON file
    pub fn load(path: &str) -> Result<Self> {
        let config_data = fs::read_to_string(path)
            .context(format!("Failed to read config file: {}", path))?;
        
        let config: Config = serde_json::from_str(&config_data)
            .context("Failed to parse config JSON")?;
        
        config.validate()?;
        
        Ok(config)
    }

    /// Validate configuration values
    fn validate(&self) -> Result<()> {
        // Validate port
        if self.http_port == 0 {
            bail!("http_port must be greater than 0");
        }

        // Validate intervals
        if self.enable_discovery && self.discovery_interval_sec == 0 {
            bail!("discovery_interval_sec must be greater than 0 when discovery is enabled");
        }

        if self.health_check_interval_sec == 0 {
            bail!("health_check_interval_sec must be greater than 0");
        }

        // Validate thresholds
        if self.health_failure_threshold == 0 {
            bail!("health_failure_threshold must be greater than 0");
        }

        if self.unhealthy_removal_threshold == 0 {
            bail!("unhealthy_removal_threshold must be greater than 0");
        }

        // Validate EMA alpha
        if self.ema_alpha < 0.0 || self.ema_alpha > 1.0 {
            bail!("ema_alpha must be between 0.0 and 1.0, got {}", self.ema_alpha);
        }

        // Validate timeout
        if self.request_timeout_ms == 0 {
            bail!("request_timeout_ms must be greater than 0");
        }

        // Validate JSON RPC port
        if self.expected_json_rpc_port == 0 {
            bail!("expected_json_rpc_port must be greater than 0");
        }

        // Ensure at least one node source
        if !self.is_static && !self.enable_discovery {
            bail!("At least one of is_static or enable_discovery must be true");
        }

        if self.is_static && self.static_nodes.is_empty() {
            bail!("is_static is true but static_nodes is empty");
        }

        if self.enable_discovery && self.discoverable_nodes.is_empty() {
            bail!("enable_discovery is true but discoverable_nodes is empty");
        }

        // Validate URLs
        for (idx, node) in self.static_nodes.iter().enumerate() {
            self.validate_url(&node.comet_rpc_url)
                .context(format!("Invalid comet_rpc_url in static_nodes[{}]", idx))?;
            self.validate_url(&node.json_rpc_url)
                .context(format!("Invalid json_rpc_url in static_nodes[{}]", idx))?;
            
            // For DNS nodes, ensure both URLs are explicitly provided and different domains are okay
            if node.is_dns {
                if node.comet_rpc_url.is_empty() || node.json_rpc_url.is_empty() {
                    bail!("DNS nodes (static_nodes[{}]) must have both comet_rpc_url and json_rpc_url explicitly specified", idx);
                }
            }
        }

        for (idx, node) in self.discoverable_nodes.iter().enumerate() {
            self.validate_url(&node.comet_rpc_url)
                .context(format!("Invalid comet_rpc_url in discoverable_nodes[{}]", idx))?;
            
            // DNS-based discoverable nodes cannot auto-discover JSON-RPC endpoints
            if node.is_dns {
                warn!("discoverable_nodes[{}] is marked as DNS. Discovery will only work for IP-based nodes. For DNS nodes, use static_nodes with explicit json_rpc_url.", idx);
            }
        }

        Ok(())
    }

    /// Validate URL format
    fn validate_url(&self, url: &str) -> Result<()> {
        if !url.starts_with("http://") && !url.starts_with("https://") {
            bail!("URL must start with http:// or https://, got: {}", url);
        }
        
        // Basic validation - just check it parses
        url::Url::parse(url)
            .context(format!("Invalid URL format: {}", url))?;
        
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_valid_config() -> Config {
        Config {
            verbose: true,
            http_port: 8080,
            is_static: true,
            enable_discovery: true,
            discovery_interval_sec: 300,
            health_check_interval_sec: 30,
            health_failure_threshold: 3,
            unhealthy_removal_threshold: 3,
            request_retry_count: 2,
            ema_alpha: 0.3,
            request_timeout_ms: 5000,
            expected_json_rpc_port: 8545,
            static_nodes: vec![StaticNode {
                comet_rpc_url: "http://node1.example.com:26657".to_string(),
                json_rpc_url: "http://node1.example.com:8545".to_string(),
                is_dns: true,
            }],
            discoverable_nodes: vec![DiscoverableNode {
                comet_rpc_url: "http://seed.example.com:26657".to_string(),
                is_dns: true,
            }],
        }
    }

    #[test]
    fn test_valid_config() {
        let config = create_valid_config();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_invalid_port() {
        let mut config = create_valid_config();
        config.http_port = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_invalid_ema_alpha() {
        let mut config = create_valid_config();
        config.ema_alpha = 1.5;
        assert!(config.validate().is_err());
        
        config.ema_alpha = -0.1;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_zero_intervals() {
        let mut config = create_valid_config();
        config.health_check_interval_sec = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_no_node_sources() {
        let mut config = create_valid_config();
        config.is_static = false;
        config.enable_discovery = false;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_empty_static_nodes() {
        let mut config = create_valid_config();
        config.is_static = true;
        config.static_nodes.clear();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_invalid_url() {
        let mut config = create_valid_config();
        config.static_nodes[0].comet_rpc_url = "invalid-url".to_string();
        assert!(config.validate().is_err());
    }
}
