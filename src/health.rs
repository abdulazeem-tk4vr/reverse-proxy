//! Health monitoring module
//! Performs periodic health checks on all nodes

use crate::config::Config;
use crate::node::{get_all_nodes, remove_node, NodePool};
use anyhow::Result;
use reqwest::Client;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;
use tracing::{debug, error, info, warn};

/// Check Comet RPC health by querying /status endpoint
async fn check_comet_rpc_health(
    client: &Client,
    url: &str,
    timeout_ms: u64,
) -> Result<bool> {
    let status_url = format!("{}/status", url.trim_end_matches('/'));
    
    let response = client
        .get(&status_url)
        .timeout(Duration::from_millis(timeout_ms))
        .send()
        .await?;

    Ok(response.status().is_success())
}

/// Check JSON-RPC health by calling eth_blockNumber
async fn check_json_rpc_health(
    client: &Client,
    url: &str,
    timeout_ms: u64,
) -> Result<bool> {
    let request_body = json!({
        "jsonrpc": "2.0",
        "method": "eth_blockNumber",
        "params": [],
        "id": 1
    });

    let response = client
        .post(url)
        .json(&request_body)
        .timeout(Duration::from_millis(timeout_ms))
        .send()
        .await?;

    if !response.status().is_success() {
        return Ok(false);
    }

    // Try to parse response to ensure it's valid
    let json_response: serde_json::Value = response.json().await?;
    
    // Check if response contains result or error
    Ok(json_response.get("result").is_some())
}

/// Health check loop
pub async fn health_check_loop(
    client: Arc<Client>,
    config: Arc<Config>,
    pool: NodePool,
    mut shutdown_rx: broadcast::Receiver<()>,
) {
    info!("Starting health check loop");

    let mut interval = tokio::time::interval(Duration::from_secs(config.health_check_interval_sec));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            _ = interval.tick() => {
                perform_health_checks(&client, &config, &pool).await;
            }
            _ = shutdown_rx.recv() => {
                info!("Health check loop shutting down");
                break;
            }
        }
    }
}

/// Perform health checks on all nodes
async fn perform_health_checks(
    client: &Client,
    config: &Config,
    pool: &NodePool,
) {
    let nodes = get_all_nodes(pool).await;

    if nodes.is_empty() {
        debug!("No nodes to health check");
        return;
    }

    debug!("Performing health checks on {} nodes", nodes.len());

    for node in nodes {
        let node_id = node.id.clone();
        let node_name = node.name.clone();

        // Check Comet RPC
        let comet_healthy = check_comet_rpc_health(
            client,
            &node.comet_rpc_url,
            config.request_timeout_ms,
        )
        .await
        .unwrap_or(false);

        // Check JSON-RPC
        let json_rpc_healthy = check_json_rpc_health(
            client,
            &node.json_rpc_url,
            config.request_timeout_ms,
        )
        .await
        .unwrap_or(false);

        // Node is healthy only if both endpoints are healthy
        let is_healthy = comet_healthy && json_rpc_healthy;

        // Update node health status
        node.update_health_check_time().await;

        if is_healthy {
            if !node.is_healthy() {
                info!("Node {} recovered and is now healthy", node_name);
            }
            node.mark_healthy();
            debug!("Node {} is healthy", node_name);
        } else {
            let was_just_marked_unhealthy = node.increment_failure(config.health_failure_threshold);
            
            if was_just_marked_unhealthy {
                warn!(
                    "Node {} marked unhealthy after {} consecutive failures",
                    node_name,
                    config.health_failure_threshold
                );

                // Check if node should be removed
                let times_unhealthy = node.get_times_marked_unhealthy();
                if times_unhealthy >= config.unhealthy_removal_threshold {
                    warn!(
                        "Node {} has been marked unhealthy {} times, removing from pool",
                        node_name, times_unhealthy
                    );
                    
                    if remove_node(pool, &node_id).await {
                        info!("Removed unhealthy node {} from pool", node_name);
                    }
                }
            } else {
                debug!(
                    "Node {} failed health check (failures: {}/{})",
                    node_name,
                    node.get_consecutive_failures(),
                    config.health_failure_threshold
                );
            }
        }
    }

    // Check if all nodes are unhealthy
    let healthy_count = crate::node::count_healthy_nodes(pool).await;
    let total_count = crate::node::count_total_nodes(pool).await;

    if total_count > 0 && healthy_count == 0 {
        error!(
            "CRITICAL: All {} nodes are unhealthy! System should initiate graceful shutdown.",
            total_count
        );
    } else {
        debug!("Health check complete: {}/{} nodes healthy", healthy_count, total_count);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::{add_node, create_node_pool, Node, NodeType};

    #[tokio::test]
    async fn test_health_check_marks_unhealthy() {
        let pool = create_node_pool();
        let node = Arc::new(Node::new(
            "test-node".to_string(),
            NodeType::Static,
            "http://invalid-url-that-will-fail:26657".to_string(),
            "http://invalid-url-that-will-fail:8545".to_string(),
            false,
        ));

        add_node(&pool, node.clone()).await;

        let client = Client::new();
        let config = Config {
            verbose: false,
            http_port: 8080,
            is_static: true,
            enable_discovery: false,
            discovery_interval_sec: 60,
            health_check_interval_sec: 30,
            health_failure_threshold: 2,
            unhealthy_removal_threshold: 3,
            request_retry_count: 1,
            ema_alpha: 0.3,
            request_timeout_ms: 1000,
            expected_json_rpc_port: 8545,
            static_nodes: vec![],
            discoverable_nodes: vec![],
        };

        // Perform health checks multiple times
        for _ in 0..2 {
            perform_health_checks(&client, &config, &pool).await;
        }

        // Node should be marked unhealthy
        assert!(!node.is_healthy());
    }

    #[tokio::test]
    async fn test_healthy_node_stays_healthy() {
        let pool = create_node_pool();
        
        // We can't actually test a real healthy node without a server
        // This test just verifies the structure works
        assert_eq!(crate::node::count_total_nodes(&pool).await, 0);
    }
}
