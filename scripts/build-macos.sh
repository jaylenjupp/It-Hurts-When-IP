#!/usr/bin/env bash
# build-macos.sh — build a verified, universal It Hurts When IP .pkg into release-out/.
#
# Runs on: the Mac Studio (build host), from any directory; it cd's to the
# repo root itself. It only builds and packages: it never installs the pkg,
# never uses sudo, never loads the helper and never touches /Library,
# /Applications or the helper socket.
#
# Usage:
#   ./scripts/build-macos.sh           # full build; the universal build takes several minutes
#   ./scripts/build-macos.sh --help
#
# Steps:
#   1. Preflight: Rust targets aarch64- and x86_64-apple-darwin installed,
#      build tools present, node_modules current (runs `npm ci` if missing or
#      older than package-lock.json).
#   2. Records the git SHA and dirty flag. A dirty build cannot be released.
#   3. Universal helper: cargo build for both targets, lipo, ad-hoc codesign.
#   4. Universal app: tauri build --target universal-apple-darwin --bundles app,
#      with a fresh IHWIP_BUILD_NONCE so the build stamp is regenerated.
#   5. ./build-pkg.sh, then moves ItHurtsWhenIP-<version>.pkg into release-out/.
#   6. Writes release-out/ItHurtsWhenIP-<version>.pkg.buildinfo (key=value).
#   7. Runs scripts/verify-pkg.sh on the pkg; the build fails if it fails.
#
# Exit codes: 0 = built and verified, 1 = build or verification failed,
#             2 = usage error.

set -euo pipefail

die() { echo "build-macos: ERROR: $*" >&2; exit 1; }
step() { echo; echo "==> $*"; }

dirty_warning() {
    cat >&2 <<'EOF'

!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!
!!  WARNING: the working tree is DIRTY (uncommitted changes).  !!
!!  This build does not match any commit and CANNOT be         !!
!!  released. Commit, then rebuild for a releasable pkg.       !!
!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!

EOF
}

case "${1:-}" in
    -h|--help)
        awk 'NR > 1 && /^$/ { exit } NR > 1 { sub(/^# ?/, ""); print }' "$0"
        exit 0
        ;;
