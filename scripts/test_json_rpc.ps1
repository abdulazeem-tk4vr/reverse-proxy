$ProxyUrl = "http://localhost:8080/json-rpc"
Write-Host "Testing JSON-RPC endpoints - 20 requests" -ForegroundColor Cyan
Write-Host ""

$body = @{
    jsonrpc = "2.0"
    method = "eth_blockNumber"
    params = @()
    id = 1
} | ConvertTo-Json

for ($i = 1; $i -le 20; $i++) {
    Write-Host -NoNewline "Request ${i}: "
    try {
        $response = Invoke-RestMethod -Uri $ProxyUrl -Method Post -Body $body -ContentType "application/json" -TimeoutSec 10
        if ($response.result) {
            $blockNum = [Convert]::ToInt64($response.result, 16)
            Write-Host "Success (Block: $blockNum)" -ForegroundColor Green
        } elseif ($response.error) {
            Write-Host "Error: $($response.error.message)" -ForegroundColor Yellow
        } else {
            Write-Host "Unknown response" -ForegroundColor Red
        }
    } catch {
        Write-Host "Failed: $_" -ForegroundColor Red
    }
    Start-Sleep -Milliseconds 200
}

Write-Host ""
Write-Host "Test complete! Check dashboard at http://localhost:8080" -ForegroundColor Green

