#!/usr/bin/env bash
# Build (if needed) and install Loopz user-locally.
set -euo pipefail
cd "$(dirname "$0")/.."
BIN_SRC="src-tauri/target/release/loopz"
[ -x "$BIN_SRC" ] || ./scripts/build.sh
BIN="$HOME/.local/bin/loopz"
ICON="$HOME/.local/share/icons/loopz.png"
DESKTOP="$HOME/.local/share/applications/loopz.desktop"
mkdir -p "$HOME/.local/bin" "$HOME/.local/share/icons" "$HOME/.local/share/applications"
cp "$BIN_SRC" "$BIN"
cp "src-tauri/icons/icon.png" "$ICON"
cat > "$DESKTOP" << DESK
[Desktop Entry]
Name=Loopz
Comment=120 gapless drum loops, standalone app
Exec=env WEBKIT_DISABLE_COMPOSITING_MODE=1 $BIN
Icon=$ICON
Terminal=false
Type=Application
Categories=AudioVideo;Audio;
StartupWMClass=loopz
DESK
update-desktop-database "$HOME/.local/share/applications" 2>/dev/null || true
echo "Installed: $BIN + launcher entry"
