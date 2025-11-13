//! Dashboard and monitoring endpoints

use crate::config::Config;
use crate::node::{count_healthy_nodes, count_total_nodes, get_all_nodes, NodePool, RpcType};
use axum::{extract::State, response::Html, Json};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Instant;

/// Application state
pub struct AppState {
    pub pool: NodePool,
    pub config: Arc<Config>,
    pub start_time: Instant,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub healthy_nodes: usize,
    pub total_nodes: usize,
    pub nodes: Vec<NodeHealthStatus>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NodeHealthStatus {
    pub name: String,
    pub is_healthy: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StatusResponse {
    pub nodes: Vec<NodeStatus>,
    pub total_comet_requests: u64,
    pub total_json_rpc_requests: u64,
    pub uptime_seconds: u64,
    pub config: ConfigSummary,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NodeStatus {
    pub name: String,
    pub node_type: String,
    pub is_dns: bool,
    pub comet_rpc_url: String,
    pub json_rpc_url: String,
    pub is_healthy: bool,
    pub comet_rpc_latency_ms: Option<f64>,
    pub json_rpc_latency_ms: Option<f64>,
    pub consecutive_failures: u32,
    pub times_marked_unhealthy: u32,
    pub request_count: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ConfigSummary {
    pub discovery_interval_sec: u64,
    pub health_check_interval_sec: u64,
    pub health_failure_threshold: u32,
    pub unhealthy_removal_threshold: u32,
    pub ema_alpha: f64,
}

/// Health check endpoint - returns simple health status
pub async fn health_handler(State(state): State<Arc<AppState>>) -> Json<HealthResponse> {
    let nodes = get_all_nodes(&state.pool).await;
    let healthy_count = count_healthy_nodes(&state.pool).await;
    let total_count = count_total_nodes(&state.pool).await;

    let node_statuses: Vec<NodeHealthStatus> = nodes
        .iter()
        .map(|node| NodeHealthStatus {
            name: node.name.clone(),
            is_healthy: node.is_healthy(),
        })
        .collect();

    Json(HealthResponse {
        status: if healthy_count > 0 { "ok".to_string() } else { "unhealthy".to_string() },
        healthy_nodes: healthy_count,
        total_nodes: total_count,
        nodes: node_statuses,
    })
}

/// Detailed status endpoint
pub async fn status_handler(State(state): State<Arc<AppState>>) -> Json<StatusResponse> {
    let nodes = get_all_nodes(&state.pool).await;
    let uptime = state.start_time.elapsed().as_secs();

    let mut node_statuses = Vec::new();
    let mut total_comet_requests = 0u64;
    let mut total_json_rpc_requests = 0u64;

    for node in nodes {
        let comet_latency = node.get_latency(RpcType::CometRpc).await;
        let json_rpc_latency = node.get_latency(RpcType::JsonRpc).await;
        let request_count = node.get_request_count();

        // Rough estimation: split requests between Comet and JSON-RPC
        total_comet_requests += request_count / 2;
        total_json_rpc_requests += request_count / 2;

        node_statuses.push(NodeStatus {
            name: node.name.clone(),
            node_type: node.node_type.as_str().to_string(),
            is_dns: node.is_dns,
            comet_rpc_url: node.comet_rpc_url.clone(),
            json_rpc_url: node.json_rpc_url.clone(),
            is_healthy: node.is_healthy(),
            comet_rpc_latency_ms: comet_latency,
            json_rpc_latency_ms: json_rpc_latency,
            consecutive_failures: node.get_consecutive_failures(),
            times_marked_unhealthy: node.get_times_marked_unhealthy(),
            request_count,
        });
    }

    Json(StatusResponse {
        nodes: node_statuses,
        total_comet_requests,
        total_json_rpc_requests,
        uptime_seconds: uptime,
        config: ConfigSummary {
            discovery_interval_sec: state.config.discovery_interval_sec,
            health_check_interval_sec: state.config.health_check_interval_sec,
            health_failure_threshold: state.config.health_failure_threshold,
            unhealthy_removal_threshold: state.config.unhealthy_removal_threshold,
            ema_alpha: state.config.ema_alpha,
        },
    })
}

/// Dashboard HTML page
pub async fn dashboard_handler() -> Html<&'static str> {
    Html(include_str!("dashboard.html"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::{add_node, create_node_pool, Node, NodeType};

    #[tokio::test]
    async fn test_health_response_structure() {
        let pool = create_node_pool();
        let node = Arc::new(Node::new(
            "test-node".to_string(),
            NodeType::Static,
            "http://test.com:26657".to_string(),
            "http://test.com:8545".to_string(),
            false,
        ));
        add_node(&pool, node).await;

        let config = Arc::new(Config {
            verbose: false,
            http_port: 8080,
            is_static: true,
            enable_discovery: false,
            discovery_interval_sec: 60,
            health_check_interval_sec: 30,
            health_failure_threshold: 3,
            unhealthy_removal_threshold: 3,
            request_retry_count: 2,
            ema_alpha: 0.3,
            request_timeout_ms: 5000,
            expected_json_rpc_port: 8545,
            static_nodes: vec![],
            discoverable_nodes: vec![],
        });

        let state = Arc::new(AppState {
            pool,
            config,
            start_time: Instant::now(),
        });

        let response = health_handler(State(state)).await;
        
        assert_eq!(response.0.total_nodes, 1);
        assert_eq!(response.0.healthy_nodes, 1);
        assert_eq!(response.0.nodes.len(), 1);
    }
}

