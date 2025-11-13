//! RPC proxying module
//! Handles forwarding requests to backend nodes and tracking latency

use crate::balancer::LoadBalancer;
use crate::config::Config;
use crate::node::{Node, RpcType};
use anyhow::{Context, Result};
use reqwest::Client;
use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, error, warn};

/// Proxy a Comet RPC request to the selected node
pub async fn proxy_comet_rpc(
    client: &Client,
    node: &Arc<Node>,
    request_body: Vec<u8>,
    request_method: &str,
    path_and_query: &str,
    balancer: &Arc<LoadBalancer>,
    config: &Arc<Config>,
) -> Result<Vec<u8>> {
    // Construct full URL with path
    let url = format!("{}{}", node.comet_rpc_url.trim_end_matches('/'), path_and_query);
    let start = Instant::now();

    debug!(
        "Proxying Comet RPC request to {} (method: {}, path: {})",
        node.name, request_method, path_and_query
    );

    // Forward the request
    let response = match request_method {
        "GET" => {
            client
                .get(&url)
                .body(request_body)
                .timeout(std::time::Duration::from_millis(config.request_timeout_ms))
                .send()
                .await
        }
        "POST" => {
            client
                .post(&url)
                .body(request_body)
                .header("Content-Type", "application/json")
                .timeout(std::time::Duration::from_millis(config.request_timeout_ms))
                .send()
                .await
        }
        _ => {
            // Default to POST for other methods
            client
                .post(&url)
                .body(request_body)
                .header("Content-Type", "application/json")
                .timeout(std::time::Duration::from_millis(config.request_timeout_ms))
                .send()
                .await
        }
    };

    let elapsed = start.elapsed().as_millis() as f64;

    match response {
        Ok(resp) => {
            let status = resp.status();
            let response_bytes = resp
                .bytes()
                .await
                .context("Failed to read response body")?
                .to_vec();

            if !status.is_success() {
                warn!(
                    "Comet RPC request to {} returned status {}",
                    node.name, status
                );
            }

            // Update latency with EMA
            let old_latency = node.get_latency(RpcType::CometRpc).await.unwrap_or(elapsed);
            let new_latency = node.update_latency(RpcType::CometRpc, elapsed, config.ema_alpha).await;

            // Mark stale if changed significantly
            balancer
                .mark_stale_if_changed(&node.id, old_latency, new_latency, RpcType::CometRpc)
                .await;

            // Increment request count
            node.increment_request_count();

            debug!(
                "Comet RPC request to {} completed in {:.2}ms (EMA: {:.2}ms)",
                node.name, elapsed, new_latency
            );

            Ok(response_bytes)
        }
        Err(e) => {
            error!("Comet RPC request to {} failed: {}", node.name, e);
            Err(anyhow::anyhow!("Request failed: {}", e))
        }
    }
}

/// Proxy a JSON-RPC request to the selected node
pub async fn proxy_json_rpc(
    client: &Client,
    node: &Arc<Node>,
    request_body: Vec<u8>,
    balancer: &Arc<LoadBalancer>,
    config: &Arc<Config>,
) -> Result<Vec<u8>> {
    let url = node.json_rpc_url.clone();
    let start = Instant::now();

    debug!("Proxying JSON-RPC request to {}", node.name);

    // Forward the request (JSON-RPC is always POST)
    let response = client
        .post(&url)
        .body(request_body)
        .header("Content-Type", "application/json")
        .timeout(std::time::Duration::from_millis(config.request_timeout_ms))
        .send()
        .await;

    let elapsed = start.elapsed().as_millis() as f64;

    match response {
        Ok(resp) => {
            let status = resp.status();
            let response_bytes = resp
                .bytes()
                .await
                .context("Failed to read response body")?
                .to_vec();

            if !status.is_success() {
                warn!(
                    "JSON-RPC request to {} returned status {}",
                    node.name, status
                );
            }

            // Update latency with EMA
            let old_latency = node.get_latency(RpcType::JsonRpc).await.unwrap_or(elapsed);
            let new_latency = node.update_latency(RpcType::JsonRpc, elapsed, config.ema_alpha).await;

            // Mark stale if changed significantly
            balancer
                .mark_stale_if_changed(&node.id, old_latency, new_latency, RpcType::JsonRpc)
                .await;

            // Increment request count
            node.increment_request_count();

            debug!(
                "JSON-RPC request to {} completed in {:.2}ms (EMA: {:.2}ms)",
                node.name, elapsed, new_latency
            );

            Ok(response_bytes)
        }
        Err(e) => {
            error!("JSON-RPC request to {} failed: {}", node.name, e);
            Err(anyhow::anyhow!("Request failed: {}", e))
        }
    }
}

/// Proxy request with retry logic
pub async fn proxy_with_retry<F, Fut>(
    mut proxy_fn: F,
    max_retries: u32,
    description: &str,
) -> Result<Vec<u8>>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<Vec<u8>>>,
{
    let mut last_error = None;

    for attempt in 0..=max_retries {
        match proxy_fn().await {
            Ok(response) => return Ok(response),
            Err(e) => {
                if attempt < max_retries {
                    warn!(
                        "{} failed (attempt {}/{}): {}. Retrying...",
                        description,
                        attempt + 1,
                        max_retries + 1,
                        e
                    );
                }
                last_error = Some(e);
            }
        }
    }

    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("All retries failed")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_proxy_with_retry_success_first_attempt() {
        let result = proxy_with_retry(
            || async { Ok(vec![1, 2, 3]) },
            2,
            "test",
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn test_proxy_with_retry_success_after_failures() {
        let mut attempt = 0;
        
        let result = proxy_with_retry(
            || {
                attempt += 1;
                async move {
                    if attempt < 3 {
                        Err(anyhow::anyhow!("Failed"))
                    } else {
                        Ok(vec![1, 2, 3])
                    }
                }
            },
            3,
            "test",
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn test_proxy_with_retry_all_fail() {
        let result = proxy_with_retry(
            || async { Err(anyhow::anyhow!("Always fails")) },
            2,
            "test",
        )
        .await;

        assert!(result.is_err());
    }
}

