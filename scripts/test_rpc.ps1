$ProxyUrl = "http://localhost:8080"
Write-Host "Testing Comet RPC endpoints - 20 requests" -ForegroundColor Cyan
Write-Host ""

for ($i = 1; $i -le 20; $i++) {
    Write-Host -NoNewline "Request ${i}: "
    try {
        $response = Invoke-RestMethod -Uri "${ProxyUrl}/rpc/status" -Method Get -TimeoutSec 5
        if ($response.result.node_info) {
            Write-Host "Success" -ForegroundColor Green
        } else {
            Write-Host "Failed" -ForegroundColor Red
        }
    } catch {
        Write-Host "Error" -ForegroundColor Red
    }
    Start-Sleep -Milliseconds 200
}

Write-Host ""
Write-Host "Test complete! Check dashboard at http://localhost:8080" -ForegroundColor Green

