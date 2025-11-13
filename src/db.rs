//! Database module for request tracking
//! Uses SQLite to store request history

use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

/// Request log entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestLog {
    pub id: i64,
    pub timestamp: String,
    pub rpc_type: String, // "comet" or "json-rpc"
    pub path: String,
    pub node_name: String,
    pub node_url: String,
    pub status_code: u16,
    pub latency_ms: f64,
    pub success: bool,
}

/// Database handler for request logging
pub struct RequestDatabase {
    conn: Arc<Mutex<Connection>>,
}

impl RequestDatabase {
    /// Create a new database instance
    pub fn new(path: &str) -> Result<Self> {
        let conn = Connection::open(path)
            .context("Failed to open SQLite database")?;

        // Create tables if they don't exist
        conn.execute(
            "CREATE TABLE IF NOT EXISTS request_logs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp TEXT NOT NULL,
                rpc_type TEXT NOT NULL,
                path TEXT NOT NULL,
                node_name TEXT NOT NULL,
                node_url TEXT NOT NULL,
                status_code INTEGER NOT NULL,
                latency_ms REAL NOT NULL,
                success INTEGER NOT NULL
            )",
            [],
        ).context("Failed to create request_logs table")?;

        // Create index on timestamp for faster queries
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_timestamp ON request_logs(timestamp DESC)",
            [],
        ).ok(); // Ignore if index already exists

        Ok(RequestDatabase {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Log a new request
    pub fn log_request(
        &self,
        rpc_type: &str,
        path: &str,
        node_name: &str,
        node_url: &str,
        status_code: u16,
        latency_ms: f64,
        success: bool,
    ) -> Result<()> {
        let timestamp = Utc::now().to_rfc3339();
        let conn = self.conn.lock().unwrap();

        conn.execute(
            "INSERT INTO request_logs 
            (timestamp, rpc_type, path, node_name, node_url, status_code, latency_ms, success) 
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                timestamp,
                rpc_type,
                path,
                node_name,
                node_url,
                status_code,
                latency_ms,
                if success { 1 } else { 0 }
            ],
        ).context("Failed to insert request log")?;

        Ok(())
    }

    /// Get recent requests (limited)
    pub fn get_recent_requests(&self, limit: usize) -> Result<Vec<RequestLog>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, timestamp, rpc_type, path, node_name, node_url, status_code, latency_ms, success 
             FROM request_logs 
             ORDER BY id DESC 
             LIMIT ?1"
        )?;

        let rows = stmt.query_map(params![limit], |row| {
            Ok(RequestLog {
                id: row.get(0)?,
                timestamp: row.get(1)?,
                rpc_type: row.get(2)?,
                path: row.get(3)?,
                node_name: row.get(4)?,
                node_url: row.get(5)?,
                status_code: row.get(6)?,
                latency_ms: row.get(7)?,
                success: row.get::<_, i32>(8)? == 1,
            })
        })?;

        let mut logs = Vec::new();
        for log_result in rows {
            logs.push(log_result?);
        }

        Ok(logs)
    }

    /// Get request statistics
    pub fn get_stats(&self) -> Result<RequestStats> {
        let conn = self.conn.lock().unwrap();

        let total: i64 = conn.query_row(
            "SELECT COUNT(*) FROM request_logs",
            [],
            |row| row.get(0),
        )?;

        let successful: i64 = conn.query_row(
            "SELECT COUNT(*) FROM request_logs WHERE success = 1",
            [],
            |row| row.get(0),
        )?;

        let avg_latency: Option<f64> = conn.query_row(
            "SELECT AVG(latency_ms) FROM request_logs WHERE success = 1",
            [],
            |row| row.get(0),
        ).ok();

        let comet_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM request_logs WHERE rpc_type = 'comet'",
            [],
            |row| row.get(0),
        )?;

        let json_rpc_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM request_logs WHERE rpc_type = 'json-rpc'",
            [],
            |row| row.get(0),
        )?;

        Ok(RequestStats {
            total_requests: total as u64,
            successful_requests: successful as u64,
            failed_requests: (total - successful) as u64,
            avg_latency_ms: avg_latency,
            comet_requests: comet_count as u64,
            json_rpc_requests: json_rpc_count as u64,
        })
    }

    /// Clean old logs (keep only last N entries)
    pub fn cleanup_old_logs(&self, keep_last: usize) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        
        conn.execute(
            "DELETE FROM request_logs 
             WHERE id NOT IN (
                 SELECT id FROM request_logs 
                 ORDER BY id DESC 
                 LIMIT ?1
             )",
            params![keep_last],
        )?;

        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RequestStats {
    pub total_requests: u64,
    pub successful_requests: u64,
    pub failed_requests: u64,
    pub avg_latency_ms: Option<f64>,
    pub comet_requests: u64,
    pub json_rpc_requests: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_database_creation() {
        let db = RequestDatabase::new(":memory:").unwrap();
        assert!(db.conn.lock().is_ok());
    }

    #[test]
    fn test_log_and_retrieve() {
        let db = RequestDatabase::new(":memory:").unwrap();
        
        db.log_request(
            "json-rpc",
            "/eth_blockNumber",
            "test-node",
            "http://test:8545",
            200,
            150.5,
            true,
        ).unwrap();

        let logs = db.get_recent_requests(10).unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].rpc_type, "json-rpc");
        assert_eq!(logs[0].node_name, "test-node");
        assert!(logs[0].success);
    }

    #[test]
    fn test_stats() {
        let db = RequestDatabase::new(":memory:").unwrap();
        
        db.log_request("comet", "/status", "node1", "http://node1", 200, 100.0, true).unwrap();
        db.log_request("json-rpc", "/", "node2", "http://node2", 200, 150.0, true).unwrap();
        db.log_request("comet", "/net_info", "node1", "http://node1", 500, 200.0, false).unwrap();

        let stats = db.get_stats().unwrap();
        assert_eq!(stats.total_requests, 3);
        assert_eq!(stats.successful_requests, 2);
        assert_eq!(stats.failed_requests, 1);
        assert_eq!(stats.comet_requests, 2);
        assert_eq!(stats.json_rpc_requests, 1);
    }
}

