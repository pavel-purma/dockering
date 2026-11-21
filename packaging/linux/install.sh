#!/bin/sh
# Installs Dockering from the unpacked .tar.gz for the current user (SHL-023):
#   ./install.sh               binary -> ~/.local/bin, desktop entry and icon -> ~/.local/share
#   ./install.sh --uninstall   removes them again
# Set PREFIX to install elsewhere, e.g. `sudo PREFIX=/usr/local ./install.sh`.
set -eu

APP_ID=dev.dockering.Dockering
PREFIX=${PREFIX:-$HOME/.local}
HERE=$(cd "$(dirname "$0")" && pwd)

BIN_DIR=$PREFIX/bin
APPS_DIR=$PREFIX/share/applications
ICON_THEME=$PREFIX/share/icons/hicolor
ICON_DIR=$ICON_THEME/512x512/apps

refresh() {
  if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database -q "$APPS_DIR" 2>/dev/null || true
  fi
  if command -v gtk-update-icon-cache >/dev/null 2>&1 && [ -f "$ICON_THEME/index.theme" ]; then
    gtk-update-icon-cache -q -t -f "$ICON_THEME" 2>/dev/null || true
  fi
}

case "${1:-}" in
--uninstall)
  rm -f "$BIN_DIR/dockering" "$APPS_DIR/$APP_ID.desktop" "$ICON_DIR/$APP_ID.png"
  refresh
  echo "Dockering removed from $PREFIX"
  exit 0
  ;;
"") ;;
*)
  echo "usage: $0 [--uninstall]" >&2
  exit 2
  ;;
esac

mkdir -p "$BIN_DIR" "$APPS_DIR" "$ICON_DIR"
install -m 0755 "$HERE/dockering" "$BIN_DIR/dockering"
install -m 0644 "$HERE/share/icons/hicolor/512x512/apps/$APP_ID.png" "$ICON_DIR/$APP_ID.png"
# An absolute Exec keeps the menu entry working when $BIN_DIR is not on the launcher's PATH.
sed "s|^Exec=dockering|Exec=\"$BIN_DIR/dockering\"|" "$HERE/share/applications/$APP_ID.desktop" >"$APPS_DIR/$APP_ID.desktop"
chmod 0644 "$APPS_DIR/$APP_ID.desktop"
refresh

echo "Dockering installed to $BIN_DIR/dockering"
echo "Menu entry: $APPS_DIR/$APP_ID.desktop"
