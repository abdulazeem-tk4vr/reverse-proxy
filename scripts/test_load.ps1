# Load Testing Script for Load-Aware Proxy (PowerShell)
# This script sends multiple requests to test load balancing

$ProxyUrl = "http://localhost:8080"
$NumRequests = 20
$DelayMs = 200

Write-Host "🚀 Starting Load Test..." -ForegroundColor Cyan
Write-Host "Proxy: $ProxyUrl"
Write-Host "Requests per endpoint: $NumRequests"
Write-Host "Delay between requests: ${DelayMs}ms"
Write-Host ""

# Function to send JSON-RPC requests
function Test-JsonRpc {
    Write-Host "📊 Testing JSON-RPC endpoints..." -ForegroundColor Yellow
    
    for ($i = 1; $i -le $NumRequests; $i++) {
        Write-Host -NoNewline "JSON-RPC Request ${i}: "
        
        try {
            $body = @{
                jsonrpc = "2.0"
                method = "eth_blockNumber"
                params = @()
                id = 1
            } | ConvertTo-Json
            
            $response = Invoke-RestMethod -Uri "$ProxyUrl/json-rpc" `
                -Method Post `
                -ContentType "application/json" `
                -Body $body `
                -TimeoutSec 5
            
            if ($response.result) {
                Write-Host "✅ Success - Block: $($response.result)" -ForegroundColor Green
            } else {
                Write-Host "❌ Failed" -ForegroundColor Red
            }
        } catch {
            Write-Host "❌ Error: $_" -ForegroundColor Red
        }
        
        Start-Sleep -Milliseconds $DelayMs
    }
}

# Function to send Comet RPC requests
function Test-CometRpc {
    Write-Host ""
    Write-Host "🌐 Testing Comet RPC endpoints..." -ForegroundColor Yellow
    
    for ($i = 1; $i -le $NumRequests; $i++) {
        Write-Host -NoNewline "Comet RPC Request ${i}: "
        
        try {
            $response = Invoke-RestMethod -Uri "$ProxyUrl/rpc/status" `
                -Method Get `
                -TimeoutSec 5
            
            if ($response.result.node_info) {
                Write-Host "✅ Success" -ForegroundColor Green
            } else {
                Write-Host "❌ Failed" -ForegroundColor Red
            }
        } catch {
            Write-Host "❌ Error: $_" -ForegroundColor Red
        }
        
        Start-Sleep -Milliseconds $DelayMs
    }
}

# Function to send mixed requests
function Test-Mixed {
    Write-Host ""
    Write-Host "🔀 Testing Mixed Requests..." -ForegroundColor Yellow
    
    for ($i = 1; $i -le $NumRequests; $i++) {
        if ($i % 2 -eq 0) {
            Write-Host -NoNewline "Request ${i} (JSON-RPC): "
            
            try {
                $body = @{
                    jsonrpc = "2.0"
                    method = "eth_blockNumber"
                    params = @()
                    id = 1
                } | ConvertTo-Json
                
                $response = Invoke-RestMethod -Uri "$ProxyUrl/json-rpc" `
                    -Method Post `
                    -ContentType "application/json" `
                    -Body $body `
                    -TimeoutSec 5
                
                if ($response.result) {
                    Write-Host "✅ Success" -ForegroundColor Green
                } else {
                    Write-Host "❌ Failed" -ForegroundColor Red
                }
            } catch {
                Write-Host "❌ Error" -ForegroundColor Red
            }
        } else {
            Write-Host -NoNewline "Request ${i} (Comet RPC): "
            
            try {
                $response = Invoke-RestMethod -Uri "$ProxyUrl/rpc/status" `
                    -Method Get `
                    -TimeoutSec 5
                
                if ($response.result.node_info) {
                    Write-Host "✅ Success" -ForegroundColor Green
                } else {
                    Write-Host "❌ Failed" -ForegroundColor Red
                }
            } catch {
                Write-Host "❌ Error" -ForegroundColor Red
            }
        }
        
        Start-Sleep -Milliseconds $DelayMs
    }
}

# Menu
Write-Host ""
Write-Host "Select test mode:" -ForegroundColor Cyan
Write-Host "1. JSON-RPC only"
Write-Host "2. Comet RPC only"
Write-Host "3. Mixed requests"
Write-Host "4. All of the above"
$choice = Read-Host "Choice [1-4]"

switch ($choice) {
    "1" {
        Test-JsonRpc
    }
    "2" {
        Test-CometRpc
    }
    "3" {
        Test-Mixed
    }
    "4" {
        Test-JsonRpc
        Test-CometRpc
        Test-Mixed
    }
    default {
        Write-Host "Invalid choice. Running mixed test by default..." -ForegroundColor Yellow
        Test-Mixed
    }
}

Write-Host ""
Write-Host "Load test complete!" -ForegroundColor Green
Write-Host ""
Write-Host "View results:" -ForegroundColor Cyan
Write-Host "  Dashboard: http://localhost:8080"
Write-Host "  Request History tab: http://localhost:8080"
Write-Host "  API Stats: http://localhost:8080/api/requests/stats"

