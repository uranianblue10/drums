#!/usr/bin/env bash
# Remove the user-local Loopz install.
set -euo pipefail
rm -f "$HOME/.local/bin/loopz" \
      "$HOME/.local/share/icons/loopz.png" \
      "$HOME/.local/share/applications/loopz.desktop" \
      "$HOME/.local/share/applications/LoopzTauri.desktop"
update-desktop-database "$HOME/.local/share/applications" 2>/dev/null || true
echo "Uninstalled."
