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

# Auto-configure Spacebar shortcut inside Dolphin and unbind conflicting "Selection Mode"
python3 - << 'EOF' 2>/dev/null || true
import os

dirs = [
    os.path.expanduser("~/.local/share/kxmlgui5/dolphin"),
    os.path.expanduser("~/.local/share/kxmlgui6/dolphin"),
    os.path.expanduser("~/.config/kxmlgui5/dolphin"),
    os.path.expanduser("~/.config/kxmlgui6/dolphin"),
]

injected = (
    '  <Action name="toggle_selection_mode" shortcut=""/>\n'
    '  <Action name="select_items" shortcut=""/>\n'
    '  <Action name="servicemenu_quicklook.desktop::quicklook" shortcut="Space"/>'
)

for d in dirs:
    os.makedirs(d, exist_ok=True)
    p = os.path.join(d, "dolphinui.rc")
    content = ""
    if os.path.exists(p):
        with open(p, "r", encoding="utf-8", errors="ignore") as f:
            content = f.read()

    lines = [
        line for line in content.splitlines()
        if "servicemenu_quicklook.desktop::quicklook" not in line
        and "toggle_selection_mode" not in line
        and "select_items" not in line
    ]
    content = "\n".join(lines)

    if "</ActionProperties>" in content:
        content = content.replace("</ActionProperties>", injected + "\n </ActionProperties>")
    elif "</gui>" in content:
        content = content.replace("</gui>", ' <ActionProperties scheme="default">\n' + injected + '\n </ActionProperties>\n</gui>')
    else:
        content = (
            "<!DOCTYPE gui SYSTEM 'kpartgui.dtd'>\n"
            '<gui name="dolphin">\n'
            ' <ActionProperties scheme="default">\n'
            + injected +
            '\n </ActionProperties>\n'
            "</gui>\n"
        )

    with open(p, "w", encoding="utf-8") as f:
        f.write(content)
EOF

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

# Check helper tools for PDF page rendering and video frame previews
MISSING_HELPERS=()
if ! command -v pdftoppm >/dev/null 2>&1 && ! command -v gs >/dev/null 2>&1; then
    MISSING_HELPERS+=("poppler-utils (for PDF page-by-page rendering)")
fi
if ! command -v ffmpeg >/dev/null 2>&1 && ! command -v ffmpegthumbnailer >/dev/null 2>&1; then
    MISSING_HELPERS+=("ffmpeg or ffmpegthumbnailer (for video frame previews)")
fi

if [ ${#MISSING_HELPERS[@]} -gt 0 ]; then
    echo ""
    echo "💡 Note: Quick Look already uses Dolphin's thumbnail cache as an automatic fallback."
    echo "   To also enable full high-res PDF page-by-page flipping and video frames, install:"
    for helper in "${MISSING_HELPERS[@]}"; do
        echo "   - ${helper}"
    done
    echo "   Debian / Ubuntu: sudo apt install poppler-utils ffmpeg"
    echo "   Arch Linux:      sudo pacman -S poppler ffmpeg"
    echo "   Fedora:          sudo dnf install poppler-utils ffmpeg-free"
fi

