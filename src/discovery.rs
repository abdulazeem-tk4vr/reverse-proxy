//! Node discovery module
//! Handles recursive discovery of nodes via Comet RPC net_info endpoint

use crate::balancer::LoadBalancer;
use crate::config::Config;
use crate::node::{add_node, node_exists, Node, NodePool, NodeType};
use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{debug, error, info, warn};

#[derive(Debug, Deserialize, Serialize)]
struct NetInfoResponse {
    pub result: NetInfoResult,
}

#[derive(Debug, Deserialize, Serialize)]
struct NetInfoResult {
    pub peers: Vec<Peer>,
}

#[derive(Debug, Deserialize, Serialize)]
struct Peer {
    pub node_info: NodeInfo,
    pub remote_ip: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct NodeInfo {
    pub id: String,
    pub listen_addr: String,
}

/// Extract IP address from listen_addr (format: "tcp://0.0.0.0:26657")
fn extract_ip_from_listen_addr(listen_addr: &str, remote_ip: &str) -> Option<String> {
    // If listen_addr contains 0.0.0.0 or empty, use remote_ip
    if listen_addr.contains("0.0.0.0") || listen_addr.is_empty() {
        return Some(remote_ip.to_string());
    }

    // Try to parse the listen_addr
    if let Some(addr_part) = listen_addr.strip_prefix("tcp://") {
        if let Some(ip) = addr_part.split(':').next() {
            if !ip.is_empty() && ip != "0.0.0.0" {
                return Some(ip.to_string());
            }
        }
    }

    Some(remote_ip.to_string())
}

/// Query net_info endpoint of a node
async fn query_net_info(
    client: &Client,
    comet_rpc_url: &str,
    timeout_ms: u64,
) -> Result<Vec<Peer>> {
    let url = format!("{}/net_info", comet_rpc_url.trim_end_matches('/'));

    debug!("Querying net_info from: {}", url);

    let response = client
        .get(&url)
        .timeout(Duration::from_millis(timeout_ms))
        .send()
        .await
        .context(format!("Failed to query net_info from {}", url))?;

    if !response.status().is_success() {
        anyhow::bail!("net_info returned status: {}", response.status());
    }

    let net_info: NetInfoResponse = response
        .json()
        .await
        .context("Failed to parse net_info response")?;

    Ok(net_info.result.peers)
}

/// Discover nodes recursively from a seed node
async fn discover_from_node(
    client: &Client,
    comet_rpc_url: &str,
    config: &Config,
    queried_nodes: &mut HashSet<String>,
    discovered_nodes: &mut Vec<(String, String, String)>, // (id, comet_url, json_url)
) -> Result<()> {
    // Mark this node as queried
    queried_nodes.insert(comet_rpc_url.to_string());

    // Query net_info
    let peers = match query_net_info(client, comet_rpc_url, config.request_timeout_ms).await {
        Ok(p) => p,
        Err(e) => {
            warn!("Failed to query net_info from {}: {}", comet_rpc_url, e);
            return Ok(());
        }
    };

    debug!("Discovered {} peers from {}", peers.len(), comet_rpc_url);

    // Process each peer
    for peer in peers {
        let ip = extract_ip_from_listen_addr(&peer.node_info.listen_addr, &peer.remote_ip);
        
        if let Some(ip) = ip {
            // Construct URLs
            // Assume Comet RPC is at port 26657
            let peer_comet_url = if ip.contains(':') {
                format!("http://{}", ip)
            } else {
                format!("http://{}:26657", ip)
            };

            let peer_json_url = format!("http://{}:{}", ip.split(':').next().unwrap_or(&ip), config.expected_json_rpc_port);

            // Create unique ID from IP
            let peer_id = format!("discovered-{}", peer_comet_url);

            // Check if already queried or discovered
            if queried_nodes.contains(&peer_comet_url) {
                continue;
            }

            // Add to discovered list
            discovered_nodes.push((peer_id, peer_comet_url.clone(), peer_json_url));

            debug!("Discovered new node: {}", peer_comet_url);
        }
    }

    Ok(())
}

/// Main discovery loop
pub async fn discover_nodes(
    client: Arc<Client>,
    config: Arc<Config>,
    pool: NodePool,
    balancer: Arc<LoadBalancer>,
) {
    info!("Starting node discovery loop");

    loop {
        let mut queried_nodes = HashSet::new();
        let mut newly_discovered_count = 0;

        // Start with discoverable nodes from config
        let seed_nodes: Vec<String> = config
            .discoverable_nodes
            .iter()
            .map(|n| n.comet_rpc_url.clone())
            .collect();

        if seed_nodes.is_empty() {
            warn!("No discoverable seed nodes configured");
            sleep(Duration::from_secs(config.discovery_interval_sec)).await;
            continue;
        }

        let mut to_query: Vec<String> = seed_nodes.clone();
        let mut discovered_nodes = Vec::new();

        // Recursive discovery
        while let Some(node_url) = to_query.pop() {
            // Skip if already in pool (unless it's a seed node on first iteration)
            if node_exists(&pool, &format!("discoverable-{}", node_url)).await {
                // Check if we've already queried it in this round
                if queried_nodes.contains(&node_url) {
                    continue;
                }
            }

            // Discover from this node
            if let Err(e) = discover_from_node(
                &client,
                &node_url,
                &config,
                &mut queried_nodes,
                &mut discovered_nodes,
            )
            .await
            {
                error!("Error discovering from {}: {}", node_url, e);
            }

            // Add newly discovered nodes to query list (only if not already queried)
            for (_, comet_url, _) in &discovered_nodes {
                if !queried_nodes.contains(comet_url) && !to_query.contains(comet_url) {
                    to_query.push(comet_url.clone());
                }
            }
        }

        // Add newly discovered nodes to pool
        for (node_id, comet_url, json_url) in discovered_nodes {
            if !node_exists(&pool, &node_id).await {
                let node = Arc::new(Node::new(
                    node_id.clone(),
                    NodeType::Discoverable,
                    comet_url.clone(),
                    json_url.clone(),
                    false, // Discovered nodes are always IP-based, not DNS
                ));

                add_node(&pool, node).await;
                newly_discovered_count += 1;

                info!("Added discovered node: {} ({})", node_id, comet_url);
            }
        }

        // If new nodes were discovered, reset balancer
        if newly_discovered_count > 0 {
            info!("Discovered {} new nodes, resetting balancer", newly_discovered_count);
            balancer.reset_for_new_nodes().await;
        } else {
            debug!("No new nodes discovered in this round");
        }

        // Wait for next discovery interval
        sleep(Duration::from_secs(config.discovery_interval_sec)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_ip_from_listen_addr() {
        // With valid IP in listen_addr
        let ip = extract_ip_from_listen_addr("tcp://192.168.1.1:26657", "10.0.0.1");
        assert_eq!(ip, Some("192.168.1.1".to_string()));

        // With 0.0.0.0 in listen_addr, should use remote_ip
        let ip = extract_ip_from_listen_addr("tcp://0.0.0.0:26657", "10.0.0.2");
        assert_eq!(ip, Some("10.0.0.2".to_string()));

        // With empty listen_addr
        let ip = extract_ip_from_listen_addr("", "10.0.0.3");
        assert_eq!(ip, Some("10.0.0.3".to_string()));
    }
}

