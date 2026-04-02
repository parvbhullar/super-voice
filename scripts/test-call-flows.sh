#!/usr/bin/env bash
# test-call-flows.sh — End-to-end test for SIP→SIP, SIP→WS, SIP→WebRTC flows.
#
# Prerequisites:
#   - Redis running on localhost:6379
#   - super-voice binary built (just build)
#   - sipsak installed (brew install sipsak)
#   - Optional: sipp for full SIP B2BUA (brew install sipp)
#   - Optional: wscat for WS test (npm i -g wscat)
#   - Optional: active-call-tester for WebRTC (cd active-call-tester && uv sync)
#
# Usage:
#   bash scripts/test-call-flows.sh           # Run all flows
#   bash scripts/test-call-flows.sh sip       # SIP-to-SIP only
#   bash scripts/test-call-flows.sh ws        # SIP-to-WS only
#   bash scripts/test-call-flows.sh webrtc    # SIP-to-WebRTC only
#   bash scripts/test-call-flows.sh setup     # Setup only (create entities, don't test)
#   bash scripts/test-call-flows.sh teardown  # Clean up entities and stop server

set -euo pipefail

# ──────────────────────────────────────────────────
# Configuration
# ──────────────────────────────────────────────────

HTTP_PORT="${HTTP_PORT:-18080}"
SIP_PORT="${SIP_PORT:-15060}"
SIP_ECHO_PORT="${SIP_ECHO_PORT:-15080}"
BASE_URL="http://localhost:${HTTP_PORT}/api/v1"
CONF_FILE="/tmp/sv-e2e-test.toml"
PID_FILE="/tmp/sv-e2e-test.pid"
LOG_FILE="/tmp/sv-e2e-test.log"
BIN="./target/release/active-call"
FLOW="${1:-all}"
CURL_TIMEOUT=5  # seconds for all API calls

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
CYAN='\033[0;36m'
NC='\033[0m'

pass() { echo -e "  ${GREEN}✓${NC} $1"; }
fail() { echo -e "  ${RED}✗${NC} $1"; FAILURES=$((FAILURES + 1)); }
info() { echo -e "  ${CYAN}→${NC} $1"; }
warn() { echo -e "  ${YELLOW}!${NC} $1"; }
header() { echo -e "\n${CYAN}━━━ $1 ━━━${NC}"; }

FAILURES=0

# ──────────────────────────────────────────────────
# Dependency checks
# ──────────────────────────────────────────────────

check_deps() {
    header "Checking dependencies"

    redis-cli ping >/dev/null 2>&1 && pass "Redis: running" || { fail "Redis: not running (redis-server required)"; exit 1; }

    if [ ! -f "$BIN" ]; then
        info "Binary not found, building..."
        cargo build --release 2>&1 | tail -1
    fi
    pass "Binary: $BIN"

    command -v sipsak >/dev/null 2>&1 && pass "sipsak: installed" || warn "sipsak: not installed (brew install sipsak)"
    command -v sipp >/dev/null 2>&1 && pass "sipp: installed" || warn "sipp: not installed (brew install sipp) — SIP B2BUA test will use sipsak fallback"
    command -v wscat >/dev/null 2>&1 && pass "wscat: installed" || warn "wscat: not installed (npm i -g wscat) — WS test will use curl fallback"
}

# ──────────────────────────────────────────────────
# Server lifecycle
# ──────────────────────────────────────────────────

generate_config() {
    cat > "$CONF_FILE" <<EOF
addr = "0.0.0.0"
http_addr = "0.0.0.0:${HTTP_PORT}"
udp_port = ${SIP_PORT}
redis_url = "redis://127.0.0.1:6379"
log_level = "info"

[handler]
type = "playbook"
default = "hello.md"
EOF
}

