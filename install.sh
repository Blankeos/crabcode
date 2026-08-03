#!/bin/bash

set -euo pipefail

REPO="yan-ad/crabcode"
INSTALL_DIR="$HOME/.local/bin"
BINARY_NAME="crabcode"

echo "🦀 Installing crabcode..."

# Download the newest preview archive, or an explicitly requested preview tag.
echo "⬇️ Downloading pre-built binary..."

# Determine platform
OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS" in
    Linux*)     PLATFORM="linux-x64";;
    Darwin*)    PLATFORM="macos-arm64";;
    *)          echo "❌ Unsupported OS: $OS"; exit 1;;
esac

if [[ "$PLATFORM" == "macos-arm64" && "$ARCH" != "arm64" && "$ARCH" != "aarch64" ]]; then
    echo "❌ Unsupported architecture for macOS preview: $ARCH (Apple Silicon required)"
    exit 1
fi

if [[ "$PLATFORM" == "linux-x64" && "$ARCH" != "x86_64" ]]; then
    echo "❌ Unsupported architecture for Linux preview: $ARCH (x86_64 required)"
    exit 1
fi

case "$ARCH" in
    x86_64|arm64) ;;
    aarch64) ;;
    *)         echo "❌ Unsupported architecture: $ARCH"; exit 1;;
esac

# Create install directory
mkdir -p "$INSTALL_DIR"

TAG="${CRABCODE_PREVIEW_TAG:-}"
if [[ -z "$TAG" ]]; then
    TAG="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases" \
        | sed -nE '/"tag_name": "gondescode-[^"]+"/{s/.*"tag_name": "([^"]+)".*/\1/p; q;}')"
fi

if [[ -z "$TAG" ]]; then
    echo "❌ No preview release found in ${REPO}."
    exit 1
fi

VERSION="${TAG#gondescode-}"
ARCHIVE="crabcode-${VERSION:0:7}-${PLATFORM}.tar.gz"
BINARY_URL="https://github.com/${REPO}/releases/download/${TAG}/${ARCHIVE}"
TEMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TEMP_DIR"' EXIT

if curl -fL "$BINARY_URL" -o "$TEMP_DIR/$ARCHIVE" \
    && tar -xzf "$TEMP_DIR/$ARCHIVE" -C "$TEMP_DIR" \
    && install -m 755 "$TEMP_DIR/$BINARY_NAME" "$INSTALL_DIR/$BINARY_NAME"; then
    if [[ "$OS" == "Darwin" ]]; then
        xattr -d com.apple.quarantine "$INSTALL_DIR/$BINARY_NAME" 2>/dev/null || true
    fi
    echo "✓ crabcode installed successfully to $INSTALL_DIR/$BINARY_NAME"
else
    echo "❌ Failed to download preview from ${REPO}."
    exit 1
fi

# Add to PATH if not already there
if [[ ":$PATH:" != *":$INSTALL_DIR:"* ]]; then
    echo ""
    echo "⚠️  Add $INSTALL_DIR to your PATH:"
    echo "   export PATH=\"\$HOME/.local/bin:\$PATH\""
    echo "   Add this to your ~/.bashrc or ~/.zshrc"
fi

echo ""
echo "Run: $BINARY_NAME"
