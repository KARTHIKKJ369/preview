#!/usr/bin/env bash
set -euo pipefail

echo "==> Building Quick Look (optimized release mode)..."
cargo build --release

INSTALL_DIR="${HOME}/.local/bin"
DESKTOP_DIR="${HOME}/.local/share/applications"

mkdir -p "${INSTALL_DIR}" "${DESKTOP_DIR}"

echo "==> Installing binary to ${INSTALL_DIR}/quicklook..."
cp target/release/quicklook "${INSTALL_DIR}/quicklook"
chmod +x "${INSTALL_DIR}/quicklook"

echo "==> Installing desktop entry to ${DESKTOP_DIR}/quicklook.desktop..."
cp quicklook.desktop "${DESKTOP_DIR}/quicklook.desktop"

# ---------------------------------------------------------------------------
# 1. KDE Plasma & Dolphin Integration (KDE 5 & KDE 6)
# ---------------------------------------------------------------------------
KIO_DIR="${HOME}/.local/share/kio/servicemenus"
KSERVICES_DIR="${HOME}/.local/share/kservices5/ServiceMenus"
mkdir -p "${KIO_DIR}" "${KSERVICES_DIR}"

cat << 'EOF' > "${KIO_DIR}/quicklook.desktop"
[Desktop Entry]
Type=Service
MimeType=all/all;all/allfiles;inode/directory;
X-KDE-ServiceTypes=KonqPopupMenu/Plugin
Actions=quicklook;
X-KDE-Priority=TopLevel
X-KDE-StartupNotify=false

[Desktop Action quicklook]
Name=Quick Look
Icon=system-search
Exec=quicklook %U
EOF

cp "${KIO_DIR}/quicklook.desktop" "${KSERVICES_DIR}/quicklook.desktop"

# Rebuild KDE service cache so Dolphin recognizes it immediately
kbuildsycoca6 2>/dev/null || kbuildsycoca5 2>/dev/null || true
echo "==> Installed KDE Dolphin Service Menu (KDE Plasma 5 & 6)"

# Optional: Register KDE global shortcut Meta+Space
for kwc in kwriteconfig6 kwriteconfig5; do
    if command -v "$kwc" >/dev/null 2>&1; then
        "$kwc" --file kglobalshortcutsrc --group "quicklook.desktop" --key "_k_friendly_name" "Quick Look" 2>/dev/null || true
        "$kwc" --file kglobalshortcutsrc --group "quicklook.desktop" --key "_launch" "Meta+Space,none,Quick Look" 2>/dev/null || true
    fi
done

# ---------------------------------------------------------------------------
# 2. GNOME Nautilus Integration
# ---------------------------------------------------------------------------
NAUTILUS_SCRIPTS="${HOME}/.local/share/nautilus/scripts"
mkdir -p "${NAUTILUS_SCRIPTS}"
cat << 'EOF' > "${NAUTILUS_SCRIPTS}/Quick Look"
#!/usr/bin/env bash
quicklook "$@"
EOF
chmod +x "${NAUTILUS_SCRIPTS}/Quick Look"

# ---------------------------------------------------------------------------
# 3. Cinnamon Nemo Integration
# ---------------------------------------------------------------------------
NEMO_DIR="${HOME}/.local/share/nemo/actions"
mkdir -p "${NEMO_DIR}"
cat << 'EOF' > "${NEMO_DIR}/quicklook.nemo_action"
[Nemo Action]
Name=Quick Look
Comment=Quick Look File Preview
Exec=quicklook %F
Icon-Name=system-search
Selection=any
Extensions=any;
Quote=double
EOF

# ---------------------------------------------------------------------------
# 4. MATE Caja Integration
# ---------------------------------------------------------------------------
CAJA_SCRIPTS="${HOME}/.local/share/caja/scripts"
mkdir -p "${CAJA_SCRIPTS}"
cp "${NAUTILUS_SCRIPTS}/Quick Look" "${CAJA_SCRIPTS}/Quick Look" 2>/dev/null || true

echo "==> Quick Look successfully installed for all desktop environments!"
echo "Usage: quicklook <file_or_dir>"
echo "In KDE Dolphin: Right-click any file -> Quick Look, or press Space"

