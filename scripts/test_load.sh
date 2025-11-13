#!/bin/bash

# Load Testing Script for Load-Aware Proxy
# This script sends multiple requests to test load balancing

PROXY_URL="http://localhost:8080"
NUM_REQUESTS=20
DELAY=0.2

echo "🚀 Starting Load Test..."
echo "Proxy: $PROXY_URL"
echo "Requests per endpoint: $NUM_REQUESTS"
echo "Delay between requests: ${DELAY}s"
echo ""

# Function to send JSON-RPC requests
test_json_rpc() {
    echo "📊 Testing JSON-RPC endpoints..."
    
    for i in $(seq 1 $NUM_REQUESTS); do
        echo -n "JSON-RPC Request $i: "
        RESPONSE=$(curl -s -X POST "$PROXY_URL/json-rpc" \
            -H "Content-Type: application/json" \
            -d '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}' \
            2>/dev/null)
        
        if [[ $RESPONSE == *"result"* ]]; then
            BLOCK=$(echo $RESPONSE | grep -oP '"result":"\K[^"]+')
            echo "✅ Success - Block: $BLOCK"
        else
            echo "❌ Failed"
        fi
        
        sleep $DELAY
    done
}

# Function to send Comet RPC requests
test_comet_rpc() {
    echo ""
    echo "🌐 Testing Comet RPC endpoints..."
    
    for i in $(seq 1 $NUM_REQUESTS); do
        echo -n "Comet RPC Request $i: "
        RESPONSE=$(curl -s "$PROXY_URL/rpc/status" 2>/dev/null)
        
        if [[ $RESPONSE == *"node_info"* ]]; then
            echo "✅ Success"
        else
            echo "❌ Failed"
        fi
        
        sleep $DELAY
    done
}

# Function to send mixed requests
test_mixed() {
    echo ""
    echo "🔀 Testing Mixed Requests..."
    
    for i in $(seq 1 $NUM_REQUESTS); do
        if [ $((i % 2)) -eq 0 ]; then
            echo -n "Request $i (JSON-RPC): "
            RESPONSE=$(curl -s -X POST "$PROXY_URL/json-rpc" \
                -H "Content-Type: application/json" \
                -d '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}' \
                2>/dev/null)
            
            if [[ $RESPONSE == *"result"* ]]; then
                echo "✅ Success"
            else
                echo "❌ Failed"
            fi
        else
            echo -n "Request $i (Comet RPC): "
            RESPONSE=$(curl -s "$PROXY_URL/rpc/status" 2>/dev/null)
            
            if [[ $RESPONSE == *"node_info"* ]]; then
                echo "✅ Success"
            else
                echo "❌ Failed"
            fi
        fi
        
        sleep $DELAY
    done
}

# Menu
echo "Select test mode:"
echo "1. JSON-RPC only"
echo "2. Comet RPC only"
echo "3. Mixed requests"
echo "4. All of the above"
echo -n "Choice [1-4]: "
read CHOICE

case $CHOICE in
    1)
        test_json_rpc
        ;;
    2)
        test_comet_rpc
        ;;
    3)
        test_mixed
        ;;
    4)
        test_json_rpc
        test_comet_rpc
        test_mixed
        ;;
    *)
        echo "Invalid choice. Running mixed test by default..."
        test_mixed
        ;;
esac

echo ""
echo "✅ Load test complete!"
echo ""
echo "📊 View results:"
echo "  Dashboard: http://localhost:8080"
echo "  Request History: http://localhost:8080 (Request History tab)"
echo "  API Stats: curl http://localhost:8080/api/requests/stats"

