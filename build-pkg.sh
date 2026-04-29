#!/bin/bash
# ============================================================
# build-pkg.sh — Build a single .pkg installer for
# "It Hurts When IP" (app + helper + uninstaller)
#
# Run from: ~/ip-switcher/
# Usage:    chmod +x build-pkg.sh && ./build-pkg.sh
#
# Prerequisites:
#   1. Universal helper binary already built:
#        cd helper-tool
#        rustup target add x86_64-apple-darwin
#        cargo build --release --target aarch64-apple-darwin
#        cargo build --release --target x86_64-apple-darwin
#        lipo -create \
#          target/aarch64-apple-darwin/release/ipswitcher-helper \
#          target/x86_64-apple-darwin/release/ipswitcher-helper \
#          -output com.ipswitcher.helper
#        codesign -s - --force com.ipswitcher.helper
#        cd ..
#
#   2. Universal app already built:
#        npm run tauri build -- --target universal-apple-darwin
#        (produces .app bundle in src-tauri/target/universal-apple-darwin/release/bundle/macos/)
# ============================================================

set -e  # Exit on any error

APP_NAME="It Hurts When IP"
PKG_ID="com.ipswitcher.pkg"
VERSION="1.11.0"
OUTPUT_PKG="ItHurtsWhenIP-${VERSION}.pkg"

# --- Paths to your pre-built binaries ---
HELPER_BIN="helper-tool/com.ipswitcher.helper"
HELPER_PLIST="helper-tool/com.ipswitcher.helper.plist"
APP_BUNDLE="src-tauri/target/universal-apple-darwin/release/bundle/macos/${APP_NAME}.app"

# --- Working directories ---
WORK_DIR="installer-build"
PAYLOAD_DIR="${WORK_DIR}/payload"
SCRIPTS_DIR="${WORK_DIR}/scripts"

echo "=========================================="
echo " Building ${APP_NAME} v${VERSION} .pkg"
echo "=========================================="

# --- Validate that required files exist ---
echo "[1/7] Checking prerequisites..."

if [ ! -f "${HELPER_BIN}" ]; then
    echo "ERROR: Helper binary not found at ${HELPER_BIN}"
    echo "       Build the universal helper first (see instructions at top of script)"
    exit 1
fi

if [ ! -f "${HELPER_PLIST}" ]; then
    echo "ERROR: Helper plist not found at ${HELPER_PLIST}"
    exit 1
fi

if [ ! -d "${APP_BUNDLE}" ]; then
    echo "ERROR: App bundle not found at ${APP_BUNDLE}"
    echo "       Build the universal app first: npm run tauri build -- --target universal-apple-darwin"
    exit 1
fi

# Verify helper is universal
HELPER_ARCH=$(file "${HELPER_BIN}")
if [[ ! "${HELPER_ARCH}" == *"universal"* ]]; then
    echo "WARNING: Helper binary may not be universal:"
    echo "         ${HELPER_ARCH}"
    echo "         Continuing anyway..."
fi

echo "  All prerequisites found."

# --- Clean previous build ---
echo "[2/7] Preparing build directory..."
rm -rf "${WORK_DIR}"
mkdir -p "${PAYLOAD_DIR}/Library/PrivilegedHelperTools"
mkdir -p "${PAYLOAD_DIR}/Library/LaunchDaemons"
mkdir -p "${PAYLOAD_DIR}/Applications"
mkdir -p "${SCRIPTS_DIR}"

# --- Copy files into payload ---
echo "[3/7] Assembling payload..."

# Helper binary
cp "${HELPER_BIN}" "${PAYLOAD_DIR}/Library/PrivilegedHelperTools/com.ipswitcher.helper"
chmod 544 "${PAYLOAD_DIR}/Library/PrivilegedHelperTools/com.ipswitcher.helper"

# LaunchDaemon plist
cp "${HELPER_PLIST}" "${PAYLOAD_DIR}/Library/LaunchDaemons/com.ipswitcher.helper.plist"
chmod 644 "${PAYLOAD_DIR}/Library/LaunchDaemons/com.ipswitcher.helper.plist"

# App bundle
cp -R "${APP_BUNDLE}" "${PAYLOAD_DIR}/Applications/${APP_NAME}.app"

# --- Bundle the uninstaller inside the .app ---
echo "[4/7] Bundling uninstaller..."

UNINSTALL_DIR="${PAYLOAD_DIR}/Applications/${APP_NAME}.app/Contents/Resources"
mkdir -p "${UNINSTALL_DIR}"

cat > "${UNINSTALL_DIR}/uninstall.command" << 'UNINSTALL_EOF'
#!/bin/bash
echo "============================================"
echo " Uninstalling It Hurts When IP..."
echo "============================================"
echo ""

