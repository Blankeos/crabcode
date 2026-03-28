#!/bin/bash

set -e

VERSION="latest"
REPO="Blankeos/crabcode"
INSTALL_DIR="$HOME/.local/bin"
BINARY_NAME="crabcode"

echo "🦀 Installing crabcode..."

# Check if cargo is available
if command -v cargo &> /dev/null; then
    echo "📦 Installing via cargo..."
    cargo install crabcode
    echo "✓ crabcode installed successfully via cargo"
    echo ""
    echo "Run: crabcode"
    exit 0
fi

# Fall back to downloading pre-built binary
echo "⬇️ Downloading pre-built binary..."

# Determine platform
OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS" in
    Linux*)     OS="linux";;
    Darwin*)    OS="macos";;
    *)          echo "❌ Unsupported OS: $OS"; exit 1;;
esac

case "$ARCH" in
    x86_64)    ARCH="x86_64";;
    aarch64)   ARCH="aarch64";;
    *)         echo "❌ Unsupported architecture: $ARCH"; exit 1;;
esac

# Create install directory
mkdir -p "$INSTALL_DIR"

# Download binary
BINARY_URL="https://github.com/${REPO}/releases/download/${VERSION}/crabcode-${OS}-${ARCH}"

if curl -L "$BINARY_URL" -o "$INSTALL_DIR/$BINARY_NAME"; then
    chmod +x "$INSTALL_DIR/$BINARY_NAME"
    echo "✓ crabcode installed successfully to $INSTALL_DIR/$BINARY_NAME"
else
    echo "❌ Failed to download binary. Please install via cargo: cargo install crabcode"
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
