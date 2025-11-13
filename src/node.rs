//! Node management module
//! Handles node representation, pool management, and latency tracking

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeType {
    Static,
    Discoverable,
}

impl NodeType {
    pub fn as_str(&self) -> &str {
        match self {
            NodeType::Static => "static",
            NodeType::Discoverable => "discoverable",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RpcType {
    CometRpc,
    JsonRpc,
}

/// Represents a backend node that can serve RPC requests
pub struct Node {
    pub id: String,
    pub name: String,
    pub node_type: NodeType,
    pub is_dns: bool,
    pub comet_rpc_url: String,
    pub json_rpc_url: String,
    pub is_healthy: AtomicBool,
    pub consecutive_failures: AtomicU32,
    pub times_marked_unhealthy: AtomicU32,
    pub comet_rpc_latency_ms: Arc<RwLock<Option<f64>>>,
    pub json_rpc_latency_ms: Arc<RwLock<Option<f64>>>,
    pub last_health_check: Arc<RwLock<Instant>>,
    pub request_count: AtomicU64,
}

impl Node {
    /// Create a new node
    pub fn new(
        id: String,
        node_type: NodeType,
        comet_rpc_url: String,
        json_rpc_url: String,
        is_dns: bool,
    ) -> Self {
        let name = format!("{}-{}", node_type.as_str(), comet_rpc_url);
        
        Node {
            id,
            name,
            node_type,
            is_dns,
            comet_rpc_url,
            json_rpc_url,
            is_healthy: AtomicBool::new(true), // Start as healthy
            consecutive_failures: AtomicU32::new(0),
            times_marked_unhealthy: AtomicU32::new(0),
            comet_rpc_latency_ms: Arc::new(RwLock::new(None)),
            json_rpc_latency_ms: Arc::new(RwLock::new(None)),
            last_health_check: Arc::new(RwLock::new(Instant::now())),
            request_count: AtomicU64::new(0),
        }
    }

    /// Update latency for a specific RPC type using EMA
    pub async fn update_latency(&self, rpc_type: RpcType, new_latency_ms: f64, ema_alpha: f64) -> f64 {
        let latency_lock = match rpc_type {
            RpcType::CometRpc => &self.comet_rpc_latency_ms,
            RpcType::JsonRpc => &self.json_rpc_latency_ms,
        };

        let mut latency = latency_lock.write().await;
        
        let updated_latency = match *latency {
            Some(old_latency) => {
                // EMA formula: new_value = alpha * measured + (1 - alpha) * old_value
                ema_alpha * new_latency_ms + (1.0 - ema_alpha) * old_latency
            }
            None => {
                // First measurement, use it directly
                new_latency_ms
            }
        };

        *latency = Some(updated_latency);
        updated_latency
    }

    /// Get latency for a specific RPC type
    pub async fn get_latency(&self, rpc_type: RpcType) -> Option<f64> {
        let latency_lock = match rpc_type {
            RpcType::CometRpc => &self.comet_rpc_latency_ms,
            RpcType::JsonRpc => &self.json_rpc_latency_ms,
        };

        *latency_lock.read().await
    }

    /// Check if node is healthy
    pub fn is_healthy(&self) -> bool {
        self.is_healthy.load(Ordering::Relaxed)
    }

    /// Mark node as healthy (resets consecutive failures)
    pub fn mark_healthy(&self) {
        self.is_healthy.store(true, Ordering::Relaxed);
        self.consecutive_failures.store(0, Ordering::Relaxed);
    }

    /// Increment failure count and mark unhealthy if threshold reached
    pub fn increment_failure(&self, threshold: u32) -> bool {
        let failures = self.consecutive_failures.fetch_add(1, Ordering::Relaxed) + 1;
        
        if failures >= threshold && self.is_healthy.load(Ordering::Relaxed) {
            self.is_healthy.store(false, Ordering::Relaxed);
            self.times_marked_unhealthy.fetch_add(1, Ordering::Relaxed);
            true // Node was just marked unhealthy
        } else {
            false
        }
    }

    /// Get consecutive failure count
    pub fn get_consecutive_failures(&self) -> u32 {
        self.consecutive_failures.load(Ordering::Relaxed)
    }

    /// Get times marked unhealthy
    pub fn get_times_marked_unhealthy(&self) -> u32 {
        self.times_marked_unhealthy.load(Ordering::Relaxed)
    }

    /// Increment request count
    pub fn increment_request_count(&self) {
        self.request_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Get request count
    pub fn get_request_count(&self) -> u64 {
        self.request_count.load(Ordering::Relaxed)
    }

    /// Update last health check timestamp
    pub async fn update_health_check_time(&self) {
        let mut last_check = self.last_health_check.write().await;
        *last_check = Instant::now();
    }

    /// Get last health check timestamp
    #[allow(dead_code)]
    pub async fn get_last_health_check(&self) -> Instant {
        *self.last_health_check.read().await
    }
}

/// Pool of nodes managed by the proxy
pub type NodePool = Arc<RwLock<HashMap<String, Arc<Node>>>>;

/// Create a new empty node pool
pub fn create_node_pool() -> NodePool {
    Arc::new(RwLock::new(HashMap::new()))
}

/// Add a node to the pool
pub async fn add_node(pool: &NodePool, node: Arc<Node>) {
    let mut pool_guard = pool.write().await;
    pool_guard.insert(node.id.clone(), node);
}

/// Remove a node from the pool by ID
pub async fn remove_node(pool: &NodePool, node_id: &str) -> bool {
    let mut pool_guard = pool.write().await;
    pool_guard.remove(node_id).is_some()
}

/// Get a node from the pool by ID
#[allow(dead_code)]
pub async fn get_node(pool: &NodePool, node_id: &str) -> Option<Arc<Node>> {
    let pool_guard = pool.read().await;
    pool_guard.get(node_id).cloned()
}

/// Check if a node exists in the pool
pub async fn node_exists(pool: &NodePool, node_id: &str) -> bool {
    let pool_guard = pool.read().await;
    pool_guard.contains_key(node_id)
}

/// Get all healthy nodes from the pool
#[allow(dead_code)]
pub async fn get_healthy_nodes(pool: &NodePool) -> Vec<Arc<Node>> {
    let pool_guard = pool.read().await;
    pool_guard
        .values()
        .filter(|node| node.is_healthy())
        .cloned()
        .collect()
}

/// Get all nodes from the pool
pub async fn get_all_nodes(pool: &NodePool) -> Vec<Arc<Node>> {
    let pool_guard = pool.read().await;
    pool_guard.values().cloned().collect()
}

/// Get count of healthy nodes
pub async fn count_healthy_nodes(pool: &NodePool) -> usize {
    let pool_guard = pool.read().await;
    pool_guard.values().filter(|node| node.is_healthy()).count()
}

/// Get total node count
pub async fn count_total_nodes(pool: &NodePool) -> usize {
    let pool_guard = pool.read().await;
    pool_guard.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_node_creation() {
        let node = Node::new(
            "node1".to_string(),
            NodeType::Static,
            "http://example.com:26657".to_string(),
            "http://example.com:8545".to_string(),
            true,
        );

        assert_eq!(node.id, "node1");
        assert_eq!(node.name, "static-http://example.com:26657");
        assert_eq!(node.node_type, NodeType::Static);
        assert!(node.is_dns);
        assert!(node.is_healthy());
        assert_eq!(node.get_consecutive_failures(), 0);
    }

    #[tokio::test]
    async fn test_latency_update_first_measurement() {
        let node = Node::new(
            "node1".to_string(),
            NodeType::Static,
            "http://example.com:26657".to_string(),
            "http://example.com:8545".to_string(),
            false,
        );

        let latency = node.update_latency(RpcType::CometRpc, 100.0, 0.3).await;
        assert_eq!(latency, 100.0);

        let retrieved = node.get_latency(RpcType::CometRpc).await;
        assert_eq!(retrieved, Some(100.0));
    }

    #[tokio::test]
    async fn test_latency_update_ema() {
        let node = Node::new(
            "node1".to_string(),
            NodeType::Static,
            "http://example.com:26657".to_string(),
            "http://example.com:8545".to_string(),
            false,
        );

        let alpha = 0.3;
        
        // First measurement
        node.update_latency(RpcType::CometRpc, 100.0, alpha).await;
        
        // Second measurement with EMA
        let new_latency = node.update_latency(RpcType::CometRpc, 150.0, alpha).await;
        
        // Expected: 0.3 * 150.0 + 0.7 * 100.0 = 45.0 + 70.0 = 115.0
        assert!((new_latency - 115.0).abs() < 0.001);
    }

    #[tokio::test]
    async fn test_separate_latency_tracking() {
        let node = Node::new(
            "node1".to_string(),
            NodeType::Static,
            "http://example.com:26657".to_string(),
            "http://example.com:8545".to_string(),
            false,
        );

        node.update_latency(RpcType::CometRpc, 100.0, 0.3).await;
        node.update_latency(RpcType::JsonRpc, 200.0, 0.3).await;

        assert_eq!(node.get_latency(RpcType::CometRpc).await, Some(100.0));
        assert_eq!(node.get_latency(RpcType::JsonRpc).await, Some(200.0));
    }

    #[tokio::test]
    async fn test_health_status_transitions() {
        let node = Node::new(
            "node1".to_string(),
            NodeType::Static,
            "http://example.com:26657".to_string(),
            "http://example.com:8545".to_string(),
            false,
        );

        assert!(node.is_healthy());

        // First two failures don't mark as unhealthy (threshold is 3)
        node.increment_failure(3);
        assert!(node.is_healthy());
        
        node.increment_failure(3);
        assert!(node.is_healthy());
        
        // Third failure marks as unhealthy
        let marked_unhealthy = node.increment_failure(3);
        assert!(marked_unhealthy);
        assert!(!node.is_healthy());
        assert_eq!(node.get_times_marked_unhealthy(), 1);

        // Mark as healthy again
        node.mark_healthy();
        assert!(node.is_healthy());
        assert_eq!(node.get_consecutive_failures(), 0);
    }

    #[tokio::test]
    async fn test_request_count() {
        let node = Node::new(
            "node1".to_string(),
            NodeType::Static,
            "http://example.com:26657".to_string(),
            "http://example.com:8545".to_string(),
            false,
        );

        assert_eq!(node.get_request_count(), 0);
        
        node.increment_request_count();
        assert_eq!(node.get_request_count(), 1);
        
        node.increment_request_count();
        node.increment_request_count();
        assert_eq!(node.get_request_count(), 3);
    }

    #[tokio::test]
    async fn test_node_pool_operations() {
        let pool = create_node_pool();

        let node1 = Arc::new(Node::new(
            "node1".to_string(),
            NodeType::Static,
            "http://example.com:26657".to_string(),
            "http://example.com:8545".to_string(),
            false,
        ));

        let node2 = Arc::new(Node::new(
            "node2".to_string(),
            NodeType::Discoverable,
            "http://seed.com:26657".to_string(),
            "http://seed.com:8545".to_string(),
            false,
        ));

        // Add nodes
        add_node(&pool, node1.clone()).await;
        add_node(&pool, node2.clone()).await;

        assert_eq!(count_total_nodes(&pool).await, 2);
        assert!(node_exists(&pool, "node1").await);
        assert!(node_exists(&pool, "node2").await);

        // Get node
        let retrieved = get_node(&pool, "node1").await;
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().id, "node1");

        // Remove node
        let removed = remove_node(&pool, "node1").await;
        assert!(removed);
        assert_eq!(count_total_nodes(&pool).await, 1);
        assert!(!node_exists(&pool, "node1").await);
    }

    #[tokio::test]
    async fn test_healthy_node_filtering() {
        let pool = create_node_pool();

        let node1 = Arc::new(Node::new(
            "node1".to_string(),
            NodeType::Static,
            "http://example1.com:26657".to_string(),
            "http://example1.com:8545".to_string(),
            false,
        ));

        let node2 = Arc::new(Node::new(
            "node2".to_string(),
            NodeType::Static,
            "http://example2.com:26657".to_string(),
            "http://example2.com:8545".to_string(),
            false,
        ));

        add_node(&pool, node1.clone()).await;
        add_node(&pool, node2.clone()).await;

        // Mark node2 as unhealthy
        node2.increment_failure(3);
        node2.increment_failure(3);
        node2.increment_failure(3);

        let healthy = get_healthy_nodes(&pool).await;
        assert_eq!(healthy.len(), 1);
        assert_eq!(healthy[0].id, "node1");

        assert_eq!(count_healthy_nodes(&pool).await, 1);
        assert_eq!(count_total_nodes(&pool).await, 2);
    }
}
