//! HTTP proxy server with Axum
//! Handles routing and request forwarding

use crate::balancer::LoadBalancer;
use crate::config::Config;
use crate::dashboard::{dashboard_handler, health_handler, status_handler, AppState};
use crate::db::RequestDatabase;
use crate::node::{NodePool, RpcType};
use crate::rpc::{proxy_comet_rpc, proxy_json_rpc, proxy_with_retry};
use axum::{
    body::Body,
    extract::State,
    http::{Request, StatusCode},
    response::{Json, Response},
    routing::{get, post},
    Router,
};
use reqwest::Client;
use serde_json::json;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::broadcast;
use tracing::{debug, error, info};

/// Shared state for all handlers
pub struct ProxyState {
    pub pool: NodePool,
    pub balancer: Arc<LoadBalancer>,
    pub client: Arc<Client>,
    pub config: Arc<Config>,
    pub db: Arc<RequestDatabase>,
}

/// Handle Comet RPC requests
async fn handle_comet_rpc(
    State(state): State<Arc<ProxyState>>,
    req: Request<Body>,
) -> Result<Response, StatusCode> {
    // Extract path (e.g., /rpc/status -> /status)
    let path = req.uri().path().strip_prefix("/rpc").unwrap_or("/");
    let query = req.uri().query().map(|q| format!("?{}", q)).unwrap_or_default();
    let path_and_query = format!("{}{}", path, query);
    
    debug!("Received Comet RPC request: {}", path_and_query);

    let request_start = Instant::now();
    let selected_node_name = Arc::new(std::sync::RwLock::new(String::from("unknown")));
    let selected_node_url = Arc::new(std::sync::RwLock::new(String::from("unknown")));

    // Extract request method and body
    let method = req.method().to_string();
    let body_bytes = match axum::body::to_bytes(req.into_body(), usize::MAX).await {
        Ok(bytes) => bytes.to_vec(),
        Err(e) => {
            error!("Failed to read request body: {}", e);
            return Err(StatusCode::BAD_REQUEST);
        }
    };

    // Attempt proxy with retry
    let result = proxy_with_retry(
        || {
            let state = state.clone();
            let body = body_bytes.clone();
            let method = method.clone();
            let path_and_query = path_and_query.clone();
            let node_name = selected_node_name.clone();
            let node_url = selected_node_url.clone();
            async move {
                // Select a node
                let node = state
                    .balancer
                    .select_node(&state.pool, RpcType::CometRpc)
                    .await
                    .ok_or_else(|| anyhow::anyhow!("No healthy nodes available"))?;

                // Store node info for logging
                *node_name.write().unwrap() = node.name.clone();
                *node_url.write().unwrap() = node.comet_rpc_url.clone();

                // Proxy the request
                proxy_comet_rpc(
                    &state.client,
                    &node,
                    body,
                    &method,
                    &path_and_query,
                    &state.balancer,
                    &state.config,
                )
                .await
            }
        },
        state.config.request_retry_count,
        "Comet RPC request",
    )
    .await;

    let latency_ms = request_start.elapsed().as_secs_f64() * 1000.0;
    let node_name = selected_node_name.read().unwrap().clone();
    let node_url = selected_node_url.read().unwrap().clone();

    match result {
        Ok(response_bytes) => {
            // Log successful request
            let _ = state.db.log_request(
                "comet",
                &path_and_query,
                &node_name,
                &node_url,
                200,
                latency_ms,
                true,
            );

            let response = Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", "application/json")
                .header("Access-Control-Allow-Origin", "*")
                .body(Body::from(response_bytes))
                .unwrap();
            Ok(response)
        }
        Err(e) => {
            // Log failed request
            let _ = state.db.log_request(
                "comet",
                &path_and_query,
                &node_name,
                &node_url,
                503,
                latency_ms,
                false,
            );

            error!("Comet RPC request failed after retries: {}", e);
            Err(StatusCode::SERVICE_UNAVAILABLE)
        }
    }
}

