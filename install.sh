#!/bin/sh
# Install xmaster - enterprise-grade X/Twitter CLI
# Usage: curl -fsSL https://raw.githubusercontent.com/paperfoot/xmaster-cli/master/install.sh | sh
set -e

REPO="paperfoot/xmaster-cli"
BINARY="xmaster"

# Detect OS and architecture
OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS" in
  Linux)  OS_TAG="unknown-linux-gnu" ;;
  Darwin) OS_TAG="apple-darwin" ;;
  *)      echo "Unsupported OS: $OS"; exit 1 ;;
esac

case "$ARCH" in
  x86_64)  ARCH_TAG="x86_64" ;;
  aarch64|arm64) ARCH_TAG="aarch64" ;;
  *)       echo "Unsupported architecture: $ARCH"; exit 1 ;;
esac

TARGET="${ARCH_TAG}-${OS_TAG}"
LATEST=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" | grep '"tag_name"' | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/')

if [ -z "$LATEST" ]; then
  echo "No releases found. Install from source:"
  echo "  cargo install --git https://github.com/${REPO}"
  exit 1
fi

ASSET="${BINARY}-${TARGET}"
URL="https://github.com/${REPO}/releases/download/${LATEST}/${ASSET}"
echo "Downloading xmaster ${LATEST} for ${TARGET}..."

TMPDIR=$(mktemp -d)
curl -fsSL "$URL" -o "${TMPDIR}/${BINARY}"

# Install to ~/.local/bin or /usr/local/bin
if [ -d "$HOME/.local/bin" ]; then
  INSTALL_DIR="$HOME/.local/bin"
elif [ -w "/usr/local/bin" ]; then
  INSTALL_DIR="/usr/local/bin"
else
  INSTALL_DIR="$HOME/.local/bin"
  mkdir -p "$INSTALL_DIR"
fi

mv "${TMPDIR}/${BINARY}" "${INSTALL_DIR}/${BINARY}"
chmod +x "${INSTALL_DIR}/${BINARY}"
rm -rf "$TMPDIR"

echo ""
echo "Installed xmaster to ${INSTALL_DIR}/${BINARY}"
echo ""

# Check if in PATH
if ! command -v xmaster >/dev/null 2>&1; then
  echo "Add to your PATH:"
  echo "  export PATH=\"${INSTALL_DIR}:\$PATH\""
  echo ""
fi

echo "Get started:"
echo "  xmaster --help"
echo "  xmaster config check"
echo ""
echo "★ If xmaster saves you time, a GitHub star helps others find it:"
echo "  https://github.com/paperfoot/xmaster-cli"
echo ""
