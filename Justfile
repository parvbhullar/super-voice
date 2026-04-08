# Root Justfile for Super Voice

set dotenv-load

server_bin := justfile_directory() / "target/release/active-call"
carrier_conf := "/tmp/sv-carrier-test.toml"
server_pid := "/tmp/active-call-test.pid"
server_log := "/tmp/active-call-test.log"
default_port := "18080"
default_sip_port := "15060"
ort_version := "1.23.2"

# ──────────────────────────────────────────────
# Setup / Install
# ──────────────────────────────────────────────

# Full setup (mirrors install.sh)
install: install-deps install-rust install-just install-onnx install-pjsip install-redis install-sipbot build gen-config
    @echo "\n\033[0;32mInstallation complete!\033[0m"
    @echo "Run the server:"
    @echo "  ./target/release/active-call --conf my-config.toml"

# Install system packages (apt on Linux, brew on macOS)
install-deps:
    #!/usr/bin/env bash
    set -euo pipefail
    case "$(uname -s)" in
        Darwin)
            if ! command -v brew &>/dev/null; then
                echo "ERROR: Homebrew not found. Install from https://brew.sh" >&2
                exit 1
            fi
            echo "==> Installing system dependencies (macOS/Homebrew)..."
            brew install pkg-config cmake openssl speex speexdsp libogg opus srtp libsndfile
            ;;
        Linux)
            echo "==> Installing system dependencies (Linux/apt)..."
            sudo apt-get update
            sudo apt-get install -y \
                build-essential curl git pkg-config cmake \
                clang libclang-dev \
                libssl-dev \
                libspeex-dev libspeexdsp-dev \
                libogg-dev libopus-dev \
                libsrtp2-dev \
                libasound2-dev
            ;;
        *)
            echo "ERROR: Unsupported OS: $(uname -s)" >&2
            exit 1
            ;;
    esac

# Install Rust via rustup (skip if present)
install-rust:
    #!/usr/bin/env bash
    set -euo pipefail
    if command -v rustc &>/dev/null; then
        echo "[skip] Rust already installed ($(rustc --version))"
    else
        echo "==> Installing Rust..."
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path
        echo "Rust installed. Run: source \$HOME/.cargo/env"
    fi

# Install just task runner (skip if present)
install-just:
    #!/usr/bin/env bash
    set -euo pipefail
    if command -v just &>/dev/null; then
        echo "[skip] just already installed ($(just --version))"
    else
        echo "==> Installing just..."
        case "$(uname -s)" in
            Darwin)
                brew install just
                ;;
            Linux)
                if curl -fsSL https://just.systems/install.sh | sudo bash -s -- --to /usr/local/bin 2>/dev/null; then
                    echo "just installed via prebuilt binary"
                else
                    echo "[warn] Prebuilt install failed, falling back to cargo install just..."
                    cargo install just
                fi
                ;;
        esac
    fi

# Install ONNX Runtime (brew on macOS, GitHub release on Linux)
install-onnx:
    #!/usr/bin/env bash
    set -euo pipefail
    case "$(uname -s)" in
        Darwin)
            if brew list onnxruntime &>/dev/null; then
                echo "[skip] onnxruntime already installed via Homebrew"
            else
                echo "==> Installing ONNX Runtime (Homebrew)..."
                brew install onnxruntime
            fi
            ;;
        Linux)
            if [ -f /usr/local/lib/libonnxruntime.so ]; then
                echo "[skip] libonnxruntime.so already installed"
            else
                echo "==> Installing ONNX Runtime {{ort_version}} (GitHub release)..."
                TMP_DIR=$(mktemp -d)
                curl -fsSL "https://github.com/microsoft/onnxruntime/releases/download/v{{ort_version}}/onnxruntime-linux-x64-{{ort_version}}.tgz" \
                    | tar -xz -C "$TMP_DIR"
                sudo cp "$TMP_DIR"/onnxruntime-linux-x64-*/lib/libonnxruntime.so* /usr/local/lib/
                sudo ldconfig
                rm -rf "$TMP_DIR"
                echo "libonnxruntime.so {{ort_version}}: installed"
            fi
            ;;
    esac

