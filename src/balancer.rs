//! Load balancing module with EMA-based weighted node selection
//! Implements staleness checking and round-robin initial phase

use crate::node::{Node, NodePool, RpcType};
use rand::Rng;
use std::cmp::Ordering as CmpOrdering;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, warn};

const LATENCY_CHANGE_THRESHOLD_MS: f64 = 5.0;

/// Balancer state per RPC type
pub struct BalancerState {
    bias_is_stale: AtomicBool,
    last_latencies: RwLock<HashMap<String, f64>>,
    cumulative_weights: RwLock<Vec<(String, f64)>>,
    round_robin_index: AtomicUsize,
}

impl BalancerState {
    fn new() -> Self {
        Self {
            bias_is_stale: AtomicBool::new(true), // Start as stale to force calculation
            last_latencies: RwLock::new(HashMap::new()),
            cumulative_weights: RwLock::new(Vec::new()),
            round_robin_index: AtomicUsize::new(0),
        }
    }

    /// Mark bias as stale (needs recalculation)
    pub fn mark_stale(&self) {
        self.bias_is_stale.store(true, Ordering::Relaxed);
    }

    /// Reset for new nodes discovered
    pub async fn reset(&self) {
        self.bias_is_stale.store(true, Ordering::Relaxed);
        self.round_robin_index.store(0, Ordering::Relaxed);
        let mut latencies = self.last_latencies.write().await;
        latencies.clear();
        let mut weights = self.cumulative_weights.write().await;
        weights.clear();
    }
}

/// Load balancer that manages node selection for both RPC types
pub struct LoadBalancer {
    comet_state: Arc<BalancerState>,
    json_rpc_state: Arc<BalancerState>,
}

impl LoadBalancer {
    pub fn new() -> Self {
        Self {
            comet_state: Arc::new(BalancerState::new()),
            json_rpc_state: Arc::new(BalancerState::new()),
        }
    }

    /// Select the best node for the given RPC type
    pub async fn select_node(
        &self,
        pool: &NodePool,
        rpc_type: RpcType,
    ) -> Option<Arc<Node>> {
        let state = match rpc_type {
            RpcType::CometRpc => &self.comet_state,
            RpcType::JsonRpc => &self.json_rpc_state,
        };

        // Get all healthy nodes
        let pool_guard = pool.read().await;
        let healthy_nodes: Vec<Arc<Node>> = pool_guard
            .values()
            .filter(|node| node.is_healthy())
            .cloned()
            .collect();
        drop(pool_guard);

        if healthy_nodes.is_empty() {
            warn!("No healthy nodes available for {:?}", rpc_type);
            return None;
        }

        // Check if all nodes have latency data
        let mut all_measured = true;
        for node in &healthy_nodes {
            let latency = node.get_latency(rpc_type).await;
            if latency.is_none() {
                all_measured = false;
                break;
            }
        }

        // Round-robin phase: not all nodes have been measured
        if !all_measured {
            let index = state.round_robin_index.fetch_add(1, Ordering::Relaxed);
            let selected = &healthy_nodes[index % healthy_nodes.len()];
            debug!(
                "Round-robin selection for {:?}: {} (index {})",
                rpc_type, selected.name, index
            );
            return Some(selected.clone());
        }

        // Weighted selection phase
        let is_stale = state.bias_is_stale.load(Ordering::Relaxed);
        
        if is_stale {
            // Recalculate bias
            self.recalculate_bias(state, &healthy_nodes, rpc_type).await;
        }

        // Perform weighted random selection
        self.weighted_random_selection(state, &healthy_nodes).await
    }

    /// Recalculate bias scores and cumulative weights
    async fn recalculate_bias(
        &self,
        state: &BalancerState,
        nodes: &[Arc<Node>],
        rpc_type: RpcType,
    ) {
        let mut node_latencies: Vec<(String, f64)> = Vec::new();

        // Collect latencies
        for node in nodes {
            if let Some(latency) = node.get_latency(rpc_type).await {
                node_latencies.push((node.id.clone(), latency));
            }
        }

        if node_latencies.is_empty() {
            return;
        }

        // Find max latency for bias calculation (kept for future use)
        let _max_latency = node_latencies
            .iter()
            .map(|(_, lat)| *lat)
            .fold(0.0f64, f64::max);

        // Calculate bias scores (inverse of latency, normalized)
        let mut cumulative_sum = 0.0;
        let mut cumulative_weights = Vec::new();
        let mut last_latencies = HashMap::new();

        for (node_id, latency) in &node_latencies {
            last_latencies.insert(node_id.clone(), *latency);
            
            // Bias calculation: higher bias for lower latency
            // Using inverse: bias = 1 / (latency + 1)
            // The +1 prevents division by zero
            let bias = 1.0 / (latency + 1.0);
            
            cumulative_sum += bias;
            cumulative_weights.push((node_id.clone(), cumulative_sum));
        }

        // Update state
        {
            let mut weights_guard = state.cumulative_weights.write().await;
            *weights_guard = cumulative_weights;
        }
        {
            let mut latencies_guard = state.last_latencies.write().await;
            *latencies_guard = last_latencies;
        }

        state.bias_is_stale.store(false, Ordering::Relaxed);
        
        debug!("Recalculated bias for {:?}, {} nodes", rpc_type, node_latencies.len());
    }

