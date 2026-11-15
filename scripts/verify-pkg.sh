#!/usr/bin/env bash
# verify-pkg.sh — check an It Hurts When IP .pkg WITHOUT installing it.
#
# Runs on: the Mac Studio (build host), from any directory. It only expands
# the pkg into a temp dir (removed on exit) and reads files: it never
# installs, never uses sudo and never touches /Library or /Applications.
#
# Usage:
#   ./scripts/verify-pkg.sh <path/to/ItHurtsWhenIP-X.Y.Z.pkg>
#
# Prints one PASS / FAIL / SKIP line per check, then a summary.
# Exit codes: 0 = every check passed, 1 = at least one check failed,
#             2 = usage error (no argument, too many, missing file).
#
# If <pkg>.buildinfo (written by scripts/build-macos.sh) sits next to the
# pkg, the build stamp and SHA-256 are checked against it; otherwise SKIP.

set -euo pipefail

APP_NAME="It Hurts When IP"
APP_ID="com.ithurtswhenip.desktop"
PKG_ID="com.ithurtswhenip.pkg"
HELPER_LABEL="com.ithurtswhenip.helper"
HELPER_INSTALL_PATH="/Library/PrivilegedHelperTools/${HELPER_LABEL}"
RESCUE_NAME="ithurtswhenip-rescue.command"

APP_REL="Applications/${APP_NAME}.app"
HELPER_REL="Library/PrivilegedHelperTools/${HELPER_LABEL}"
PLIST_REL="Library/LaunchDaemons/${HELPER_LABEL}.plist"
# Every path the payload may contain outside the app bundle itself.
EXPECTED_ENTRIES="Applications
${APP_REL}
Library
Library/LaunchDaemons
${PLIST_REL}
Library/PrivilegedHelperTools
${HELPER_REL}"

usage_error() {
    echo "verify-pkg: $*" >&2
    echo "usage: $0 <path/to/ItHurtsWhenIP-X.Y.Z.pkg>" >&2
    exit 2
}

case "${1:-}" in
    -h|--help)
        awk 'NR > 1 && /^$/ { exit } NR > 1 { sub(/^# ?/, ""); print }' "$0"
        exit 0
        ;;