# Install pjproject from source (both platforms)
install-pjsip:
    #!/usr/bin/env bash
    set -euo pipefail
    if pkg-config --exists libpjproject 2>/dev/null; then
        echo "[skip] pjproject already installed ($(pkg-config --modversion libpjproject))"
    else
        echo "==> Installing pjproject..."
        bash scripts/install-pjproject.sh
    fi
    echo "pjproject: $(pkg-config --modversion libpjproject)"

# Install Redis (apt on Linux, brew on macOS)
install-redis:
    #!/usr/bin/env bash
    set -euo pipefail
    case "$(uname -s)" in
        Darwin)
            if command -v redis-server &>/dev/null; then
                echo "[skip] Redis already installed"
            else
                echo "==> Installing Redis (Homebrew)..."
                brew install redis
            fi
            brew services start redis 2>/dev/null || true
            redis-cli ping | grep -q PONG && echo "Redis: OK" || echo "[warn] Redis ping failed"
            ;;
        Linux)
            if command -v redis-server &>/dev/null; then
                echo "[skip] Redis already installed"
            else
                echo "==> Installing Redis (apt)..."
                sudo apt-get install -y redis-server
            fi
            # Enable AOF persistence
            if ! grep -q "^appendonly yes" /etc/redis/redis.conf 2>/dev/null; then
                sudo sed -i 's/^# appendonly no/appendonly yes/' /etc/redis/redis.conf || true
                sudo sed -i 's/^appendonly no/appendonly yes/' /etc/redis/redis.conf || true
                echo "[info] Enabled Redis AOF persistence"
            fi
            sudo systemctl enable redis-server &>/dev/null || true
            sudo systemctl restart redis-server
            redis-cli ping | grep -q PONG && echo "Redis: OK" || echo "[warn] Redis ping failed"
            ;;
    esac

# Install sipbot SIP test tool (skip if present)
install-sipbot:
    #!/usr/bin/env bash
    set -euo pipefail
    if command -v sipbot &>/dev/null; then
        echo "[skip] sipbot already installed"
    else
        echo "==> Installing sipbot..."
        cargo install sipbot
    fi

# Generate default my-config.toml if missing
gen-config:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ -f my-config.toml ]; then
        echo "[skip] my-config.toml already exists"
    else
        echo "==> Creating default my-config.toml..."
        cat > my-config.toml << 'EOF'
    addr = "0.0.0.0"
    http_addr = "0.0.0.0:8080"
    udp_port = 5060
    redis_url = "redis://127.0.0.1:6379"
    log_level = "info"
    api_keys = ["change-me"]

    [handler]
    type = "playbook"
    default = "hello.md"
    EOF
        echo "[warn] Created my-config.toml — change api_keys before production!"
    fi

# ──────────────────────────────────────────────
# Build
# ──────────────────────────────────────────────

# Build release binary with carrier features
build:
    cargo build --release

# Build without C dependencies (pure Rust)
build-minimal:
    cargo build --release --no-default-features

# Check compilation (both feature paths)
check:
    cargo check --features carrier
    cargo check --no-default-features

# ──────────────────────────────────────────────
# Test
# ──────────────────────────────────────────────

# Run all tests (requires Redis + Sofia-SIP + SpanDSP)
test:
    cargo test --features carrier

# Run carrier E2E test suite (17 tests)
test-e2e:
    cargo test --test carrier_e2e_test --features carrier -- --nocapture

# Run carrier integration tests only
test-carrier:
    cargo test --features carrier --test carrier_integration -- --nocapture

# Run tests without C deps
test-minimal:
    cargo test --no-default-features

# Run specific test module
test-mod mod:
    cargo test --features carrier --lib {{mod}} -- --nocapture