/// Handle JSON-RPC requests
async fn handle_json_rpc(
    State(state): State<Arc<ProxyState>>,
    req: Request<Body>,
) -> Result<Response, StatusCode> {
    debug!("Received JSON-RPC request");

    let request_start = Instant::now();
    let selected_node_name = Arc::new(std::sync::RwLock::new(String::from("unknown")));
    let selected_node_url = Arc::new(std::sync::RwLock::new(String::from("unknown")));

    // Extract request body
    let body_bytes = match axum::body::to_bytes(req.into_body(), usize::MAX).await {
        Ok(bytes) => bytes.to_vec(),
        Err(e) => {
            error!("Failed to read request body: {}", e);
            return Err(StatusCode::BAD_REQUEST);
        }
    };

    // Extract JSON-RPC method from request body
    let method_name = if let Ok(json_body) = serde_json::from_slice::<serde_json::Value>(&body_bytes) {
        json_body.get("method")
            .and_then(|m| m.as_str())
            .map(|m| m.to_string())
            .unwrap_or_else(|| "unknown".to_string())
    } else {
        "unknown".to_string()
    };

    // Attempt proxy with retry
    let result = proxy_with_retry(
        || {
            let state = state.clone();
            let body = body_bytes.clone();
            let node_name = selected_node_name.clone();
            let node_url = selected_node_url.clone();
            async move {
                // Select a node
                let node = state
                    .balancer
                    .select_node(&state.pool, RpcType::JsonRpc)
                    .await
                    .ok_or_else(|| anyhow::anyhow!("No healthy nodes available"))?;

                // Store node info for logging
                *node_name.write().unwrap() = node.name.clone();
                *node_url.write().unwrap() = node.json_rpc_url.clone();

                // Proxy the request
                proxy_json_rpc(&state.client, &node, body, &state.balancer, &state.config).await
            }
        },
        state.config.request_retry_count,
        "JSON-RPC request",
    )
    .await;

    let latency_ms = request_start.elapsed().as_secs_f64() * 1000.0;
    let node_name = selected_node_name.read().unwrap().clone();
    let node_url = selected_node_url.read().unwrap().clone();

    match result {
        Ok(response_bytes) => {
            // Log successful request
            let _ = state.db.log_request(
                "json-rpc",
                &method_name,
                &node_name,
                &node_url,
                200,
                latency_ms,
                true,
            );

            let response = Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", "application/json")
                .header("Access-Control-Allow-Origin", "*")
                .body(Body::from(response_bytes))
                .unwrap();
            Ok(response)
        }
        Err(e) => {
            // Log failed request
            let _ = state.db.log_request(
                "json-rpc",
                &method_name,
                &node_name,
                &node_url,
                503,
                latency_ms,
                false,
            );

            error!("JSON-RPC request failed after retries: {}", e);
            Err(StatusCode::SERVICE_UNAVAILABLE)
        }
    }
}

/// Handler for request history API
async fn requests_handler(
    State(state): State<Arc<ProxyState>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    match state.db.get_recent_requests(100) {
        Ok(logs) => Ok(Json(json!({ "requests": logs }))),
        Err(e) => {
            error!("Failed to fetch request logs: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// Handler for request statistics API
async fn requests_stats_handler(
    State(state): State<Arc<ProxyState>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    match state.db.get_stats() {
        Ok(stats) => Ok(Json(json!(stats))),
        Err(e) => {
            error!("Failed to fetch request stats: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// Start the HTTP proxy server
pub async fn start_server(
    pool: NodePool,
    balancer: Arc<LoadBalancer>,
    client: Arc<Client>,
    config: Arc<Config>,
    db: Arc<RequestDatabase>,
    mut shutdown_rx: broadcast::Receiver<()>,
) -> Result<(), Box<dyn std::error::Error>> {
    let proxy_state = Arc::new(ProxyState {
        pool: pool.clone(),
        balancer,
        client,
        config: config.clone(),
        db: db.clone(),
    });

    let dashboard_state = Arc::new(AppState {
        pool,
        config: config.clone(),
        start_time: Instant::now(),
    });

    // Build router
    let app = Router::new()
        .route("/rpc/{*path}", post(handle_comet_rpc).get(handle_comet_rpc))
        .route("/rpc", post(handle_comet_rpc).get(handle_comet_rpc))
        .with_state(proxy_state.clone())
        .route("/json-rpc", post(handle_json_rpc))
        .with_state(proxy_state.clone())
        .route("/api/requests", get(requests_handler))
        .with_state(proxy_state.clone())
        .route("/api/requests/stats", get(requests_stats_handler))
        .with_state(proxy_state.clone())
        .route("/health", get(health_handler))
        .with_state(dashboard_state.clone())
        .route("/api/status", get(status_handler))
        .with_state(dashboard_state.clone())
        .route("/", get(dashboard_handler))
        .route("/dashboard", get(dashboard_handler));

    let addr = format!("0.0.0.0:{}", config.http_port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    
    info!("HTTP server listening on {}", addr);

    // Run server with graceful shutdown
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = shutdown_rx.recv().await;
            info!("HTTP server shutting down gracefully");
        })
        .await?;

    Ok(())
}