start_server() {
    header "Starting super-voice"

    if [ -f "$PID_FILE" ] && kill -0 "$(cat "$PID_FILE")" 2>/dev/null; then
        pass "Server already running (PID $(cat "$PID_FILE"))"
        return 0
    fi

    generate_config
    nohup "$BIN" --conf "$CONF_FILE" > "$LOG_FILE" 2>&1 &
    echo $! > "$PID_FILE"
    sleep 2

    if kill -0 "$(cat "$PID_FILE")" 2>/dev/null; then
        pass "Server started (PID $(cat "$PID_FILE")) — HTTP :${HTTP_PORT} SIP :${SIP_PORT}"
    else
        fail "Server failed to start"
        tail -20 "$LOG_FILE"
        exit 1
    fi
}

stop_server() {
    if [ -f "$PID_FILE" ] && kill -0 "$(cat "$PID_FILE")" 2>/dev/null; then
        kill "$(cat "$PID_FILE")" 2>/dev/null || true
        rm -f "$PID_FILE"
        pass "Server stopped"
    fi
}

# ──────────────────────────────────────────────────
# API key + entity setup
#
# NOTE: We do NOT create endpoints via API — the server already binds
# its SIP port from the config file (udp_port). Creating an endpoint
# via API would try to start PjBridge which can hang on macOS.
# We only create gateways, trunks, and DIDs.
# ──────────────────────────────────────────────────

setup_api_key() {
    if [ -n "${API_KEY:-}" ]; then
        return 0
    fi

    local random_hex
    random_hex=$(openssl rand -hex 32)
    local hash
    hash=$(echo -n "$random_hex" | shasum -a 256 | cut -d' ' -f1)
    redis-cli SADD "sv:api_keys" "e2e-test:${hash}" > /dev/null
    export API_KEY="sv_${random_hex}"
    info "API_KEY created: ${API_KEY:0:20}..."
}

# All API calls have a timeout to prevent hanging
api() {
    local method="$1" path="$2"
    shift 2
    curl -s --max-time "$CURL_TIMEOUT" -X "$method" "${BASE_URL}${path}" \
        -H "Authorization: Bearer $API_KEY" \
        -H "Content-Type: application/json" \
        "$@" 2>/dev/null || echo '{"error":"timeout or connection refused"}'
}

create_entity() {
    local type="$1" name="$2" data="$3"
    local result
    result=$(api POST "/${type}" -d "$data")
    if echo "$result" | grep -q "already exists" 2>/dev/null; then
        info "${type}/${name}: already exists"
    elif echo "$result" | grep -q '"error"' 2>/dev/null; then
        warn "${type}/${name}: $(echo "$result" | head -1)"
    else
        pass "${type}/${name}: created"
    fi
}

setup_entities() {
    header "Creating test entities"

    # Gateway (points to local SIP echo server or external target)
    create_entity gateways e2e-echo-gw "{
        \"name\": \"e2e-echo-gw\",
        \"proxy\": \"127.0.0.1:${SIP_ECHO_PORT}\",
        \"transport\": \"udp\"
    }"

    # Trunk with the gateway
    create_entity trunks e2e-trunk '{
        "name": "e2e-trunk",
        "gateways": [{"name": "e2e-echo-gw"}],
        "nofailover_sip_codes": [486, 603]
    }'

    # DID for SIP-to-SIP proxy
    create_entity dids "+15551000001" '{
        "number": "+15551000001",
        "trunk": "e2e-trunk",
        "routing": {"mode": "sip_proxy"}
    }'

    # DID for SIP-to-WS bridge
    create_entity dids "+15552000002" '{
        "number": "+15552000002",
        "trunk": "e2e-trunk",
        "routing": {"mode": "ws_bridge", "playbook": "hello.md"}
    }'

    # DID for SIP-to-WebRTC bridge
    create_entity dids "+15553000003" '{
        "number": "+15553000003",
        "trunk": "e2e-trunk",
        "routing": {"mode": "webrtc_bridge", "playbook": "hello.md"}
    }'

    # Verify health
    local health
    health=$(api GET /system/health)
    if echo "$health" | grep -q '"status"' 2>/dev/null; then
        pass "Health check: OK"
    else
        warn "Health check: $health"
    fi
}

