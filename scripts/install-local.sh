#!/usr/bin/env bash
# Installs Remoport for the current user, with its launcher and icon.
#
# On Wayland the taskbar matches the window's app ID (app.remoport.Remoport) to
# an installed .desktop file of the same name and shows that file's icon.
# Without them, the desktop falls back to a generic icon.
#
#   scripts/install-local.sh              build in release mode and install
#   scripts/install-local.sh --uninstall  remove what this script installed
set -euo pipefail

app_id=app.remoport.Remoport
root=$(cd "$(dirname "$0")/.." && pwd)
data_home=${XDG_DATA_HOME:-$HOME/.local/share}
bin=$HOME/.local/bin/remoport
desktop=$data_home/applications/$app_id.desktop
icon=$data_home/icons/hicolor/scalable/apps/$app_id.svg
metainfo=$data_home/metainfo/$app_id.metainfo.xml

# Let the desktop notice the new or removed launcher without a new session.
refresh() {
    if command -v update-desktop-database >/dev/null; then
        update-desktop-database "$data_home/applications" || true
    fi
    for tool in kbuildsycoca6 kbuildsycoca5; do
        if command -v "$tool" >/dev/null; then
            "$tool" >/dev/null 2>&1 || true
            break
        fi
    done
}

if [ "${1:-}" = "--uninstall" ]; then
    rm -f -- "$bin" "$desktop" "$icon" "$metainfo"
    refresh
    echo "Remoport uninstalled."
    exit 0
fi

cargo build --release --manifest-path "$root/Cargo.toml"
install -Dm755 "$root/target/release/remoport" "$bin"
install -Dm644 "$root/data/icons/hicolor/scalable/apps/$app_id.svg" "$icon"
install -Dm644 "$root/data/$app_id.metainfo.xml" "$metainfo"
# The launcher names the binary by its full path, so it starts even when
# ~/.local/bin is not on the desktop session's PATH.
mkdir -p "$(dirname "$desktop")"
sed "s|^Exec=remoport$|Exec=$bin|" "$root/data/$app_id.desktop" > "$desktop"
chmod 644 "$desktop"
refresh
echo "Remoport installed: $bin"
