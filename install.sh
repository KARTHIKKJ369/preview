#!/usr/bin/env bash
set -euo pipefail

echo "==> Building Quick Look (release mode)..."
cargo build --release

INSTALL_DIR="${HOME}/.local/bin"
DESKTOP_DIR="${HOME}/.local/share/applications"

mkdir -p "${INSTALL_DIR}" "${DESKTOP_DIR}"

echo "==> Installing binary to ${INSTALL_DIR}/quicklook..."
cp target/release/quicklook "${INSTALL_DIR}/quicklook"
chmod +x "${INSTALL_DIR}/quicklook"

echo "==> Installing desktop entry to ${DESKTOP_DIR}/quicklook.desktop..."
cp quicklook.desktop "${DESKTOP_DIR}/quicklook.desktop"

# Nautilus script integration (if GNOME Files is used)
NAUTILUS_SCRIPTS="${HOME}/.local/share/nautilus/scripts"
if [ -d "${HOME}/.local/share/nautilus" ] || [ -d "${NAUTILUS_SCRIPTS}" ]; then
    mkdir -p "${NAUTILUS_SCRIPTS}"
    cat << 'EOF' > "${NAUTILUS_SCRIPTS}/Quick Look"
#!/usr/bin/env bash
quicklook "$@"
EOF
    chmod +x "${NAUTILUS_SCRIPTS}/Quick Look"
    echo "==> Installed GNOME Nautilus script: Right-click -> Scripts -> Quick Look"
fi

echo "==> Quick Look successfully installed!"
echo "Usage: quicklook <file_or_dir>"