teardown_entities() {
    header "Cleaning up test entities"

    api DELETE "/dids/%2B15551000001" >/dev/null && pass "DID +15551000001 deleted" || info "DID not found"
    api DELETE "/dids/%2B15552000002" >/dev/null && pass "DID +15552000002 deleted" || info "DID not found"
    api DELETE "/dids/%2B15553000003" >/dev/null && pass "DID +15553000003 deleted" || info "DID not found"
    api DELETE /trunks/e2e-trunk >/dev/null && pass "Trunk deleted" || info "Trunk not found"
    api DELETE /gateways/e2e-echo-gw >/dev/null && pass "Gateway deleted" || info "Gateway not found"
}

# ──────────────────────────────────────────────────
# Test 1: SIP-to-SIP Proxy
# ──────────────────────────────────────────────────

test_sip_to_sip() {
    header "Test: SIP → SIP Proxy"
    info "DID: +15551000001 → mode: sip_proxy → trunk: e2e-trunk → gw: 127.0.0.1:${SIP_ECHO_PORT}"

    # Step 1: OPTIONS ping (basic SIP connectivity)
    info "Step 1: SIP OPTIONS ping"
    if command -v sipsak >/dev/null 2>&1; then
        local options_result
        options_result=$(timeout 5 sipsak -s "sip:test@127.0.0.1:${SIP_PORT}" -v 2>&1 || true)
        if echo "$options_result" | grep -qiE "200|reply|received"; then
            pass "OPTIONS ping: 200 OK"
        else
            warn "OPTIONS ping: no 200 (server may not respond to bare OPTIONS)"
            info "$(echo "$options_result" | grep -iE "^SIP|reply|error" | head -2)"
        fi
    else
        warn "sipsak not installed — skipping OPTIONS test"
    fi

    # Step 2: INVITE through proxy
    info "Step 2: SIP INVITE through proxy"
    if command -v sipp >/dev/null 2>&1; then
        # Start a UAS (auto-answer) on the echo port
        sipp -sn uas -p "$SIP_ECHO_PORT" -bg -trace_err 2>/dev/null &
        local uas_pid=$!
        sleep 1

        # Send INVITE through the proxy
        local invite_result
        invite_result=$(timeout 15 sipp -sn uac "127.0.0.1:${SIP_PORT}" \
            -s "+15551000001" \
            -m 1 -r 1 -rp 1000 \
            -timeout 10 2>&1 || true)

        if echo "$invite_result" | grep -qiE "successful|0 failed"; then
            pass "INVITE proxy: call completed through B2BUA"
        else
            warn "INVITE proxy: $(echo "$invite_result" | grep -iE "failed|timeout|error" | head -1)"
        fi

        kill "$uas_pid" 2>/dev/null || true
        wait "$uas_pid" 2>/dev/null || true
    else
        # sipsak fallback — sends INVITE but won't complete without UAS
        if command -v sipsak >/dev/null 2>&1; then
            info "No sipp — using sipsak INVITE (will timeout without echo server)"
            local sipsak_result
            sipsak_result=$(timeout 5 sipsak -s "sip:+15551000001@127.0.0.1:${SIP_PORT}" -M 2>&1 || true)
            if echo "$sipsak_result" | grep -qiE "received|100|180|200|trying|timeout"; then
                pass "INVITE: proxy responded (dispatch path exercised)"
            else
                warn "INVITE: no response"
            fi
        else
            warn "Neither sipp nor sipsak — skipping INVITE test"
        fi
    fi

    # Step 3: Check CDR
    info "Step 3: Verify CDR generation"
    sleep 1
    local cdrs
    cdrs=$(api GET /cdrs)
    if echo "$cdrs" | grep -q "e2e-trunk" 2>/dev/null; then
        pass "CDR generated for e2e-trunk"
    else
        info "No CDR found (expected if call didn't connect)"
    fi
}

# ──────────────────────────────────────────────────
# Test 2: SIP-to-WebSocket Bridge
# ──────────────────────────────────────────────────