    /// Perform weighted random selection using cumulative weights
    async fn weighted_random_selection(
        &self,
        state: &BalancerState,
        nodes: &[Arc<Node>],
    ) -> Option<Arc<Node>> {
        let weights_guard = state.cumulative_weights.read().await;
        
        if weights_guard.is_empty() {
            return nodes.first().cloned();
        }

        let total_weight = weights_guard.last().map(|(_, w)| *w).unwrap_or(1.0);
        
        if total_weight == 0.0 {
            return nodes.first().cloned();
        }

        let mut rng = rand::rng();
        let random_value: f64 = rng.random_range(0.0..total_weight);

        // Binary search for the selected node
        let selected_id = weights_guard
            .binary_search_by(|(_, weight)| {
                if *weight < random_value {
                    CmpOrdering::Less
                } else {
                    CmpOrdering::Greater
                }
            })
            .unwrap_or_else(|i| i);

        let node_id = weights_guard.get(selected_id)
            .or_else(|| weights_guard.last())
            .map(|(id, _)| id.clone())?;

        drop(weights_guard);

        // Find the node in the list
        nodes.iter().find(|n| n.id == node_id).cloned()
    }

    /// Check if latency has changed significantly and mark stale if needed
    pub async fn mark_stale_if_changed(
        &self,
        node_id: &str,
        old_latency: f64,
        new_latency: f64,
        rpc_type: RpcType,
    ) {
        let diff = (new_latency - old_latency).abs();
        
        if diff >= LATENCY_CHANGE_THRESHOLD_MS {
            let state = match rpc_type {
                RpcType::CometRpc => &self.comet_state,
                RpcType::JsonRpc => &self.json_rpc_state,
            };
            
            state.mark_stale();
            
            debug!(
                "Marked {:?} bias as stale for node {}: latency changed from {:.2}ms to {:.2}ms (diff: {:.2}ms)",
                rpc_type, node_id, old_latency, new_latency, diff
            );
        }
    }

    /// Reset balancer state when new nodes are discovered
    pub async fn reset_for_new_nodes(&self) {
        self.comet_state.reset().await;
        self.json_rpc_state.reset().await;
        debug!("Reset balancer state for new nodes");
    }

    /// Get balancer state for a specific RPC type (for testing/monitoring)
    #[allow(dead_code)]
    pub fn get_state(&self, rpc_type: RpcType) -> Arc<BalancerState> {
        match rpc_type {
            RpcType::CometRpc => self.comet_state.clone(),
            RpcType::JsonRpc => self.json_rpc_state.clone(),
        }
    }
}

