#!/usr/bin/env bash
# Install the vhs-gui .desktop entry and icon for Wayland taskbar integration.
# Run once after cloning or moving the repo. Re-run if the repo path changes.
set -euo pipefail

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
ICON_SRC="$REPO_DIR/vhs-gui/assets/icon.png"
DESKTOP_SRC="$REPO_DIR/vhs-gui/resources/vhs-gui.desktop"
ICON_DIR="$HOME/.local/share/icons/hicolor/256x256/apps"
APP_DIR="$HOME/.local/share/applications"
BINARY="/usr/local/bin/vhs-gui"

mkdir -p "$ICON_DIR" "$APP_DIR"

cp "$ICON_SRC" "$ICON_DIR/vhs-gui.png"

sed "s|Exec=.*|Exec=$BINARY|" "$DESKTOP_SRC" > "$APP_DIR/vhs-gui.desktop"

update-desktop-database "$APP_DIR" 2>/dev/null || true
gtk-update-icon-cache -f -t "$HOME/.local/share/icons/hicolor/" 2>/dev/null || true

echo "Installed:"
echo "  Icon:    $ICON_DIR/vhs-gui.png"
echo "  Desktop: $APP_DIR/vhs-gui.desktop  (Exec=$BINARY)"