# Validate startup time (<1s)
test-startup: build
    bash scripts/check_startup.sh {{server_bin}}

# ──────────────────────────────────────────────
# Server (Carrier Mode with Redis)
# ──────────────────────────────────────────────

# Generate carrier test config
_gen-config:
    #!/usr/bin/env bash
    cat > {{carrier_conf}} << 'EOF'
    addr = "0.0.0.0"
    http_addr = "0.0.0.0:{{default_port}}"
    udp_port = {{default_sip_port}}
    redis_url = "redis://127.0.0.1:6379"
    log_level = "info"

    [handler]
    type = "playbook"
    default = "hello.md"
    EOF
    echo "Config: {{carrier_conf}}"

# Start server in carrier mode (with Redis)
start: build _gen-config
    #!/usr/bin/env bash
    if [ -f {{server_pid}} ] && kill -0 $(cat {{server_pid}}) 2>/dev/null; then
        echo "Server already running (PID $(cat {{server_pid}}))"
        exit 0
    fi
    nohup {{server_bin}} --conf {{carrier_conf}} > {{server_log}} 2>&1 &
    echo $! > {{server_pid}}
    sleep 2
    if kill -0 $(cat {{server_pid}}) 2>/dev/null; then
        echo "Server started (PID $(cat {{server_pid}}))"
        echo "  HTTP: http://localhost:{{default_port}}"
        echo "  SIP:  sip:*:{{default_sip_port}}"
        echo "  Log:  {{server_log}}"
    else
        echo "FAILED. Log:"
        tail -20 {{server_log}}
        exit 1
    fi

# Stop the server
stop:
    #!/usr/bin/env bash
    if [ -f {{server_pid}} ] && kill -0 $(cat {{server_pid}}) 2>/dev/null; then
        kill $(cat {{server_pid}})
        rm -f {{server_pid}}
        echo "Server stopped"
    else
        echo "Server not running"
        rm -f {{server_pid}}
    fi

# Restart the server
restart: stop start

# Show server status + health
status:
    #!/usr/bin/env bash
    if [ -f {{server_pid}} ] && kill -0 $(cat {{server_pid}}) 2>/dev/null; then
        echo "Server: running (PID $(cat {{server_pid}}))"
    else
        echo "Server: not running"
    fi
    echo ""
    if [ -n "$API_KEY" ]; then
        curl -s -H "Authorization: Bearer $API_KEY" http://localhost:{{default_port}}/api/v1/system/health 2>/dev/null | python3 -m json.tool || echo "API unreachable (carrier)"
    else
        curl -s http://localhost:{{default_port}}/list 2>/dev/null | python3 -c "import sys,json; d=json.load(sys.stdin); print(f'Active calls: {len(d)}')" || echo "HTTP unreachable"
    fi

# View server log
log lines="50":
    @tail -n {{lines}} {{server_log}} 2>/dev/null || echo "No log"

# Follow server log (live)
tail:
    @tail -f {{server_log}}

# ──────────────────────────────────────────────
# API Key Management
# ──────────────────────────────────────────────

# Create a new API key and print it
create-key name="default":
    #!/usr/bin/env bash
    RANDOM_HEX=$(openssl rand -hex 32)
    HASH=$(echo -n "$RANDOM_HEX" | shasum -a 256 | cut -d' ' -f1)
    redis-cli SADD "sv:api_keys" "{{name}}:${HASH}" > /dev/null
    API_KEY="sv_${RANDOM_HEX}"
    echo "API_KEY=${API_KEY}"
    echo ""
    echo "Export it:"
    echo "  export API_KEY=${API_KEY}"

# List API key names
list-keys:
    @redis-cli SMEMBERS "sv:api_keys" 2>/dev/null | sed 's/:.*//' || echo "Redis unavailable"

# ──────────────────────────────────────────────
# Carrier API Quick Tests
# ──────────────────────────────────────────────

