#!/usr/bin/env bash
# Build snapgo and install it for the current user (no root needed).
#
#   ./install.sh                             build + install
#   ./install.sh --shortcut '<Shift><Super>s'  also bind a GNOME keyboard shortcut
set -euo pipefail
cd "$(dirname "$0")"

APP_ID=org.snapgo.Snapgo
BIN_DIR="$HOME/.local/bin"
APP_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
SHORTCUT=""

while [ $# -gt 0 ]; do
    case "$1" in
        --shortcut) SHORTCUT="${2:?--shortcut needs a key combination}"; shift 2 ;;
        *) echo "unknown option: $1" >&2; exit 1 ;;
    esac
done

if ! pkg-config --exists gtk4; then
    echo "GTK 4 development files are missing. Install them first:" >&2
    echo "    sudo apt install libgtk-4-dev" >&2
    exit 1
fi

cargo build --release

install -Dm755 target/release/snapgo "$BIN_DIR/snapgo"
# The desktop file is what lets GNOME remember the screenshot permission.
mkdir -p "$APP_DIR"
sed "s|^Exec=snapgo|Exec=$BIN_DIR/snapgo|" "data/$APP_ID.desktop" > "$APP_DIR/$APP_ID.desktop"
update-desktop-database "$APP_DIR" 2>/dev/null || true
echo "Installed $BIN_DIR/snapgo"

if [ -n "$SHORTCUT" ]; then
    SCHEMA=org.gnome.settings-daemon.plugins.media-keys
    KEYPATH=/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/snapgo/
    current=$(gsettings get $SCHEMA custom-keybindings)
    case "$current" in
        *"'$KEYPATH'"*) ;;
        "@as []"|"[]") gsettings set $SCHEMA custom-keybindings "['$KEYPATH']" ;;
        *) gsettings set $SCHEMA custom-keybindings "${current%]}, '$KEYPATH']" ;;
    esac
    gsettings set "$SCHEMA.custom-keybinding:$KEYPATH" name 'snapgo'
    gsettings set "$SCHEMA.custom-keybinding:$KEYPATH" command "$BIN_DIR/snapgo"
    gsettings set "$SCHEMA.custom-keybinding:$KEYPATH" binding "$SHORTCUT"
    echo "Bound $SHORTCUT to snapgo"
fi
