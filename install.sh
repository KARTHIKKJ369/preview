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
chmod +x "${DESKTOP_DIR}/quicklook.desktop"

# Refresh freedesktop application index database
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "${DESKTOP_DIR}" 2>/dev/null || true
fi

# ---------------------------------------------------------------------------
# 1. KDE Plasma & Dolphin Integration (KDE 5 & KDE 6)
# ---------------------------------------------------------------------------
KIO_DIR="${HOME}/.local/share/kio/servicemenus"
KSERVICES_DIR="${HOME}/.local/share/kservices5/ServiceMenus"
mkdir -p "${KIO_DIR}" "${KSERVICES_DIR}"

cat << 'EOF' > "${KIO_DIR}/quicklook.desktop"
[Desktop Entry]
Type=Service
ServiceTypes=KonqPopupMenu/Plugin
X-KDE-ServiceTypes=KonqPopupMenu/Plugin
MimeType=all/all;all/allfiles;inode/directory;
Actions=quicklook;
X-KDE-Priority=TopLevel
X-KDE-StartupNotify=false

[Desktop Action quicklook]
Name=Quick Look
Icon=system-search
Exec=quicklook %U
EOF

cp "${KIO_DIR}/quicklook.desktop" "${KSERVICES_DIR}/quicklook.desktop"
chmod +x "${KIO_DIR}/quicklook.desktop" "${KSERVICES_DIR}/quicklook.desktop"

# Auto-configure Spacebar shortcut inside Dolphin (KDE 5 & KDE 6)
for rc_dir in "${HOME}/.local/share/kxmlgui5/dolphin" "${HOME}/.local/share/kxmlgui6/dolphin" "${HOME}/.config/kxmlgui5/dolphin" "${HOME}/.config/kxmlgui6/dolphin"; do
    mkdir -p "${rc_dir}"
    rc_file="${rc_dir}/dolphinui.rc"
    if [ ! -f "${rc_file}" ]; then
        cat << 'EOF' > "${rc_file}"
<!DOCTYPE gui SYSTEM 'kpartgui.dtd'>
<gui name="dolphin">
 <ActionProperties scheme="default">
  <Action name="servicemenu_quicklook.desktop::quicklook" shortcut="Space"/>
 </ActionProperties>
</gui>
EOF
    else
        if ! grep -q "servicemenu_quicklook.desktop::quicklook" "${rc_file}"; then
            if grep -q "</ActionProperties>" "${rc_file}"; then
                sed -i '/<\/ActionProperties>/i \  <Action name="servicemenu_quicklook.desktop::quicklook" shortcut="Space"/>' "${rc_file}"
            elif grep -q "</gui>" "${rc_file}"; then
                sed -i '/<\/gui>/i \ <ActionProperties scheme="default">\n  <Action name="servicemenu_quicklook.desktop::quicklook" shortcut="Space"/>\n </ActionProperties>' "${rc_file}"
            fi
        fi
    fi
done

# Rebuild KDE service cache so Dolphin recognizes it immediately
kbuildsycoca6 --noincremental 2>/dev/null || kbuildsycoca5 --noincremental 2>/dev/null || true
echo "==> Installed KDE Dolphin Service Menu & auto-configured Spacebar shortcut"

# Refresh running Dolphin instance so changes take effect immediately
if pgrep -x dolphin >/dev/null 2>&1; then
    dolphin --quit 2>/dev/null || true
fi

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