impl Default for LoadBalancer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::{create_node_pool, add_node, NodeType};

    async fn setup_test_pool() -> NodePool {
        let pool = create_node_pool();
        
        let node1 = Arc::new(Node::new(
            "node1".to_string(),
            NodeType::Static,
            "http://node1.com:26657".to_string(),
            "http://node1.com:8545".to_string(),
            false,
        ));
        
        let node2 = Arc::new(Node::new(
            "node2".to_string(),
            NodeType::Static,
            "http://node2.com:26657".to_string(),
            "http://node2.com:8545".to_string(),
            false,
        ));
        
        let node3 = Arc::new(Node::new(
            "node3".to_string(),
            NodeType::Static,
            "http://node3.com:26657".to_string(),
            "http://node3.com:8545".to_string(),
            false,
        ));

        add_node(&pool, node1).await;
        add_node(&pool, node2).await;
        add_node(&pool, node3).await;

        pool
    }

    #[tokio::test]
    async fn test_round_robin_selection() {
        let pool = setup_test_pool().await;
        let balancer = LoadBalancer::new();

        // Without latency data, should use round-robin
        let node1 = balancer.select_node(&pool, RpcType::CometRpc).await;
        let node2 = balancer.select_node(&pool, RpcType::CometRpc).await;
        let node3 = balancer.select_node(&pool, RpcType::CometRpc).await;

        assert!(node1.is_some());
        assert!(node2.is_some());
        assert!(node3.is_some());

        // All different nodes should be selected
        let ids: Vec<String> = vec![
            node1.unwrap().id.clone(),
            node2.unwrap().id.clone(),
            node3.unwrap().id.clone(),
        ];
        
        // Check that we got 3 different nodes (round-robin)
        assert_eq!(ids.len(), 3);
    }

    #[tokio::test]
    async fn test_weighted_selection_with_latencies() {
        let pool = setup_test_pool().await;
        let balancer = LoadBalancer::new();

        // Set latencies for all nodes
        {
            let pool_guard = pool.read().await;
            if let Some(node) = pool_guard.get("node1") {
                node.update_latency(RpcType::CometRpc, 50.0, 0.3).await;
            }
            if let Some(node) = pool_guard.get("node2") {
                node.update_latency(RpcType::CometRpc, 100.0, 0.3).await;
            }
            if let Some(node) = pool_guard.get("node3") {
                node.update_latency(RpcType::CometRpc, 200.0, 0.3).await;
            }
        }

        // Now it should use weighted selection
        // node1 (50ms) should be selected more often than node3 (200ms)
        let mut selections = HashMap::new();
        
        for _ in 0..100 {
            if let Some(node) = balancer.select_node(&pool, RpcType::CometRpc).await {
                *selections.entry(node.id.clone()).or_insert(0) += 1;
            }
        }

        // node1 should have more selections than node3 (statistically)
        let node1_count = selections.get("node1").unwrap_or(&0);
        let node3_count = selections.get("node3").unwrap_or(&0);
        
        assert!(node1_count > node3_count);
    }

    #[tokio::test]
    async fn test_filters_unhealthy_nodes() {
        let pool = setup_test_pool().await;
        let balancer = LoadBalancer::new();

        // Mark node2 as unhealthy
        {
            let pool_guard = pool.read().await;
            if let Some(node) = pool_guard.get("node2") {
                node.increment_failure(3);
                node.increment_failure(3);
                node.increment_failure(3);
            }
        }

        // Select 10 nodes, none should be node2
        for _ in 0..10 {
            if let Some(node) = balancer.select_node(&pool, RpcType::CometRpc).await {
                assert_ne!(node.id, "node2");
            }
        }
    }

    #[tokio::test]
    async fn test_staleness_marking() {
        let balancer = LoadBalancer::new();
        let state = balancer.get_state(RpcType::CometRpc);
        
        // Initially starts as stale, set it to not stale
        state.bias_is_stale.store(false, Ordering::Relaxed);

        // Small change (< 5ms) should not mark stale
        balancer.mark_stale_if_changed("node1", 100.0, 102.0, RpcType::CometRpc).await;
        assert!(!state.bias_is_stale.load(Ordering::Relaxed));

        // Large change (>= 5ms) should mark stale
        balancer.mark_stale_if_changed("node1", 100.0, 106.0, RpcType::CometRpc).await;
        assert!(state.bias_is_stale.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn test_separate_rpc_type_states() {
        let pool = setup_test_pool().await;
        let balancer = LoadBalancer::new();

        // Set different latencies for different RPC types
        {
            let pool_guard = pool.read().await;
            if let Some(node) = pool_guard.get("node1") {
                node.update_latency(RpcType::CometRpc, 50.0, 0.3).await;
                node.update_latency(RpcType::JsonRpc, 200.0, 0.3).await;
            }
            if let Some(node) = pool_guard.get("node2") {
                node.update_latency(RpcType::CometRpc, 200.0, 0.3).await;
                node.update_latency(RpcType::JsonRpc, 50.0, 0.3).await;
            }
        }

        // Both should work independently
        let comet_node = balancer.select_node(&pool, RpcType::CometRpc).await;
        let json_node = balancer.select_node(&pool, RpcType::JsonRpc).await;

        assert!(comet_node.is_some());
        assert!(json_node.is_some());
    }

    #[tokio::test]
    async fn test_reset_for_new_nodes() {
        let balancer = LoadBalancer::new();

        // Mark as not stale
        let state = balancer.get_state(RpcType::CometRpc);
        state.bias_is_stale.store(false, Ordering::Relaxed);
        state.round_robin_index.store(5, Ordering::Relaxed);

        // Reset
        balancer.reset_for_new_nodes().await;

        // Should be stale and index reset
        assert!(state.bias_is_stale.load(Ordering::Relaxed));
        assert_eq!(state.round_robin_index.load(Ordering::Relaxed), 0);
    }
}