esac
if [[ $# -gt 0 ]]; then
    echo "build-macos: unexpected argument: $1" >&2
    echo "usage: $0 [--help]" >&2
    exit 2
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"
[[ -f build-pkg.sh && -f src-tauri/tauri.conf.json && -f helper-tool/Cargo.toml ]] \
    || die "not the It Hurts When IP repo root: ${REPO_ROOT}"

RUST_TARGETS=(aarch64-apple-darwin x86_64-apple-darwin)
HELPER_BIN_NAME="ithurtswhenip-helper"
HELPER_OUT="helper-tool/com.ithurtswhenip.helper"
APP_BUNDLE="src-tauri/target/universal-apple-darwin/release/bundle/macos/It Hurts When IP.app"
OUT_DIR="release-out"

# --- 1. Preflight -------------------------------------------------------------
step "Preflight"
for tool in git rustup cargo npm lipo codesign pkgbuild pkgutil shasum; do
    command -v "${tool}" >/dev/null 2>&1 || die "required tool not found: ${tool}"
done

INSTALLED_TARGETS="$(rustup target list --installed)"
for target in "${RUST_TARGETS[@]}"; do
    grep -qx "${target}" <<<"${INSTALLED_TARGETS}" \
        || die "Rust target ${target} is not installed. Nothing was installed automatically; ask the owner to approve: rustup target add ${target}"
done
echo "Rust targets: ${RUST_TARGETS[*]}"

# Same parsing as scripts/ship/release-dev.sh.
VERSION="$(grep -m1 -Eo '"version"[[:space:]]*:[[:space:]]*"[^"]+"' src-tauri/tauri.conf.json | sed -E 's/.*"([^"]+)"$/\1/' || true)"
[[ -n "${VERSION}" ]] || die "could not read version from src-tauri/tauri.conf.json"
# build-pkg.sh names the pkg from its own VERSION line; catch drift before a long build.
PKG_SCRIPT_VERSION="$(sed -nE 's/^VERSION="([^"]+)"$/\1/p' build-pkg.sh)"
[[ "${PKG_SCRIPT_VERSION}" == "${VERSION}" ]] \
    || die "build-pkg.sh VERSION (${PKG_SCRIPT_VERSION:-unset}) differs from tauri.conf.json (${VERSION}); fix with scripts/bump-version.sh"
PKG_NAME="ItHurtsWhenIP-${VERSION}.pkg"
echo "Version: ${VERSION}"

# npm writes node_modules/.package-lock.json on every install; use it as the marker.
NODE_MARKER="node_modules/.package-lock.json"
[[ -e "${NODE_MARKER}" ]] || NODE_MARKER="node_modules"
if [[ ! -d node_modules || package-lock.json -nt "${NODE_MARKER}" ]]; then
    echo "node_modules missing or older than package-lock.json: running npm ci"
    npm ci
else
    echo "node_modules: up to date"
fi

# --- 2. Git state ---------------------------------------------------------------
step "Git state"
SHA="$(git rev-parse --short=7 HEAD)"
SHA="${SHA:0:7}" # build.rs stamps exactly 7 chars; git may print more if 7 is ambiguous
DIRTY=false
if [[ -n "$(git --no-optional-locks status --porcelain)" ]]; then
    DIRTY=true
fi
echo "Commit: ${SHA}  dirty: ${DIRTY}"
if ${DIRTY}; then
    dirty_warning
fi

# --- 3. Universal helper ------------------------------------------------------------
step "Helper: universal build"
for target in "${RUST_TARGETS[@]}"; do
    cargo build --release --target "${target}" --manifest-path helper-tool/Cargo.toml
done
lipo -create \
    "helper-tool/target/aarch64-apple-darwin/release/${HELPER_BIN_NAME}" \
    "helper-tool/target/x86_64-apple-darwin/release/${HELPER_BIN_NAME}" \
    -output "${HELPER_OUT}"
codesign -s - --force "${HELPER_OUT}"
echo "Helper: ${HELPER_OUT} ($(lipo -archs "${HELPER_OUT}"))"

# --- 4. Universal app -------------------------------------------------------------------
step "App: universal Tauri build (takes several minutes)"
# Any new value makes build.rs re-run, so the stamp (SHA, dirty, time) is fresh.
NONCE="$(date -u +%Y%m%dT%H%M%SZ)-$$-${RANDOM}"
IHWIP_BUILD_NONCE="${NONCE}" npm run tauri build -- --target universal-apple-darwin --bundles app
[[ -d "${APP_BUNDLE}" ]] || die "app bundle not found after tauri build: ${APP_BUNDLE}"
echo "App: ${APP_BUNDLE}"

# --- 5. Package -----------------------------------------------------------------------------
step "Package: ./build-pkg.sh"
# build-pkg.sh writes ./${PKG_NAME} and would overwrite a pkg already there
# (e.g. a copy of a published release). Move it aside instead of losing it.
if [[ -e "${PKG_NAME}" ]]; then
    mkdir -p "${OUT_DIR}/previous"
    MOVED_TO="${OUT_DIR}/previous/${PKG_NAME%.pkg}.moved-$(date -u +%Y%m%dT%H%M%SZ).pkg"
    mv "${PKG_NAME}" "${MOVED_TO}"
    echo "NOTE: an existing ./${PKG_NAME} was moved to ${MOVED_TO} so build-pkg.sh doesn't overwrite it"
fi
bash ./build-pkg.sh # not executable in git (mode 100644)
[[ -f "${PKG_NAME}" ]] || die "build-pkg.sh did not produce ${PKG_NAME}"
mkdir -p "${OUT_DIR}"
PKG_OUT="${OUT_DIR}/${PKG_NAME}"
mv -f "${PKG_NAME}" "${PKG_OUT}"

# --- 6. Build info ----------------------------------------------------------------------------
SHA256="$(shasum -a 256 "${PKG_OUT}" | awk '{ print $1 }')"
BUILDINFO="${PKG_OUT}.buildinfo"
{
    echo "version=${VERSION}"
    echo "git_sha=${SHA}"
    echo "dirty=${DIRTY}"
    echo "built_utc=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo "sha256=${SHA256}"
} >"${BUILDINFO}"
echo "Wrote ${BUILDINFO}"

# --- 7. Verify ----------------------------------------------------------------------------------
step "Verify: scripts/verify-pkg.sh ${PKG_OUT}"
if ! "${SCRIPT_DIR}/verify-pkg.sh" "${PKG_OUT}"; then
    echo >&2
    echo "build-macos: BUILD FAILED: verify-pkg.sh reported FAIL lines for ${PKG_OUT} (see above). Do not ship this pkg." >&2
    exit 1
fi

echo
echo "=========================================="
echo " Built and verified"
echo "   pkg:     ${REPO_ROOT}/${PKG_OUT}"
echo "   sha256:  ${SHA256}"
echo "   git sha: ${SHA}"
echo "   dirty:   ${DIRTY}"
echo "=========================================="
if ${DIRTY}; then
    dirty_warning
fi