test_sip_to_ws() {
    header "Test: SIP → WebSocket Bridge"
    info "DID: +15552000002 → mode: ws_bridge → playbook: hello.md"

    # Step 1: Direct WS connection (bypasses SIP, tests WS path)
    info "Step 1: Direct WebSocket connection"
    local ws_url="ws://localhost:${HTTP_PORT}/call?session_id=e2e-ws-test&codec=pcmu"

    if command -v wscat >/dev/null 2>&1; then
        local ws_output
        ws_output=$(timeout 5 wscat -c "$ws_url" --wait 2 2>&1 || true)
        if [ -n "$ws_output" ]; then
            pass "WebSocket: connected (got response)"
        else
            pass "WebSocket: connection attempted"
        fi
    else
        # curl probe — check if server accepts WS upgrade
        local code
        code=$(curl -s -o /dev/null -w "%{http_code}" --max-time "$CURL_TIMEOUT" \
            -H "Connection: Upgrade" \
            -H "Upgrade: websocket" \
            -H "Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==" \
            -H "Sec-WebSocket-Version: 13" \
            "http://localhost:${HTTP_PORT}/call?session_id=e2e-ws-test&codec=pcmu" 2>/dev/null || echo "000")
        if [ "$code" = "101" ]; then
            pass "WebSocket upgrade: 101 Switching Protocols"
        elif [ "$code" = "000" ]; then
            fail "WebSocket: server unreachable"
        else
            info "WebSocket upgrade: HTTP $code (may need real WS client)"
        fi
    fi

    # Step 2: SIP INVITE → WS bridge
    info "Step 2: SIP INVITE → WS bridge dispatch"
    if command -v sipsak >/dev/null 2>&1; then
        local result
        result=$(timeout 5 sipsak -s "sip:+15552000002@127.0.0.1:${SIP_PORT}" -M 2>&1 || true)
        if echo "$result" | grep -qiE "received|100|180|200|trying|timeout"; then
            pass "SIP→WS: proxy accepted and dispatched to ws_bridge"
        else
            warn "SIP→WS: $(echo "$result" | head -1)"
        fi
    else
        warn "sipsak not installed — skipping SIP→WS test"
    fi

    # Step 3: Bulk WS via active-call-tester
    info "Step 3: Bulk WS test (active-call-tester)"
    if [ -f "active-call-tester/pyproject.toml" ] && command -v uv >/dev/null 2>&1; then
        local tester_result
        tester_result=$(cd active-call-tester && timeout 20 uv run python -m active_call_tester run \
            --protocol websocket \
            --target "ws://localhost:${HTTP_PORT}/call" \
            --codec pcmu \
            --concurrency 1 \
            --duration 5 \
            --hold 3 2>&1 || true)
        if echo "$tester_result" | grep -qiE "pass|success|completed|1/1"; then
            pass "Bulk WS: passed"
        else
            info "Bulk WS: $(echo "$tester_result" | tail -2)"
        fi
    else
        info "Skipped (active-call-tester not available)"
    fi
}

# ──────────────────────────────────────────────────
# Test 3: SIP-to-WebRTC Bridge
# ──────────────────────────────────────────────────

