#!/bin/sh
set -e

BINARY="architect"

main() {
    bin_path=$(command -v "$BINARY" 2>/dev/null || true)

    if [ -z "$bin_path" ]; then
        # Check common locations
        for dir in /usr/local/bin "$HOME/.local/bin"; do
            if [ -f "$dir/$BINARY" ]; then
                bin_path="$dir/$BINARY"
                break
            fi
        done
    fi

    if [ -n "$bin_path" ]; then
        echo "Removing $bin_path..."
        rm -f "$bin_path"
    else
        echo "Binary not found, skipping."
    fi

    config_dir="$HOME/.config/architect"
    data_dir="$HOME/.architect"

    if [ -d "$config_dir" ]; then
        printf "Remove config (%s)? [y/N] " "$config_dir"
        read -r answer
        case "$answer" in
            [yY]*) rm -rf "$config_dir"; echo "Removed." ;;
            *)     echo "Kept." ;;
        esac
    fi

    if [ -d "$data_dir" ]; then
        printf "Remove data (%s)? [y/N] " "$data_dir"
        read -r answer
        case "$answer" in
            [yY]*) rm -rf "$data_dir"; echo "Removed." ;;
            *)     echo "Kept." ;;
        esac
    fi

    echo ""
    echo "Uninstall complete."
}

main
