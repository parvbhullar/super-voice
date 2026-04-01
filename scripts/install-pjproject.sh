#!/usr/bin/env bash
# scripts/install-pjproject.sh
# Build and install pjproject 2.14.1 from source.
# Installs to /usr/local by default; override with PREFIX env var.

set -euo pipefail

PJPROJECT_VERSION="2.14.1"
PREFIX="${PREFIX:-/usr/local}"
WORKDIR="$(mktemp -d)"

echo "==> Downloading pjproject ${PJPROJECT_VERSION}..."
cd "$WORKDIR"
curl -fsSL "https://github.com/pjsip/pjproject/archive/refs/tags/${PJPROJECT_VERSION}.tar.gz" \
    | tar xz

cd "pjproject-${PJPROJECT_VERSION}"

echo "==> Configuring (SIP-only, no video, no sound device)..."
# Try with macOS Homebrew OpenSSL path first, then fall back to plain configure.
./configure \
    --prefix="$PREFIX" \
    --disable-video \
    --disable-sound \
    --disable-v4l2 \
    --disable-opencore-amr \
    --disable-silk \
    --disable-bcg729 \
    --disable-libyuv \
    --disable-libwebrtc \
    --enable-shared \
    --with-ssl="$(brew --prefix openssl 2>/dev/null)" 2>/dev/null || \
./configure \
    --prefix="$PREFIX" \
    --disable-video \
    --disable-sound \
    --disable-v4l2 \
    --disable-opencore-amr \
    --disable-silk \
    --disable-bcg729 \
    --disable-libyuv \
    --disable-libwebrtc \
    --enable-shared

echo "==> Building..."
make dep
make -j"$(nproc 2>/dev/null || sysctl -n hw.ncpu)"

echo "==> Installing to ${PREFIX}..."
make install

echo "==> Cleaning up..."
rm -rf "$WORKDIR"

echo "==> pjproject ${PJPROJECT_VERSION} installed to ${PREFIX}"
echo "    Verify: pkg-config --modversion libpjproject"