# Stop and unload helper daemon
if launchctl list 2>/dev/null | grep -q "com.ipswitcher.helper"; then
    echo "Stopping helper daemon..."
    sudo launchctl unload /Library/LaunchDaemons/com.ipswitcher.helper.plist 2>/dev/null
fi

# Remove helper binary
if [ -f "/Library/PrivilegedHelperTools/com.ipswitcher.helper" ]; then
    sudo rm -f /Library/PrivilegedHelperTools/com.ipswitcher.helper
    echo "  Removed helper binary"
fi

# Remove LaunchDaemon plist
if [ -f "/Library/LaunchDaemons/com.ipswitcher.helper.plist" ]; then
    sudo rm -f /Library/LaunchDaemons/com.ipswitcher.helper.plist
    echo "  Removed LaunchDaemon"
fi

# Remove socket
if [ -S "/var/run/com.ipswitcher.helper.sock" ]; then
    sudo rm -f /var/run/com.ipswitcher.helper.sock
    echo "  Removed socket"
fi

# Remove app from Applications
if [ -d "/Applications/It Hurts When IP.app" ]; then
    sudo rm -rf "/Applications/It Hurts When IP.app"
    echo "  Removed app from Applications"
fi

# Remove app data
if [ -d ~/Library/Application\ Support/com.ipswitcher.switcher ]; then
    rm -rf ~/Library/Application\ Support/com.ipswitcher.switcher
    echo "  Removed app data"
fi

# Remove log files
sudo rm -f /var/log/com.ipswitcher.helper.log 2>/dev/null
sudo rm -f /var/log/com.ipswitcher.helper.error.log 2>/dev/null

echo ""
echo "Uninstall complete. You can close this window."
UNINSTALL_EOF

chmod +x "${UNINSTALL_DIR}/uninstall.command"

# --- Create preinstall script (handles upgrades) ---
echo "[5/7] Creating installer scripts..."

cat > "${SCRIPTS_DIR}/preinstall" << 'PREINSTALL_EOF'
#!/bin/bash
# Pre-install: Clean up any existing installation for upgrade

# Kill the running app if open
killall "It Hurts When IP" 2>/dev/null || true

# Stop and unload existing helper daemon
if launchctl list 2>/dev/null | grep -q "com.ipswitcher.helper"; then
    launchctl unload /Library/LaunchDaemons/com.ipswitcher.helper.plist 2>/dev/null || true
fi

# Remove old socket
rm -f /var/run/com.ipswitcher.helper.sock 2>/dev/null || true

# Remove old helper binary (will be replaced)
rm -f /Library/PrivilegedHelperTools/com.ipswitcher.helper 2>/dev/null || true

exit 0
PREINSTALL_EOF

chmod +x "${SCRIPTS_DIR}/preinstall"

# --- Create postinstall script ---
cat > "${SCRIPTS_DIR}/postinstall" << 'POSTINSTALL_EOF'
#!/bin/bash
# Post-install: Set permissions and start helper daemon

# Ensure correct ownership and permissions
chown root:wheel /Library/PrivilegedHelperTools/com.ipswitcher.helper
chmod 544 /Library/PrivilegedHelperTools/com.ipswitcher.helper

chown root:wheel /Library/LaunchDaemons/com.ipswitcher.helper.plist
chmod 644 /Library/LaunchDaemons/com.ipswitcher.helper.plist

# Load and start the helper daemon
launchctl load /Library/LaunchDaemons/com.ipswitcher.helper.plist

exit 0
POSTINSTALL_EOF

chmod +x "${SCRIPTS_DIR}/postinstall"

# --- Ad-hoc sign the app bundle ---
echo "[6/7] Ad-hoc signing binaries..."
codesign -s - --force --deep "${PAYLOAD_DIR}/Applications/${APP_NAME}.app"
codesign -s - --force "${PAYLOAD_DIR}/Library/PrivilegedHelperTools/com.ipswitcher.helper"

# --- Build the .pkg ---
echo "[7/7] Building .pkg..."

pkgbuild \
    --root "${PAYLOAD_DIR}" \
    --identifier "${PKG_ID}" \
    --version "${VERSION}" \
    --scripts "${SCRIPTS_DIR}" \
    --install-location "/" \
    "${OUTPUT_PKG}"

# --- Clean up ---
rm -rf "${WORK_DIR}"

echo ""
echo "=========================================="
echo " SUCCESS: ${OUTPUT_PKG} created"
echo "=========================================="
echo ""
echo " Distribute this single .pkg file."
echo " Users double-click → enter password → done."
echo ""
echo " To uninstall: Right-click app → Show Package Contents"
echo "               → Contents → Resources → uninstall.command"
echo ""