#!/usr/bin/env bash
# install.sh — Full setup script for super-voice (active-call)
# Run: bash install.sh

set -euo pipefail

GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

step() { echo -e "\n${GREEN}==>${NC} $1"; }
warn() { echo -e "${YELLOW}[warn]${NC} $1"; }

# ── 1. System Dependencies ────────────────────────────────────────────────────
step "Installing system dependencies..."
sudo apt-get update
sudo apt-get install -y \
  build-essential curl git pkg-config cmake \
  clang libclang-dev \
  libssl-dev \
  libspeex-dev libspeexdsp-dev \
  libogg-dev libopus-dev \
  libsrtp2-dev \
  libasound2-dev

# ── 2. Rust ───────────────────────────────────────────────────────────────────
step "Installing Rust..."
if command -v rustc &>/dev/null; then
  warn "Rust already installed ($(rustc --version)), skipping."
else
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path
fi
source "$HOME/.cargo/env"

# ── 3. Just (task runner) ─────────────────────────────────────────────────────
step "Installing just..."
if command -v just &>/dev/null; then
  warn "just already installed ($(just --version)), skipping."
else
  # Try prebuilt binary first (faster than cargo install)
  if curl -fsSL https://just.systems/install.sh | sudo bash -s -- --to /usr/local/bin 2>/dev/null; then
    echo "just installed via prebuilt binary"
  else
    warn "Prebuilt install failed, falling back to cargo install just..."
    cargo install just
  fi
fi

# ── 4. ONNX Runtime ──────────────────────────────────────────────────────────
step "Installing ONNX Runtime 1.23.2 (required by ort 2.0.0-rc.11)..."
ORT_VERSION="1.23.2"
ORT_LIB="/usr/local/lib/libonnxruntime.so"
if [ -f "$ORT_LIB" ]; then
  warn "libonnxruntime.so already installed at $ORT_LIB, skipping."
else
  ORT_URL="https://github.com/microsoft/onnxruntime/releases/download/v${ORT_VERSION}/onnxruntime-linux-x64-${ORT_VERSION}.tgz"
  TMP_DIR=$(mktemp -d)
  curl -fsSL "$ORT_URL" | tar -xz -C "$TMP_DIR"
  sudo cp "$TMP_DIR"/onnxruntime-linux-x64-*/lib/libonnxruntime.so* /usr/local/lib/
  sudo ldconfig
  rm -rf "$TMP_DIR"
  echo "libonnxruntime.so ${ORT_VERSION}: installed"
fi

# ── 5. PJSIP ─────────────────────────────────────────────────────────────────
step "Installing pjproject 2.14.1 (SIP C stack)..."
if pkg-config --exists libpjproject 2>/dev/null; then
  warn "pjproject already installed ($(pkg-config --modversion libpjproject)), skipping."
else
  bash scripts/install-pjproject.sh
fi
echo "pjproject: $(pkg-config --modversion libpjproject)"

# ── 6. Redis ──────────────────────────────────────────────────────────────────
step "Installing Redis..."
if command -v redis-server &>/dev/null; then
  warn "Redis already installed, skipping apt install."
else
  sudo apt-get install -y redis-server
fi

# Enable AOF persistence
if ! grep -q "^appendonly yes" /etc/redis/redis.conf 2>/dev/null; then
  sudo sed -i 's/^# appendonly no/appendonly yes/' /etc/redis/redis.conf || true
  sudo sed -i 's/^appendonly no/appendonly yes/' /etc/redis/redis.conf || true
  warn "Enabled Redis AOF persistence in /etc/redis/redis.conf"
fi

sudo systemctl enable redis-server &>/dev/null || true
sudo systemctl restart redis-server
redis-cli ping | grep -q PONG && echo "Redis: OK" || warn "Redis ping failed — check service"

# ── 7. sipbot (optional, for SIP integration tests) ───────────────────────────
step "Installing sipbot (SIP test tool)..."
if command -v sipbot &>/dev/null; then
  warn "sipbot already installed, skipping."
else
  cargo install sipbot
fi

# ── 8. Build project ──────────────────────────────────────────────────────────
step "Building active-call (release)..."
cargo build --release

# ── 9. Generate default config if missing ────────────────────────────────────
if [ ! -f my-config.toml ]; then
  step "Creating default my-config.toml..."
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
  warn "Created my-config.toml — change api_keys before running in production!"
else
  warn "my-config.toml already exists, skipping."
fi

# ── Done ──────────────────────────────────────────────────────────────────────
echo -e "\n${GREEN}Installation complete!${NC}"
echo ""
echo "Run the server:"
echo "  ./target/release/active-call --conf my-config.toml"
echo ""
echo "Open console:"
echo "  http://localhost:8080/console"
echo "  Login with the api_keys value from my-config.toml"