# Health check (requires $API_KEY)
health:
    @curl -s -H "Authorization: Bearer $API_KEY" http://localhost:{{default_port}}/api/v1/system/health | python3 -m json.tool

# System info
info:
    @curl -s -H "Authorization: Bearer $API_KEY" http://localhost:{{default_port}}/api/v1/system/info | python3 -m json.tool

# List all entities
entities:
    #!/usr/bin/env bash
    BASE="http://localhost:{{default_port}}/api/v1"
    AUTH="Authorization: Bearer $API_KEY"
    echo "Endpoints: $(curl -s -H "$AUTH" $BASE/endpoints 2>/dev/null | python3 -c 'import sys,json; d=json.load(sys.stdin); print(len(d) if isinstance(d,list) else d)' 2>/dev/null || echo 'error')"
    echo "Gateways:  $(curl -s -H "$AUTH" $BASE/gateways 2>/dev/null | python3 -c 'import sys,json; d=json.load(sys.stdin); print(len(d) if isinstance(d,list) else d)' 2>/dev/null || echo 'error')"
    echo "Trunks:    $(curl -s -H "$AUTH" $BASE/trunks 2>/dev/null | python3 -c 'import sys,json; d=json.load(sys.stdin); print(len(d) if isinstance(d,list) else d)' 2>/dev/null || echo 'error')"
    echo "DIDs:      $(curl -s -H "$AUTH" $BASE/dids 2>/dev/null | python3 -c 'import sys,json; d=json.load(sys.stdin); print(len(d) if isinstance(d,list) else d)' 2>/dev/null || echo 'error')"
    echo "Routes:    $(curl -s -H "$AUTH" $BASE/routing/tables 2>/dev/null | python3 -c 'import sys,json; d=json.load(sys.stdin); print(len(d) if isinstance(d,list) else d)' 2>/dev/null || echo 'error')"
    echo "Webhooks:  $(curl -s -H "$AUTH" $BASE/webhooks 2>/dev/null | python3 -c 'import sys,json; d=json.load(sys.stdin); print(len(d) if isinstance(d,list) else d)' 2>/dev/null || echo 'error')"

# Diagnostics summary
diagnostics:
    @curl -s -H "Authorization: Bearer $API_KEY" http://localhost:{{default_port}}/api/v1/diagnostics/summary | python3 -m json.tool

# Active calls
calls:
    @curl -s -H "Authorization: Bearer $API_KEY" http://localhost:{{default_port}}/api/v1/calls | python3 -m json.tool

# ──────────────────────────────────────────────
# SIP Load Testing
# ──────────────────────────────────────────────

# Send N SIP OPTIONS pings
sip-ping count="10":
    #!/usr/bin/env bash
    echo "Sending {{count}} SIP OPTIONS to 127.0.0.1:{{default_sip_port}}..."
    for i in $(seq 1 {{count}}); do
        sipsak -s sip:test@127.0.0.1:{{default_sip_port}} 2>/dev/null &
    done
    wait
    echo "Done: {{count}} OPTIONS sent"

# Send N concurrent SIP INVITEs
sip-flood count="50":
    #!/usr/bin/env bash
    echo "Sending {{count}} SIP INVITEs to 127.0.0.1:{{default_sip_port}}..."
    for i in $(seq 1 {{count}}); do
        sipsak -s sip:+14155551234@127.0.0.1:{{default_sip_port}} -M -C sip:test${i}@127.0.0.1 2>/dev/null &
    done
    wait
    echo "Done: {{count}} INVITEs sent"
    echo ""
    if [ -n "$API_KEY" ]; then
        curl -s -H "Authorization: Bearer $API_KEY" http://localhost:{{default_port}}/api/v1/system/health | python3 -m json.tool
    fi

# ──────────────────────────────────────────────
# Full E2E Pipelines
# ──────────────────────────────────────────────