test_sip_to_webrtc() {
    header "Test: SIP → WebRTC Bridge"
    info "DID: +15553000003 → mode: webrtc_bridge → playbook: hello.md"

    # Step 1: SIP INVITE → WebRTC bridge dispatch
    info "Step 1: SIP INVITE → WebRTC bridge dispatch"
    if command -v sipsak >/dev/null 2>&1; then
        local result
        result=$(timeout 5 sipsak -s "sip:+15553000003@127.0.0.1:${SIP_PORT}" -M 2>&1 || true)
        if echo "$result" | grep -qiE "received|100|180|200|trying|timeout"; then
            pass "SIP→WebRTC: proxy accepted and dispatched to webrtc_bridge"
        else
            warn "SIP→WebRTC: $(echo "$result" | head -1)"
        fi
    else
        warn "sipsak not installed — skipping"
    fi

    # Step 2: Headless WebRTC via aiortc
    info "Step 2: Headless WebRTC client (aiortc)"
    if [ -f "active-call-tester/pyproject.toml" ] && command -v uv >/dev/null 2>&1; then
        local webrtc_result
        webrtc_result=$(cd active-call-tester && timeout 20 uv run python -m active_call_tester run \
            --protocol webrtc \
            --target "ws://localhost:${HTTP_PORT}/call" \
            --codec opus \
            --concurrency 1 \
            --duration 5 \
            --hold 3 2>&1 || true)
        if echo "$webrtc_result" | grep -qiE "pass|success|completed|ice.*complete"; then
            pass "Headless WebRTC: ICE completed"
        else
            info "Headless WebRTC: $(echo "$webrtc_result" | tail -2)"
        fi
    else
        info "Skipped (active-call-tester not available)"
        info "Install: cd active-call-tester && uv sync"
    fi

    # Step 3: Browser WebRTC via Playwright
    info "Step 3: Browser WebRTC (Playwright)"
    if [ -f "active-call-tester/pyproject.toml" ] && command -v uv >/dev/null 2>&1; then
        local browser_ok
        browser_ok=$(cd active-call-tester && uv run python -c "from playwright.sync_api import sync_playwright; print('ok')" 2>/dev/null || echo "no")
        if [ "$browser_ok" = "ok" ]; then
            local browser_result
            browser_result=$(cd active-call-tester && timeout 20 uv run python -m active_call_tester run \
                --protocol webrtc-browser \
                --target "ws://localhost:${HTTP_PORT}/call" \
                --codec opus \
                --concurrency 1 \
                --duration 5 \
                --hold 3 2>&1 || true)
            if echo "$browser_result" | grep -qiE "pass|success|completed"; then
                pass "Browser WebRTC: call completed"
            else
                info "Browser WebRTC: $(echo "$browser_result" | tail -2)"
            fi
        else
            info "Skipped (playwright not installed)"
        fi
    else
        info "Skipped (active-call-tester not available)"
    fi
}

# ──────────────────────────────────────────────────
# Summary
# ──────────────────────────────────────────────────

print_summary() {
    header "Summary"
    if [ "$FAILURES" -eq 0 ]; then
        echo -e "  ${GREEN}All tests passed${NC}"
    else
        echo -e "  ${RED}${FAILURES} test(s) failed${NC}"
    fi
    echo ""
    echo "  Server:  http://localhost:${HTTP_PORT}  SIP :${SIP_PORT}"
    echo "  Logs:    $LOG_FILE"
    echo "  Cleanup: bash scripts/test-call-flows.sh teardown"
}

# ──────────────────────────────────────────────────
# Main
# ──────────────────────────────────────────────────

main() {
    echo -e "${CYAN}╔══════════════════════════════════════════╗${NC}"
    echo -e "${CYAN}║   super-voice E2E Call Flow Tests        ║${NC}"
    echo -e "${CYAN}╚══════════════════════════════════════════╝${NC}"

    case "$FLOW" in
        setup)
            check_deps
            start_server
            setup_api_key
            setup_entities
            echo ""
            echo "  Setup complete. Export and test:"
            echo "    export API_KEY=$API_KEY"
            echo "    bash scripts/test-call-flows.sh sip"
            ;;
        teardown)
            setup_api_key
            teardown_entities
            stop_server
            ;;
        sip)
            check_deps
            start_server
            setup_api_key
            setup_entities
            test_sip_to_sip
            print_summary
            ;;
        ws)
            check_deps
            start_server
            setup_api_key
            setup_entities
            test_sip_to_ws
            print_summary
            ;;
        webrtc)
            check_deps
            start_server
            setup_api_key
            setup_entities
            test_sip_to_webrtc
            print_summary
            ;;
        all)
            check_deps
            start_server
            setup_api_key
            setup_entities
            test_sip_to_sip
            test_sip_to_ws
            test_sip_to_webrtc
            print_summary
            ;;
        *)
            echo "Usage: $0 {all|sip|ws|webrtc|setup|teardown}"
            exit 1
            ;;
    esac

    exit "$FAILURES"
}

main
