#!/bin/sh
set -e

INSTALL_DIR="/usr/local/bin"
BINARY="architect"
SOURCE="target/release/$BINARY"

main() {
    if [ ! -f "$SOURCE" ]; then
        echo "Binary not found at $SOURCE"
        echo "Run 'cargo build --release -p architect-client' first."
        exit 1
    fi

    if [ "$(id -u)" -ne 0 ]; then
        INSTALL_DIR="$HOME/.local/bin"
        mkdir -p "$INSTALL_DIR"
    fi

    echo "Installing to ${INSTALL_DIR}/${BINARY}..."
    cp "$SOURCE" "$INSTALL_DIR/$BINARY"
    chmod +x "$INSTALL_DIR/$BINARY"

    if ! echo "$PATH" | tr ':' '\n' | grep -qx "$INSTALL_DIR"; then
        echo ""
        echo "Add this to your shell profile:"
        echo "  export PATH=\"$INSTALL_DIR:\$PATH\""
    fi

    echo ""
    echo "Installed: $("$INSTALL_DIR/$BINARY" --version 2>/dev/null || echo "$BINARY")"
}

main