# Quick smoke: build + start + health check + SIP ping
smoke: start
    #!/usr/bin/env bash
    sleep 1
    echo "── Health Check ──"
    if [ -z "$API_KEY" ]; then
        echo "No API_KEY set. Creating one..."
        RANDOM_HEX=$(openssl rand -hex 32)
        HASH=$(echo -n "$RANDOM_HEX" | shasum -a 256 | cut -d' ' -f1)
        redis-cli SADD "sv:api_keys" "smoke:${HASH}" > /dev/null
        export API_KEY="sv_${RANDOM_HEX}"
        echo "API_KEY=$API_KEY"
    fi
    curl -s -H "Authorization: Bearer $API_KEY" http://localhost:{{default_port}}/api/v1/system/health | python3 -m json.tool
    echo ""
    echo "── SIP Ping (10) ──"
    just sip-ping 10
    echo ""
    echo "── Server Health After Ping ──"
    curl -s -H "Authorization: Bearer $API_KEY" http://localhost:{{default_port}}/api/v1/system/health | python3 -m json.tool

# Full E2E: build + test + start + smoke + SIP load
e2e: build
    #!/usr/bin/env bash
    echo "══ Unit Tests ══"
    cargo test --features carrier 2>&1 | tail -5
    echo ""
    echo "══ E2E Tests ══"
    cargo test --test carrier_e2e_test --features carrier 2>&1 | tail -5
    echo ""
    echo "══ Start Server ══"
    just start
    sleep 1
    echo ""
    if [ -z "$API_KEY" ]; then
        RANDOM_HEX=$(openssl rand -hex 32)
        HASH=$(echo -n "$RANDOM_HEX" | shasum -a 256 | cut -d' ' -f1)
        redis-cli SADD "sv:api_keys" "e2e:${HASH}" > /dev/null
        export API_KEY="sv_${RANDOM_HEX}"
    fi
    echo "══ API Health ══"
    curl -s -H "Authorization: Bearer $API_KEY" http://localhost:{{default_port}}/api/v1/system/health | python3 -m json.tool
    echo ""
    echo "══ SIP Load (100 OPTIONS) ══"
    just sip-ping 100
    echo ""
    echo "══ SIP Load (50 INVITEs) ══"
    just sip-flood 50
    echo ""
    echo "══ Final Health ══"
    curl -s -H "Authorization: Bearer $API_KEY" http://localhost:{{default_port}}/api/v1/system/health | python3 -m json.tool
    echo ""
    echo "══ DONE ══"

# ──────────────────────────────────────────────
# Call Flow E2E Tests
# ──────────────────────────────────────────────

# Test all call flows (SIP→SIP, SIP→WS, SIP→WebRTC)
test-flows: build
    bash scripts/test-call-flows.sh all

# Test SIP-to-SIP proxy flow only
test-sip: build
    bash scripts/test-call-flows.sh sip

# Test SIP-to-WebSocket bridge flow only
test-ws: build
    bash scripts/test-call-flows.sh ws

# Test SIP-to-WebRTC bridge flow only
test-webrtc: build
    bash scripts/test-call-flows.sh webrtc

# Setup test entities without running tests
test-setup: build
    bash scripts/test-call-flows.sh setup

# Clean up test entities and stop server
test-teardown:
    bash scripts/test-call-flows.sh teardown

# ──────────────────────────────────────────────
# Tester (Python E2E tool)
# ──────────────────────────────────────────────

# Run any tester command: just tester <command>
[no-cd]
tester +args="--list":
    cd active-call-tester && just {{args}}

# Run tester API check (validates all 86 carrier endpoints)
api-check: (tester "e2e-api-check")

# ──────────────────────────────────────────────
# Docker
# ──────────────────────────────────────────────

# Build carrier Docker image
docker-build:
    docker build -f Dockerfile.carrier -t active-call:carrier .

# Run carrier Docker image
docker-run:
    docker run --net host active-call:carrier --conf /app/config.toml

# ──────────────────────────────────────────────
# Cleanup
# ──────────────────────────────────────────────

# Stop server + clean build artifacts
clean: stop
    cargo clean
