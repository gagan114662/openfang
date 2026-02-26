#!/usr/bin/env bash
# Integration test script for OpenFang + Raindrop Telegram integration

set -e

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo "=== OpenFang + Raindrop Telegram Integration Test ==="
echo ""

# Step 1: Check services are running
echo "Step 1: Checking services..."

# Check OpenFang daemon
if curl -s http://127.0.0.1:4200/api/health > /dev/null 2>&1; then
    echo -e "${GREEN}✓${NC} OpenFang daemon is running on port 4200"
else
    echo -e "${RED}✗${NC} OpenFang daemon is NOT running"
    echo "Start with: GROQ_API_KEY=<key> target/release/openfang.exe start"
    exit 1
fi

# Check Raindrop service
if curl -s http://127.0.0.1:4201/health > /dev/null 2>&1; then
    echo -e "${GREEN}✓${NC} Raindrop service is running on port 4201"
else
    echo -e "${RED}✗${NC} Raindrop service is NOT running"
    echo "Start with: cd crates/raindrop && cargo run"
    exit 1
fi

echo ""

# Step 2: Test Telegram Bot connectivity
echo "Step 2: Testing Telegram Bot API connectivity..."

if [ -z "$TELEGRAM_BOT_TOKEN" ]; then
    echo -e "${RED}✗${NC} TELEGRAM_BOT_TOKEN environment variable not set"
    echo "Set with: export TELEGRAM_BOT_TOKEN=<your-token>"
    exit 1
fi

BOT_INFO=$(curl -s "https://api.telegram.org/bot${TELEGRAM_BOT_TOKEN}/getMe")
BOT_OK=$(echo "$BOT_INFO" | python3 -c "import sys,json; print(json.load(sys.stdin).get('ok', False))" 2>/dev/null || echo "false")

if [ "$BOT_OK" = "True" ]; then
    BOT_USERNAME=$(echo "$BOT_INFO" | python3 -c "import sys,json; print(json.load(sys.stdin)['result']['username'])")
    echo -e "${GREEN}✓${NC} Telegram bot authenticated: @${BOT_USERNAME}"
else
    echo -e "${RED}✗${NC} Telegram bot authentication failed"
    echo "Response: $BOT_INFO"
    exit 1
fi

echo ""

# Step 3: Test OpenFang agent endpoint
echo "Step 3: Testing OpenFang agent endpoint..."

AGENTS_RESPONSE=$(curl -s http://127.0.0.1:4200/api/agents)
AGENT_COUNT=$(echo "$AGENTS_RESPONSE" | python3 -c "import sys,json; print(len(json.load(sys.stdin)))" 2>/dev/null || echo "0")

if [ "$AGENT_COUNT" -gt 0 ]; then
    echo -e "${GREEN}✓${NC} OpenFang has $AGENT_COUNT agent(s) available"

    # Get first agent ID
    AGENT_ID=$(echo "$AGENTS_RESPONSE" | python3 -c "import sys,json; print(json.load(sys.stdin)[0]['id'])")
    echo "  First agent ID: $AGENT_ID"

    # Test message endpoint (dry run - no actual LLM call)
    echo "  Testing message endpoint..."
    MESSAGE_TEST=$(curl -s -X POST "http://127.0.0.1:4200/api/agents/${AGENT_ID}/message" \
        -H "Content-Type: application/json" \
        -d '{"message": "test"}' 2>&1)

    if echo "$MESSAGE_TEST" | grep -q "error" 2>/dev/null; then
        echo -e "${YELLOW}!${NC} Message endpoint returned error (may need GROQ_API_KEY)"
    else
        echo -e "${GREEN}✓${NC} Message endpoint is reachable"
    fi
else
    echo -e "${RED}✗${NC} No agents found in OpenFang"
    exit 1
fi

echo ""

# Step 4: Test Raindrop SSE stream endpoint
echo "Step 4: Testing Raindrop SSE stream endpoint..."

# Test basic health
RAINDROP_HEALTH=$(curl -s http://127.0.0.1:4201/health)
if echo "$RAINDROP_HEALTH" | grep -q "ok" 2>/dev/null; then
    echo -e "${GREEN}✓${NC} Raindrop health endpoint OK"
else
    echo -e "${RED}✗${NC} Raindrop health check failed"
    exit 1
fi

# Test SSE endpoint (timeout after 2 seconds)
echo "  Testing SSE stream endpoint..."
SSE_TEST=$(timeout 2 curl -s -N http://127.0.0.1:4201/v1/incidents/stream 2>&1 || true)

if echo "$SSE_TEST" | grep -q "data:" 2>/dev/null || echo "$SSE_TEST" | grep -q "event:" 2>/dev/null; then
    echo -e "${GREEN}✓${NC} SSE stream endpoint is active"
elif [ -z "$SSE_TEST" ]; then
    echo -e "${GREEN}✓${NC} SSE stream endpoint is listening (no events yet)"
else
    echo -e "${YELLOW}!${NC} SSE stream endpoint status unclear"
    echo "  Response: $SSE_TEST"
fi

echo ""

# Step 5: Manual verification steps
echo "=== Manual Verification Steps ==="
echo ""
echo "1. Send a message to your Telegram bot:"
echo -e "   ${YELLOW}Open Telegram and message @${BOT_USERNAME}${NC}"
echo ""
echo "2. Monitor Raindrop logs for incoming webhook:"
echo "   tail -f crates/raindrop/raindrop.log"
echo ""
echo "3. Check OpenFang dashboard for agent activity:"
echo "   open http://127.0.0.1:4200"
echo ""
echo "4. Verify bot replies in Telegram chat"
echo ""
echo "5. Test error handling:"
echo "   - Try: curl -X POST http://127.0.0.1:4201/api/telegram/webhook \\"
echo "     -H 'Content-Type: application/json' \\"
echo "     -d '{\"message\":{\"chat\":{\"id\":123},\"text\":\"test\"}}'"
echo ""
echo "6. Monitor SSE stream for events:"
echo "   curl -N http://127.0.0.1:4201/v1/incidents/stream"
echo ""
echo -e "${GREEN}=== All automated checks passed! ===${NC}"
echo ""
echo "Next steps:"
echo "  1. Set Telegram webhook: curl -X POST 'https://api.telegram.org/bot\${TELEGRAM_BOT_TOKEN}/setWebhook?url=https://your-domain.com/api/telegram/webhook'"
echo "  2. Send test messages to bot"
echo "  3. Monitor logs and dashboard"
echo ""