esac
[[ $# -eq 1 ]] || usage_error "expected exactly one argument, got $#"
[[ -f "$1" ]] || usage_error "pkg not found: $1"

PKG="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

PASSES=0
FAILS=0
SKIPS=0
pass() { PASSES=$((PASSES + 1)); printf 'PASS  %s\n' "$*"; }
fail() { FAILS=$((FAILS + 1)); printf 'FAIL  %s\n' "$*"; }
skip() { SKIPS=$((SKIPS + 1)); printf 'SKIP  %s\n' "$*"; }
info() { printf 'INFO  %s\n' "$*"; }

summary_and_exit() {
    echo
    echo "Summary: ${PASSES} passed, ${FAILS} failed, ${SKIPS} skipped — ${PKG}"
    if [[ ${FAILS} -gt 0 ]]; then
        echo "RESULT: FAIL"
        exit 1
    fi
    echo "RESULT: PASS"
    exit 0
}

# Prints a plist value (works for XML and binary plists); non-zero if missing.
plist_get() { /usr/libexec/PlistBuddy -c "Print :$1" "$2" 2>/dev/null; }

# First line of some (possibly multi-line) command output, for FAIL lines.
first_line() { printf '%s\n' "$1" | awk 'NR == 1 { print }'; }

# Joins lines with ", ", for FAIL lines.
join_lines() { printf '%s\n' "$1" | awk 'NF { printf "%s%s", (n++ ? ", " : ""), $0 } END { print "" }'; }

# Same parsing as scripts/ship/release-dev.sh.
EXPECTED_VERSION="$(grep -m1 -Eo '"version"[[:space:]]*:[[:space:]]*"[^"]+"' "${REPO_ROOT}/src-tauri/tauri.conf.json" \
    | sed -E 's/.*"([^"]+)"$/\1/' || true)"

WORK="$(mktemp -d)"
trap 'rm -rf "${WORK}"' EXIT
trap 'exit 130' INT TERM HUP

echo "Verifying ${PKG}"
echo "Expected version (src-tauri/tauri.conf.json): ${EXPECTED_VERSION:-<unreadable>}"
echo

# --- Expand ------------------------------------------------------------------
EXP="${WORK}/expanded"
if pkgutil --expand-full "${PKG}" "${EXP}" >/dev/null 2>&1; then
    EXPAND_HOW="pkgutil --expand-full"
else
    # Older pkgutil without --expand-full: expand, then unpack the archives.
    rm -rf "${EXP}"
    EXPAND_HOW="pkgutil --expand + tar"
    if ! pkgutil --expand "${PKG}" "${EXP}" >/dev/null 2>&1; then
        fail "expand pkg — pkgutil could not expand it (not a flat package?)"
        summary_and_exit
    fi
    for part in Payload Scripts; do
        if [[ -f "${EXP}/${part}" ]]; then
            mv "${EXP}/${part}" "${WORK}/${part}.archive"
            mkdir "${EXP}/${part}"
            if ! tar -xf "${WORK}/${part}.archive" -C "${EXP}/${part}" 2>/dev/null; then
                fail "expand pkg — could not unpack ${part} with tar"
                summary_and_exit
            fi
        fi
    done
fi
PAYLOAD="${EXP}/Payload"
SCRIPTS="${EXP}/Scripts"
PKGINFO="${EXP}/PackageInfo"
if [[ ! -d "${PAYLOAD}" || ! -f "${PKGINFO}" ]]; then
    fail "expand pkg — no Payload/PackageInfo (expected a component pkg from pkgbuild)"
    summary_and_exit
fi
pass "expand pkg (${EXPAND_HOW})"

APP="${PAYLOAD}/${APP_REL}"
HELPER="${PAYLOAD}/${HELPER_REL}"
DAEMON_PLIST="${PAYLOAD}/${PLIST_REL}"
INFO_PLIST="${APP}/Contents/Info.plist"

# --- Package metadata ----------------------------------------------------------
PKGINFO_ID="$(xmllint --xpath 'string(/pkg-info/@identifier)' "${PKGINFO}" 2>/dev/null || true)"
PKGINFO_VERSION="$(xmllint --xpath 'string(/pkg-info/@version)' "${PKGINFO}" 2>/dev/null || true)"
if [[ "${PKGINFO_ID}" == "${PKG_ID}" ]]; then
    pass "pkg identifier is ${PKG_ID}"
else
    fail "pkg identifier is ${PKG_ID} — PackageInfo says '${PKGINFO_ID}'"
fi
if [[ -n "${EXPECTED_VERSION}" && "${PKGINFO_VERSION}" == "${EXPECTED_VERSION}" ]]; then
    pass "pkg version ${PKGINFO_VERSION} equals the app version"
else
    fail "pkg version equals the app version — PackageInfo '${PKGINFO_VERSION}', tauri.conf.json '${EXPECTED_VERSION}'"
fi
# Relocatable bundles make the installer silently no-op when the bundle id is
# already registered elsewhere; build-pkg.sh turns BundleIsRelocatable off.
# pkgbuild records that as an empty <relocate/>; the pkg-info relocatable
# attribute (install location) must not be true either.
PKGINFO_RELOCATABLE="$(xmllint --xpath 'string(/pkg-info/@relocatable)' "${PKGINFO}" 2>/dev/null || true)"
RELOCATED_BUNDLES="$(xmllint --xpath 'count(/pkg-info/relocate/bundle)' "${PKGINFO}" 2>/dev/null || echo '?')"
if [[ "${PKGINFO_RELOCATABLE}" == "false" && "${RELOCATED_BUNDLES}" == "0" ]]; then
    pass "app bundle not relocatable (PackageInfo relocatable=\"false\", no bundles under <relocate>)"
else
    fail "app bundle not relocatable — PackageInfo relocatable='${PKGINFO_RELOCATABLE}', bundles under <relocate>: ${RELOCATED_BUNDLES}"
fi

# --- Payload layout ----------------------------------------------------------------
ACTUAL_ENTRIES="$(cd "${PAYLOAD}" && find . -mindepth 1 \( -path "./${APP_REL}" -print -prune \) -o -print \
    | sed 's|^\./||' | LC_ALL=C sort)"
SORTED_EXPECTED="$(printf '%s\n' "${EXPECTED_ENTRIES}" | LC_ALL=C sort)"
UNEXPECTED="$(LC_ALL=C comm -13 <(printf '%s\n' "${SORTED_EXPECTED}") <(printf '%s\n' "${ACTUAL_ENTRIES}"))"
MISSING="$(LC_ALL=C comm -23 <(printf '%s\n' "${SORTED_EXPECTED}") <(printf '%s\n' "${ACTUAL_ENTRIES}"))"
if [[ -z "${UNEXPECTED}" && -z "${MISSING}" && -d "${APP}" && -f "${HELPER}" && -f "${DAEMON_PLIST}" ]]; then
    pass "payload is exactly: ${APP_REL}, ${HELPER_REL}, ${PLIST_REL}"
else
    problems=""
    [[ -z "${UNEXPECTED}" ]] || problems="unexpected: $(join_lines "${UNEXPECTED}")"
    [[ -z "${MISSING}" ]] || problems="${problems:+${problems}; }missing: $(join_lines "${MISSING}")"
    [[ -n "${problems}" ]] || problems="wrong file types (app must be a directory, helper and plist files)"
    fail "payload is exactly the app, helper and LaunchDaemon plist — ${problems}"
fi

junk_check() {
    local label="$1"
    shift
    local hits
    hits="$(cd "${PAYLOAD}" && find . "$@" -print | sed 's|^\./||' | awk 'NR <= 5')" || true
    if [[ -z "${hits}" ]]; then
        pass "${label}"
    else
        fail "${label} — found: $(join_lines "${hits}")"
    fi
}
junk_check "no AppleDouble ._* files in payload" -name '._*'
junk_check "no .DS_Store files in payload" -name '.DS_Store'
junk_check "nothing from node_modules in payload" -path '*node_modules*'

# --- Installer scripts -----------------------------------------------------------------
for script in preinstall postinstall; do
    file="${SCRIPTS}/${script}"
    label="${script} present, executable, passes bash -n"
    if [[ ! -f "${file}" ]]; then
        fail "${label} — missing from the pkg's Scripts"
    elif [[ ! -x "${file}" ]]; then
        fail "${label} — not executable"
    elif ! err="$(bash -n "${file}" 2>&1)"; then
        fail "${label} — bash -n: $(first_line "${err}")"
    else
        pass "${label}"
    fi
done

# --- Architectures -----------------------------------------------------------------------
APP_EXEC=""
APP_EXEC_NAME="$(plist_get CFBundleExecutable "${INFO_PLIST}" || true)"
[[ -z "${APP_EXEC_NAME}" ]] || APP_EXEC="${APP}/Contents/MacOS/${APP_EXEC_NAME}"

check_universal() {
    local label="$1" file="$2" archs extras arch
    if [[ -z "${file}" ]]; then
        fail "${label} is universal — CFBundleExecutable missing from Info.plist"
        return
    fi
    if [[ ! -f "${file}" ]]; then
        fail "${label} is universal — not found: ${file#"${PAYLOAD}"/}"
        return
    fi
    archs="$(lipo -archs "${file}" 2>/dev/null)" || archs=""
    if [[ " ${archs} " == *" x86_64 "* && " ${archs} " == *" arm64 "* ]]; then
        extras=""
        for arch in ${archs}; do
            [[ "${arch}" == "x86_64" || "${arch}" == "arm64" ]] || extras="${extras} ${arch}"
        done
        pass "${label} is universal: x86_64 arm64${extras}"
    else
        fail "${label} is universal — lipo -archs: ${archs:-<not a Mach-O file>}"
    fi
}
check_universal "app executable (${APP_EXEC_NAME:-?})" "${APP_EXEC}"
check_universal "helper" "${HELPER}"

# --- Code signatures -------------------------------------------------------------------------
if out="$(codesign --verify --deep --strict "${APP}" 2>&1)"; then
    pass "codesign --verify --deep --strict: app bundle"
else
    fail "codesign --verify --deep --strict: app bundle — $(first_line "${out}")"
fi
if out="$(codesign --verify --strict "${HELPER}" 2>&1)"; then
    pass "codesign --verify --strict: helper"
else
    fail "codesign --verify --strict: helper — $(first_line "${out}")"
fi

# --- Property lists ------------------------------------------------------------------------------
lint_check() {
    local label="$1" file="$2" out
    if out="$(plutil -lint "${file}" 2>&1)"; then
        pass "plutil -lint: ${label}"
    else
        fail "plutil -lint: ${label} — $(first_line "${out}")"
    fi
}
lint_check "app Info.plist" "${INFO_PLIST}"
lint_check "LaunchDaemon plist" "${DAEMON_PLIST}"

DAEMON_LABEL="$(plist_get Label "${DAEMON_PLIST}" || true)"
if [[ "${DAEMON_LABEL}" == "${HELPER_LABEL}" ]]; then
    pass "LaunchDaemon Label is ${HELPER_LABEL}"
else
    fail "LaunchDaemon Label is ${HELPER_LABEL} — got '${DAEMON_LABEL}'"
fi
# launchd runs Program if set, otherwise ProgramArguments[0].
DAEMON_PROGRAM="$(plist_get Program "${DAEMON_PLIST}" || plist_get ProgramArguments:0 "${DAEMON_PLIST}" || true)"
if [[ "${DAEMON_PROGRAM}" == "${HELPER_INSTALL_PATH}" ]]; then
    pass "LaunchDaemon program is ${HELPER_INSTALL_PATH}"
else
    fail "LaunchDaemon program is ${HELPER_INSTALL_PATH} — got '${DAEMON_PROGRAM}'"
fi

# --- App identity ------------------------------------------------------------------------------------
APP_VERSION="$(plist_get CFBundleShortVersionString "${INFO_PLIST}" || true)"
if [[ -n "${EXPECTED_VERSION}" && "${APP_VERSION}" == "${EXPECTED_VERSION}" ]]; then
    pass "CFBundleShortVersionString ${APP_VERSION} equals tauri.conf.json version"
else
    fail "CFBundleShortVersionString equals tauri.conf.json version — app '${APP_VERSION}', tauri.conf.json '${EXPECTED_VERSION}'"
fi
BUNDLE_ID="$(plist_get CFBundleIdentifier "${INFO_PLIST}" || true)"
if [[ "${BUNDLE_ID}" == "${APP_ID}" ]]; then
    pass "CFBundleIdentifier is ${APP_ID}"
else
    fail "CFBundleIdentifier is ${APP_ID} — got '${BUNDLE_ID}'"
fi
info "LSMinimumSystemVersion = $(plist_get LSMinimumSystemVersion "${INFO_PLIST}" || echo '<not set>')"

# --- Build stamp (needs <pkg>.buildinfo from scripts/build-macos.sh) -----------------------------
BUILDINFO="${PKG}.buildinfo"
buildinfo_get() { awk -v key="$1" 'index($0, key "=") == 1 { print substr($0, length(key) + 2); exit }' "${BUILDINFO}"; }

# The app embeds "IHWIP_BUILD_STAMP:<sha>:<dirty>:<utc>" (src-tauri/src/build_info.rs).
# Every arch slice must carry the stamp, so each one is thinned out and searched.
check_stamp_slices() {
    local exe="$1" stamp="$2" label="$3" archs arch slice found="" missing=""
    archs="$(lipo -archs "${exe}" 2>/dev/null)" || archs=""
    if [[ -z "${archs}" ]]; then
        fail "${label} — lipo cannot read the app executable"
        return
    fi
    for arch in ${archs}; do
        slice="${WORK}/stamp-slice-${arch}"
        # A thin (non-fat) executable is its own single slice.
        lipo "${exe}" -thin "${arch}" -output "${slice}" 2>/dev/null || slice="${exe}"
        if LC_ALL=C grep -a -q -F "${stamp}" "${slice}"; then
            found="${found} ${arch}"
        else
            missing="${missing} ${arch}"
        fi
    done
    if [[ -z "${missing}" ]]; then
        pass "${label} (slices:${found})"
    else
        fail "${label} — missing in slice(s):${missing} (stale app build or dirty mismatch?)"
    fi
}

if [[ -f "${BUILDINFO}" ]]; then
    BI_SHA="$(buildinfo_get git_sha)"
    BI_DIRTY="$(buildinfo_get dirty)"
    BI_SUM="$(buildinfo_get sha256)"
    STAMP="IHWIP_BUILD_STAMP:${BI_SHA}:${BI_DIRTY}:"
    STAMP_LABEL="build stamp: every slice of the app executable contains ${STAMP}"
    if [[ ! "${BI_SHA}" =~ ^[0-9a-f]{7}$ ]]; then
        fail "${STAMP_LABEL} — buildinfo git_sha is '${BI_SHA}' (expected 7 hex chars)"
    elif [[ "${BI_DIRTY}" != "true" && "${BI_DIRTY}" != "false" ]]; then
        fail "${STAMP_LABEL} — buildinfo dirty is '${BI_DIRTY}' (expected true or false)"
    elif [[ -z "${APP_EXEC}" || ! -f "${APP_EXEC}" ]]; then
        fail "${STAMP_LABEL} — app executable not found"
    else
        check_stamp_slices "${APP_EXEC}" "${STAMP}" "${STAMP_LABEL}"
    fi
    PKG_SUM="$(shasum -a 256 "${PKG}" | awk '{ print $1 }')"
    if [[ -n "${BI_SUM}" && "${PKG_SUM}" == "${BI_SUM}" ]]; then
        pass "pkg SHA-256 equals buildinfo: ${PKG_SUM}"
    else
        fail "pkg SHA-256 equals buildinfo — pkg ${PKG_SUM}, buildinfo '${BI_SUM}'"
    fi
else
    skip "build stamp: no $(basename "${BUILDINFO}") next to the pkg"
    skip "pkg SHA-256 equals buildinfo: no buildinfo file"
fi

# --- Rescue script (FAIL once recovery/ithurtswhenip-rescue.command exists) -----------------------
RESCUE_SRC="${REPO_ROOT}/recovery/${RESCUE_NAME}"
RESCUE_BUNDLED="${APP}/Contents/Resources/${RESCUE_NAME}"
RESCUE_LABEL="rescue script in Contents/Resources, executable, identical to recovery/${RESCUE_NAME}"
if [[ ! -f "${RESCUE_SRC}" ]]; then
    skip "${RESCUE_LABEL} (recovery/${RESCUE_NAME} not in the repo yet)"
elif [[ ! -f "${RESCUE_BUNDLED}" ]]; then
    fail "${RESCUE_LABEL} — missing from the app bundle"
elif [[ ! -x "${RESCUE_BUNDLED}" ]]; then
    fail "${RESCUE_LABEL} — not executable"
elif ! cmp -s "${RESCUE_SRC}" "${RESCUE_BUNDLED}"; then
    fail "${RESCUE_LABEL} — differs from the repo copy"
else
    pass "${RESCUE_LABEL}"
fi

summary_and_exit
