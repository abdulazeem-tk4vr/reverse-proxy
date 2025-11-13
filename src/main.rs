//! Load-Aware Reverse Proxy
//! 
//! A high-performance reverse proxy with intelligent load balancing,
//! health monitoring, and automatic node discovery.

mod balancer;
mod config;
mod dashboard;
mod db;
mod discovery;
mod health;
mod node;
mod proxy;
mod rpc;

use anyhow::{Context, Result};
use balancer::LoadBalancer;
use config::Config;
use db::RequestDatabase;
use node::{add_node, count_healthy_nodes, count_total_nodes, create_node_pool, Node, NodeType};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    // Load configuration
    let config = Config::load("config.json").context("Failed to load configuration")?;

    // Initialize logging
    init_logging(&config);

    info!("🚀 Starting Load-Aware Reverse Proxy");
    info!("Configuration loaded successfully");

    // Create shared HTTP client
    let client = Arc::new(
        reqwest::Client::builder()
            .timeout(Duration::from_millis(config.request_timeout_ms))
            .pool_max_idle_per_host(10)
            .pool_idle_timeout(Duration::from_secs(90))
            .build()
            .context("Failed to create HTTP client")?,
    );

    info!("HTTP client initialized with {}ms timeout", config.request_timeout_ms);

    // Initialize request database (clear old database on restart)
    let db_path = "requests.db";
    if std::path::Path::new(db_path).exists() {
        std::fs::remove_file(db_path)
            .context("Failed to remove old database file")?;
        info!("Cleared old request database");
    }
    let db = Arc::new(RequestDatabase::new(db_path)
        .context("Failed to initialize request database")?);
    info!("Request database initialized");

    // Create node pool
    let pool = create_node_pool();
    info!("Node pool created");

    // Add static nodes if enabled
    if config.is_static {
        info!("Adding {} static nodes", config.static_nodes.len());
        for static_node in &config.static_nodes {
            let node_id = format!("static-{}", static_node.comet_rpc_url);
            let node = Arc::new(Node::new(
                node_id.clone(),
                NodeType::Static,
                static_node.comet_rpc_url.clone(),
                static_node.json_rpc_url.clone(),
                static_node.is_dns,
            ));
            add_node(&pool, node).await;
            info!("Added static node: {} (DNS: {})", node_id, static_node.is_dns);
        }
    }

    // Create load balancer
    let balancer = Arc::new(LoadBalancer::new());
    info!("Load balancer initialized");

    // Setup shutdown signal
    let (shutdown_tx, _) = broadcast::channel::<()>(10);

    // Spawn Ctrl+C handler
    let shutdown_tx_clone = shutdown_tx.clone();
    tokio::spawn(async move {
        match tokio::signal::ctrl_c().await {
            Ok(()) => {
                info!("Received Ctrl+C, initiating graceful shutdown...");
                let _ = shutdown_tx_clone.send(());
            }
            Err(err) => {
                error!("Failed to listen for Ctrl+C signal: {}", err);
            }
        }
    });

    // Spawn discovery task if enabled
    let discovery_handle = if config.enable_discovery {
        info!(
            "Starting node discovery (interval: {}s)",
            config.discovery_interval_sec
        );
        let discovery_client = client.clone();
        let discovery_config = Arc::new(config.clone());
        let discovery_pool = pool.clone();
        let discovery_balancer = balancer.clone();

        Some(tokio::spawn(async move {
            discovery::discover_nodes(
                discovery_client,
                discovery_config,
                discovery_pool,
                discovery_balancer,
            )
            .await;
        }))
    } else {
        info!("Node discovery disabled");
        None
    };

    // Spawn health check task
    info!(
        "Starting health monitoring (interval: {}s, failure threshold: {})",
        config.health_check_interval_sec, config.health_failure_threshold
    );
    let health_client = client.clone();
    let health_config = Arc::new(config.clone());
    let health_pool = pool.clone();
    let health_shutdown_rx = shutdown_tx.subscribe();

    let health_handle = tokio::spawn(async move {
        health::health_check_loop(health_client, health_config, health_pool, health_shutdown_rx)
            .await;
    });

    // Monitor for all nodes unhealthy condition
    let monitor_pool = pool.clone();
    let monitor_shutdown_tx = shutdown_tx.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(10));
        loop {
            interval.tick().await;
            
            let healthy = count_healthy_nodes(&monitor_pool).await;
            let total = count_total_nodes(&monitor_pool).await;
            
            if total > 0 && healthy == 0 {
                error!(
                    "CRITICAL: All {} nodes are unhealthy! Initiating graceful shutdown...",
                    total
                );
                let _ = monitor_shutdown_tx.send(());
                break;
            }
        }
    });

    // Wait a moment for initial setup
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Display initial status
    let healthy = count_healthy_nodes(&pool).await;
    let total = count_total_nodes(&pool).await;
    info!("Initial node status: {}/{} nodes healthy", healthy, total);

    if total == 0 {
        warn!("No nodes available yet. Waiting for discovery or configuration...");
    }

    // Start HTTP server
    info!("Starting HTTP server on port {}", config.http_port);
    info!("Dashboard available at: http://0.0.0.0:{}/", config.http_port);
    info!("Comet RPC endpoint: http://0.0.0.0:{}/rpc", config.http_port);
    info!("JSON-RPC endpoint: http://0.0.0.0:{}/json-rpc", config.http_port);
    
    let server_config = Arc::new(config.clone());
    let server_shutdown_rx = shutdown_tx.subscribe();
    
    let server_result = proxy::start_server(
        pool,
        balancer,
        client,
        server_config,
        db,
        server_shutdown_rx,
    )
    .await;

    // Wait for tasks to complete
    if let Some(handle) = discovery_handle {
        let _ = handle.await;
    }
    let _ = health_handle.await;

    match server_result {
        Ok(_) => {
            info!("Server shut down gracefully");
            Ok(())
        }
        Err(e) => {
            error!("Server error: {}", e);
            Err(anyhow::anyhow!("Server error: {}", e))
        }
    }
}

/// Initialize logging based on configuration
fn init_logging(config: &Config) {
    let filter = if config.verbose {
        EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new("debug"))
    } else {
        EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new("error"))
    };

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_thread_ids(false)
        .with_file(false)
        .with_line_number(false)
        .init();
}

