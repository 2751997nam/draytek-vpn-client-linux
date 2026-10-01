#!/usr/bin/env bash
set -euo pipefail

# Build an AppImage for the DrayTek VPN standalone app.
# The helper binary and polkit policy are bundled inside the AppImage
# but require manual installation for privilege separation to work.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$PROJECT_DIR"

TOOLS_DIR="$SCRIPT_DIR/appimage-tools"
LINUXDEPLOY="$TOOLS_DIR/linuxdeploy-x86_64.AppImage"
GTK_PLUGIN="$TOOLS_DIR/linuxdeploy-plugin-gtk.sh"

# ── Download tools ────────────────────────────────────────────────
mkdir -p "$TOOLS_DIR"

if [[ ! -x "$LINUXDEPLOY" ]]; then
    echo "Downloading linuxdeploy..."
    wget -O "$LINUXDEPLOY" \
        "https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage"
    chmod +x "$LINUXDEPLOY"
fi

if [[ ! -x "$GTK_PLUGIN" ]]; then
    echo "Downloading linuxdeploy-plugin-gtk..."
    wget -O "$GTK_PLUGIN" \
        "https://raw.githubusercontent.com/linuxdeploy/linuxdeploy-plugin-gtk/master/linuxdeploy-plugin-gtk.sh"
    chmod +x "$GTK_PLUGIN"
fi

# ── Build release binaries ────────────────────────────────────────
echo "Building release binaries..."
cargo build -p draytek-vpn --release

# ── Set up AppDir ─────────────────────────────────────────────────
rm -rf "$SCRIPT_DIR/AppDir"
mkdir -p "$SCRIPT_DIR/AppDir/usr/bin"
mkdir -p "$SCRIPT_DIR/AppDir/usr/lib/draytek-vpn"
mkdir -p "$SCRIPT_DIR/AppDir/usr/share/polkit-1/actions"

cp "$PROJECT_DIR/target/release/draytek-vpn" "$SCRIPT_DIR/AppDir/usr/bin/"
cp "$PROJECT_DIR/target/release/draytek-vpn-helper" "$SCRIPT_DIR/AppDir/usr/lib/draytek-vpn/"
cp "$SCRIPT_DIR/data/com.draytek.vpn.policy" "$SCRIPT_DIR/AppDir/usr/share/polkit-1/actions/"

# ── Icon ───────────────────────────────────────────────────────────
# The app ships its own icon, so the AppImage no longer borrows a generic
# network-vpn glyph from whatever icon theme the build host happens to have.
ICON_SRC="$SCRIPT_DIR/data/icons/draytek-vpn.svg"
if [[ ! -f "$ICON_SRC" ]]; then
    error "Missing application icon: $ICON_SRC"
    exit 1
fi
mkdir -p "$SCRIPT_DIR/AppDir/usr/share/icons/hicolor/scalable/apps"
cp "$ICON_SRC" "$SCRIPT_DIR/AppDir/usr/share/icons/hicolor/scalable/apps/"

# ── Build AppImage ────────────────────────────────────────────────
export DEPLOY_GTK_VERSION=4
export PATH="$TOOLS_DIR:$PATH"

"$LINUXDEPLOY" \
    --appdir "$SCRIPT_DIR/AppDir" \
    --executable "$PROJECT_DIR/target/release/draytek-vpn" \
    --desktop-file "$SCRIPT_DIR/data/draytek-vpn.desktop" \
    --icon-file "$ICON_SRC" \
    --plugin gtk \
    --output appimage

# ── Rename to consistent format ───────────────────────────────────
VERSION=$(grep '^version' "$SCRIPT_DIR/Cargo.toml" | head -1 | sed 's/.*"\(.*\)".*/\1/')
APPIMAGE_OUT="$PROJECT_DIR/draytek-vpn-standalone_${VERSION}_x86_64.AppImage"
# linuxdeploy names the file from the desktop Name field
GENERATED=$(ls -t "$PROJECT_DIR"/*.AppImage 2>/dev/null | grep -v linuxdeploy | head -1)
if [[ -n "$GENERATED" && "$GENERATED" != "$APPIMAGE_OUT" ]]; then
    mv "$GENERATED" "$APPIMAGE_OUT"
fi

echo ""
echo "AppImage built successfully:"
ls -lh "$APPIMAGE_OUT"
echo ""
echo "NOTE: The helper binary and polkit policy are bundled but need"
echo "manual installation for privilege separation to work:"
echo "  sudo install -m 755 AppDir/usr/lib/draytek-vpn/draytek-vpn-helper /usr/lib/draytek-vpn/"
echo "  sudo install -m 644 AppDir/usr/share/polkit-1/actions/com.draytek.vpn.policy /usr/share/polkit-1/actions/"
