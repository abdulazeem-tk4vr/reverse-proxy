# Load-Aware Reverse Proxy

A high-performance reverse proxy with intelligent load balancing, health monitoring, and automatic node discovery for Comet RPC and JSON-RPC endpoints.

## Features

- **Intelligent Load Balancing**: EMA-based weighted selection favoring low-latency nodes
- **Dual RPC Support**: Separate handling for Comet RPC and JSON-RPC endpoints
- **Automatic Node Discovery**: Recursive discovery via Comet RPC's `/net_info` endpoint
- **Health Monitoring**: Periodic health checks with automatic node removal
- **Real-time Dashboard**: Web-based monitoring interface
- **Graceful Shutdown**: Proper cleanup on Ctrl+C or when all nodes are unhealthy
- **Request Retry Logic**: Configurable retry mechanism for failed requests
- **Separate Latency Tracking**: Independent EMA latency tracking per RPC type

## Quick Start

### Prerequisites

- Rust 1.70+ (install from [rust-lang.org](https://www.rust-lang.org/))

### Build

```bash
cargo build --release
```

### Configuration

Create or edit `config.json`:

```json
{
  "verbose": true,
  "http_port": 8080,
  "is_static": true,
  "enable_discovery": true,
  "discovery_interval_sec": 300,
  "health_check_interval_sec": 30,
  "health_failure_threshold": 3,
  "unhealthy_removal_threshold": 3,
  "request_retry_count": 2,
  "ema_alpha": 0.3,
  "request_timeout_ms": 5000,
  "expected_json_rpc_port": 8545,
  "static_nodes": [
    {
      "comet_rpc_url": "http://node1.example.com:26657",
      "json_rpc_url": "http://node1.example.com:8545"
    }
  ],
  "discoverable_nodes": [
    {
      "comet_rpc_url": "http://seed.example.com:26657"
    }
  ]
}
```

### Run

```bash
cargo run --release
```

Or with the binary:

```bash
./target/release/load-aware-proxy
```

## Endpoints

- **`/rpc`** - Comet RPC proxy endpoint (GET/POST)
- **`/json-rpc`** - JSON-RPC proxy endpoint (POST)
- **`/health`** - Health check endpoint (returns node status)
- **`/api/status`** - Detailed status API (returns full statistics)
- **`/`** or **`/dashboard`** - Web dashboard

## Configuration Options

| Option | Type | Description |
|--------|------|-------------|
| `verbose` | bool | Enable debug logging (otherwise only errors) |
| `http_port` | u16 | Port for the proxy server |
| `is_static` | bool | Load static nodes from config |
| `enable_discovery` | bool | Enable recursive node discovery |
| `discovery_interval_sec` | u64 | How often to discover new nodes |
| `health_check_interval_sec` | u64 | How often to check node health |
| `health_failure_threshold` | u32 | Consecutive failures before marking unhealthy |
| `unhealthy_removal_threshold` | u32 | Times marked unhealthy before removal |
| `request_retry_count` | u32 | Number of retries on request failure |
| `ema_alpha` | f64 | EMA smoothing factor (0.0-1.0) |
| `request_timeout_ms` | u64 | Request timeout in milliseconds |
| `expected_json_rpc_port` | u16 | JSON-RPC port for discovered nodes |

## Architecture

### Core Modules

1. **config.rs** - Configuration loading and validation
2. **node.rs** - Node representation and pool management
3. **balancer.rs** - Load balancing with EMA-based selection
4. **discovery.rs** - Recursive node discovery
5. **health.rs** - Health monitoring system
6. **rpc.rs** - Request proxying and latency tracking
7. **proxy.rs** - Axum HTTP server
8. **dashboard.rs** - Monitoring endpoints
9. **main.rs** - Application orchestration

### Load Balancing Algorithm

The proxy uses a sophisticated load balancing algorithm:

1. **Initial Phase**: Round-robin selection until all nodes have latency data
2. **Weighted Phase**: Bias calculation based on inverse latency
3. **Staleness Detection**: Recalculate weights when latency changes ≥5ms
4. **Separate Tracking**: Independent latency tracking for Comet RPC and JSON-RPC

Bias formula: `bias = 1.0 / (latency + 1.0)`

### Node Discovery

- Starts with seed nodes from `discoverable_nodes` config
- Queries `/net_info` endpoint to find peers
- Recursively discovers from newly found nodes
- Avoids infinite loops with visited set
- Resets balancer when new nodes are discovered

### Health Monitoring

- Checks Comet RPC via `/status` endpoint
- Checks JSON-RPC via `eth_blockNumber` method
- Node marked unhealthy after N consecutive failures
- Node removed after M times marked unhealthy
- Automatic graceful shutdown if all nodes unhealthy

## Development

### Run Tests

```bash
cargo test
```

### Run with Verbose Logging

Set `verbose: true` in `config.json` or use:

```bash
RUST_LOG=debug cargo run
```

### Build for Production

```bash
cargo build --release --target x86_64-unknown-linux-gnu
```

## Example Usage

### Using Comet RPC

```bash
curl http://localhost:8080/rpc/status
```

### Using JSON-RPC

```bash
curl -X POST http://localhost:8080/json-rpc \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "method": "eth_blockNumber",
    "params": [],
    "id": 1
  }'
```

### Check Health

```bash
curl http://localhost:8080/health
```

### View Dashboard

Open in browser: `http://localhost:8080/`

## Monitoring

The dashboard provides real-time monitoring of:

- Node health status
- Latency metrics (Comet RPC and JSON-RPC)
- Request counts per node
- Failure statistics
- Configuration summary
- System uptime

## License

MIT License - see LICENSE file for details

## Author

Abdul Azeem

## Contributing

Contributions are welcome! Please ensure all tests pass before submitting a PR.

```bash
cargo test
cargo fmt
cargo clippy
```
