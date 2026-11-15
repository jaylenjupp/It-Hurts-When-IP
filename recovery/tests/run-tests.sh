#!/bin/bash
#
# run-tests.sh - test suite for recovery/ithurtswhenip-rescue.command
#
# Runs on: the Mac Studio (or any Mac), from any folder, without root:
#     ./recovery/tests/run-tests.sh                  # run every test
#     ./recovery/tests/run-tests.sh --update-golden  # rewrite golden plans, then review them by eye
#
# Safety:
#  - Everything happens inside one fresh `mktemp -d` folder, deleted at the end
#    (kept, and its path printed, if a test fails).
#  - The rescue script only ever runs with IHWIP_ROOT=<fake tree in that folder>
#    and fake tools, under a PATH whose first entry holds failing stubs for
#    launchctl, pkgutil, networksetup, pkill, killall, osascript and sudo
#    (each prints "REAL TOOL REACHED", logs to a marker file and exits 99).
#  - The only runs without IHWIP_ROOT are --help and --version.
#  - Guards about real root or a missing IHWIP_ROOT are tested by sourcing the
#    script and calling apply_guards / mode_allowed directly, which runs nothing.

set -u

HERE=$(cd -P -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P) || exit 2
RECOVERY=$(cd -P -- "$HERE/.." && pwd -P) || exit 2
REPO=$(cd -P -- "$RECOVERY/.." && pwd -P) || exit 2
SCRIPT="$RECOVERY/ithurtswhenip-rescue.command"
FAKES="$HERE/fakes"
GOLDEN="$HERE/golden"
FAKE_NS="$REPO/tools/fake-networksetup/fake-networksetup"
FIXTURES_ROOT="$REPO/tools/fake-networksetup/fixtures"
NOW="20261115T120000Z"
TAB=$'\t'
NL=$'\n'

UPDATE_GOLDEN=0
for arg in "$@"; do
    case "$arg" in
        --update-golden) UPDATE_GOLDEN=1 ;;
        *)
            echo "usage: $0 [--update-golden]" >&2
            exit 2
            ;;
    esac
done

for f in "$SCRIPT" "$FAKE_NS" "$FAKES/launchctl" "$FAKES/pkgutil" "$FAKES/pgrep" "$FAKES/pkill"; do
    if [ ! -x "$f" ]; then
        echo "missing or not executable: $f" >&2
        exit 2
    fi
done

# ---------------------------------------------------------------- suite folder

PASSED=0
FAILED=0
ABORTED=0
FAILED_NAMES=""

SUITE=$(mktemp -d "${TMPDIR:-/tmp}/ihwip-rescue-tests.XXXXXX") || {
    echo "mktemp -d failed" >&2
    exit 2
}
SUITE=$(cd -P -- "$SUITE" && pwd -P) || exit 2
case "$SUITE" in
    */ihwip-rescue-tests.??????) ;;
    *)
        echo "refusing to start: unexpected temp folder: $SUITE" >&2
        exit 2
        ;;
esac
for f in "$SUITE"/* "$SUITE"/.[!.]*; do
    if [ -e "$f" ] || [ -L "$f" ]; then
        echo "refusing to start: temp folder is not fresh: $SUITE" >&2
        exit 2
    fi
done

# shellcheck disable=SC2329 # invoked by the EXIT trap below
cleanup() {
    case "$SUITE" in
        */ihwip-rescue-tests.??????) ;;
        *) return 0 ;;
    esac
    if [ "$FAILED" -eq 0 ] && [ "$ABORTED" -eq 0 ]; then
        chmod -R u+w "$SUITE" 2>/dev/null
        rm -rf -- "$SUITE"
    else
        echo "test files kept for inspection: $SUITE"
    fi
}
trap cleanup EXIT

abort() {
    echo "BUG in run-tests.sh: $1" >&2
    ABORTED=1
    exit 3
}

MARKER="$SUITE/REAL_TOOL_REACHED.log"
ALL_OUT="$SUITE/all-output.log"
STUBS="$SUITE/stubs"
mkdir "$STUBS" || exit 2
for tool in launchctl pkgutil networksetup pkill killall osascript sudo; do
    {
        echo '#!/bin/bash'
        # shellcheck disable=SC2016 # $* belongs in the generated stub
        printf 'echo "REAL TOOL REACHED: %s $*"\n' "$tool"
        # shellcheck disable=SC2016
        printf 'echo "REAL TOOL REACHED: %s $*" >&2\n' "$tool"
        # shellcheck disable=SC2016
        printf 'echo "%s $*" >>%q\n' "$tool" "$MARKER"
        echo 'exit 99'
    } >"$STUBS/$tool"
    chmod 755 "$STUBS/$tool"
done
TEST_PATH="$STUBS:/usr/bin:/bin:/usr/sbin:/sbin"
: >"$ALL_OUT"

# ---------------------------------------------------------------- bookkeeping

T=""
ROOT=""
CUR=""
CUR_FAILS=0
CUR_DETAILS=""
TN=0
RC=0
ENVV=()

begin() {
    TN=$((TN + 1))
    CUR="$1"
    CUR_FAILS=0
    CUR_DETAILS=""
    T="$SUITE/t$TN"
    ROOT="$T/root"
    mkdir "$T" "$T/tmp" "$T/home" "$ROOT" "$ROOT/.fake" || abort "cannot create $T"
    : >"$T/inventory.txt"
    reset_env
}

end() {
    if [ "$CUR_FAILS" -eq 0 ]; then
        printf 'ok    %s\n' "$CUR"
        PASSED=$((PASSED + 1))
    else
        printf 'FAIL  %s\n%s' "$CUR" "$CUR_DETAILS"
        FAILED=$((FAILED + 1))
        FAILED_NAMES="$FAILED_NAMES  $CUR$NL"
    fi
}

bad() {
    CUR_DETAILS="$CUR_DETAILS      $1$NL"
    CUR_FAILS=$((CUR_FAILS + 1))
}

rel() {
    printf '%s' "${1#"$T"/}"
}

# ---------------------------------------------------------------- environment / runs

reset_env() {
    ENVV=(
        "PATH=$TEST_PATH"
        "HOME=$T/home"
        "TMPDIR=$T/tmp"
        "IHWIP_ROOT=$ROOT"
        "IHWIP_NETWORKSETUP=$FAKE_NS"
        "IHWIP_LAUNCHCTL=$FAKES/launchctl"
        "IHWIP_PKGUTIL=$FAKES/pkgutil"
        "IHWIP_PGREP=$FAKES/pgrep"
        "IHWIP_PKILL=$FAKES/pkill"
        "IHWIP_NOW=$NOW"
        "IHWIP_FAKE_STATE=$T/ns-state"
        "IHWIP_FAKE_FIXTURES=$FIXTURES_ROOT/default"
        "IHWIP_FAKE_CALLS=$T/calls.log"
    )
}

env_unset() {
    local e
    local -a keep=()
    for e in "${ENVV[@]}"; do
        case "$e" in
            "$1="*) ;;
            *) keep[${#keep[@]}]="$e" ;;
        esac
    done
    ENVV=("${keep[@]}")
}

env_set() {
    env_unset "${1%%=*}"
    ENVV[${#ENVV[@]}]="$1"
}

env_has() {
    local e
    for e in "${ENVV[@]}"; do
        case "$e" in
            "$1="*) return 0 ;;
        esac
    done
    return 1
}

env_get() {
    local e
    for e in "${ENVV[@]}"; do
        case "$e" in
            "$1="*)
                printf '%s' "${e#*=}"
                return 0
                ;;
        esac
    done
    return 1
}

collect_output() {
    {
        printf '=== %s\n' "$CUR"
        cat "$T/out.txt" "$T/err.txt"
    } >>"$ALL_OUT"
}

# run_rescue <args...>: the only way tests run the script with a fake root.
run_rescue() {
    local root a v val has_uninstall=0
    env_has IHWIP_ROOT || abort "run_rescue without IHWIP_ROOT"
    root=$(env_get IHWIP_ROOT)
    for a in "$@"; do
        if [ "$a" = "--uninstall" ]; then has_uninstall=1; fi
    done
    case "$root" in
        "$SUITE"/?*) ;;
        *)
            if [ $# -ne 1 ] || [ "$1" != "--plan" ]; then
                abort "IHWIP_ROOT outside the suite folder is only allowed with --plan"
            fi
            ;;
    esac
    if [ "$has_uninstall" -eq 1 ]; then
        for v in IHWIP_NETWORKSETUP IHWIP_LAUNCHCTL IHWIP_PKGUTIL IHWIP_PGREP IHWIP_PKILL; do
            val=$(env_get "$v") || abort "--uninstall run without $v"
            case "$val" in
                "$FAKES"/* | "$FAKE_NS") ;;
                *) abort "--uninstall run with a non-fake $v=$val" ;;
            esac
        done
    fi
    /usr/bin/env -i "${ENVV[@]}" /bin/bash "$SCRIPT" "$@" >"$T/out.txt" 2>"$T/err.txt" </dev/null
    RC=$?
    collect_output
}

# run_rescue_bare --help|--version: the only runs without IHWIP_ROOT.
run_rescue_bare() {
    if [ $# -ne 1 ] || { [ "$1" != "--help" ] && [ "$1" != "--version" ]; }; then
        abort "run_rescue_bare only allows --help or --version"
    fi
    /usr/bin/env -i "PATH=$TEST_PATH" "HOME=$T/home" /bin/bash "$SCRIPT" "$1" >"$T/out.txt" 2>"$T/err.txt" </dev/null
    RC=$?
    collect_output
}

# run_probe <euid> <action>: sources the script (main does not run) and calls
# only apply_guards and mode_allowed, then prints what they decided.
run_probe() {
    # shellcheck disable=SC2016 # expanded by the inner bash
    /usr/bin/env -i "${ENVV[@]}" /bin/bash -c '
        . "$1" || exit 90
        apply_guards "$2"
        mode_allowed "$3" "$2"
        printf "R=[%s] TEST_MODE=%s NS=%s LAUNCHCTL=%s PKGUTIL=%s PGREP=%s PKILL=%s ROOTVAR=%s\n" \
            "$R" "$TEST_MODE" "$NS" "$LAUNCHCTL" "$PKGUTIL" "$PGREP" "$PKILL" "${IHWIP_ROOT-unset}"
    ' probe "$SCRIPT" "$1" "$2" >"$T/out.txt" 2>"$T/err.txt" </dev/null
    RC=$?
    collect_output
}

# ---------------------------------------------------------------- assertions

assert_rc() {
    if [ "$RC" -ne "$1" ]; then
        bad "exit code $RC, expected $1; stderr: $(tr '\n' ' ' <"$T/err.txt" | cut -c1-300)"
    fi
}

out_has() {
    grep -Fq -- "$1" "$T/out.txt" || bad "stdout lacks: $1"
}

out_lacks() {
    if grep -Fq -- "$1" "$T/out.txt"; then bad "stdout unexpectedly has: $1"; fi
}

err_has() {
    grep -Fq -- "$1" "$T/err.txt" || bad "stderr lacks: $1"
}

err_empty() {
    if [ -s "$T/err.txt" ]; then bad "stderr not empty: $(tr '\n' ' ' <"$T/err.txt" | cut -c1-300)"; fi
}

out_count() {
    local c
    c=$(grep -Fc -- "$2" "$T/out.txt")
    if [ "$c" -ne "$1" ]; then bad "expected $1 line(s) with '$2' in stdout, found $c"; fi
}

file_has() {
    if [ ! -f "$1" ]; then
        bad "missing file: $(rel "$1")"
    elif ! grep -Fq -- "$2" "$1"; then
        bad "$(rel "$1") lacks: $2"
    fi
}

file_lacks() {
    if [ -f "$1" ] && grep -Fq -- "$2" "$1"; then bad "$(rel "$1") unexpectedly has: $2"; fi
}

present() {
    local p
    for p in "$@"; do
        if [ ! -e "$p" ] && [ ! -L "$p" ]; then bad "missing: $(rel "$p")"; fi
    done
}

absent() {
    local p
    for p in "$@"; do
        if [ -e "$p" ] || [ -L "$p" ]; then bad "still present: $(rel "$p")"; fi
    done
}

same_file() {
    if ! cmp -s "$1" "$2"; then bad "differs: $(rel "$1") vs $(rel "$2")"; fi
}

calls_have() {
    if [ ! -f "$T/calls.log" ] || ! grep -Fq -- "$1" "$T/calls.log"; then bad "fake calls log lacks: $1"; fi
}

calls_lack() {
    if [ -f "$T/calls.log" ] && grep -Fq -- "$1" "$T/calls.log"; then bad "fake calls log unexpectedly has: $1"; fi
}

no_mutating_calls() {
    if [ -f "$T/calls.log" ] && grep -Eq '^(launchctl (bootout|unload)|pkgutil --forget|pkill) ' "$T/calls.log"; then
        bad "mutating fake call(s): $(grep -E '^(launchctl (bootout|unload)|pkgutil --forget|pkill) ' "$T/calls.log" | tr '\n' ';')"
    fi
    ns_no_writes
}

# ns_no_writes: every networksetup call (if any) was a read.
ns_no_writes() {
    local log="$T/ns-state/calls.log" other
    [ -f "$log" ] || return 0
    other=$(grep -Ev '^networksetup -(listallnetworkservices|getinfo)( |$)' "$log")
    if [ -n "$other" ]; then bad "networksetup calls that are not read-only: $(printf '%s' "$other" | tr '\n' ';')"; fi
}

# ns_read_only: networksetup was called, and only to read.
ns_read_only() {
    if [ ! -f "$T/ns-state/calls.log" ]; then
        bad "the networksetup fake was never called"
        return
    fi
    ns_no_writes
}

inventory_gone() {
    local p
    while IFS= read -r p; do
        [ -n "$p" ] || continue
        if [ -e "$ROOT$p" ] || [ -L "$ROOT$p" ]; then bad "inventory item still present: $p"; fi
    done <"$T/inventory.txt"
}

# tree_sum <dir>: every path with type, mode, size and content hash, sorted.
tree_sum() {
    (
        cd "$1" || exit 1
        find . -print | LC_ALL=C sort | while IFS= read -r p; do
            if [ -L "$p" ]; then
                printf '%s\tlink\t%s\n' "$p" "$(readlink "$p")"
            elif [ -d "$p" ]; then
                printf '%s\tdir\t%s\n' "$p" "$(stat -f '%Lp' "$p")"
            else
                printf '%s\tfile\t%s\t%s\n' "$p" "$(stat -f '%Lp %z' "$p")" "$(md5 -q "$p" 2>/dev/null || echo unreadable)"
            fi
        done
    )
}

golden() {
    local g="$GOLDEN/$1.txt" norm="$T/$1.norm" esc
    esc=$(printf '%s' "$ROOT" | sed 's/[][\.*^$#/]/\\&/g')
    sed "s#$esc#<ROOT>#g" "$T/out.txt" >"$norm"
    if [ "$UPDATE_GOLDEN" -eq 1 ]; then
        cp "$norm" "$g"
        CUR_DETAILS="$CUR_DETAILS      updated golden: ${g#"$REPO"/}$NL"
        return
    fi
    if [ ! -f "$g" ]; then
        bad "golden missing: ${g#"$REPO"/} (run with --update-golden and review it)"
        return
    fi
    if ! diff -u "$g" "$norm" >"$T/$1.diff"; then
        bad "plan differs from ${g#"$REPO"/}:$NL$(head -n 40 "$T/$1.diff" | sed 's/^/        /')"
    fi
}

# ---------------------------------------------------------------- fake trees

put() {
    mkdir -p "$ROOT/${1%/*}"
    printf '%s\n' "$2" >"$ROOT/$1"
}

inv() {
    printf '/%s\n' "$1" >>"$T/inventory.txt"
}

inv_put() {
    put "$1" "$2"
    inv "$1"
}

inv_dir() {
    mkdir -p "$ROOT/$1"
    inv "$1"
}

state_add() {
    printf '%s\n' "$2" >>"$ROOT/.fake/$1"
}

tree_base() {
    mkdir -p "$ROOT/Library/LaunchDaemons" "$ROOT/Library/PrivilegedHelperTools" \
        "$ROOT/Library/Application Support" "$ROOT/Library/Preferences" "$ROOT/Applications" \
        "$ROOT/var/run" "$ROOT/var/log" "$ROOT/Users/Shared" "$ROOT/Users/Guest/Library"
    put "Users/.localized" ""
    put "Library/LaunchDaemons/com.example.other.plist" "<plist>other daemon</plist>"
    put "Applications/Safari.app/Contents/Info.plist" "safari"
    put "var/log/system.log" "system log"
    state_add launchctl.loaded com.example.other
    state_add pkgutil.receipts com.apple.pkg.Core
}

tree_current_system() {
    inv_put "Library/LaunchDaemons/com.ithurtswhenip.helper.plist" "<plist>com.ithurtswhenip.helper</plist>"
    inv_put "Library/PrivilegedHelperTools/com.ithurtswhenip.helper" "helper binary"
    inv_put "var/run/com.ithurtswhenip.helper.sock" "" # a plain file stands in for the socket
    inv_put "var/log/com.ithurtswhenip.helper.log" "helper log"
    inv_put "var/log/com.ithurtswhenip.helper.error.log" "helper error log"
    inv_dir "Library/Application Support/com.ithurtswhenip.helper"
    put "Library/Application Support/com.ithurtswhenip.helper/journal.json" '{"schema":1,"services":[]}'
    inv_dir "Applications/It Hurts When IP.app"
    put "Applications/It Hurts When IP.app/Contents/MacOS/it-hurts-when-ip" "app binary"
    put "Applications/It Hurts When IP.app/Contents/Info.plist" "app info"
    state_add launchctl.loaded com.ithurtswhenip.helper
    state_add pkgutil.receipts com.ithurtswhenip.pkg
}

tree_legacy_system() {
    inv_put "Library/LaunchDaemons/com.ipswitcher.helper.plist" "<plist>com.ipswitcher.helper</plist>"
    inv_put "Library/PrivilegedHelperTools/com.ipswitcher.helper" "legacy helper binary"
    inv_put "var/run/com.ipswitcher.helper.sock" ""
    inv_put "var/log/com.ipswitcher.helper.log" "legacy helper log"
    inv_put "var/log/com.ipswitcher.helper.error.log" "legacy helper error log"
    state_add launchctl.loaded com.ipswitcher.helper
    state_add pkgutil.receipts com.ipswitcher.pkg
    state_add pkgutil.receipts com.ipswitcher.helper
}

# user_all_items <user> <id>: every per-user inventory location for one id.
user_all_items() {
    local h="Users/$1/Library" id="$2"
    inv_dir "$h/Application Support/$id"
    put "$h/Application Support/$id/config.json" "{\"user\":\"$1\",\"id\":\"$id\"}"
    inv_dir "$h/Caches/$id"
    put "$h/Caches/$id/Cache.db" "cache"
    inv_dir "$h/WebKit/$id"
    put "$h/WebKit/$id/WebsiteData/LocalStorage/data" "webkit"
    inv_dir "$h/HTTPStorages/$id"
    put "$h/HTTPStorages/$id/httpstorages.sqlite" "http storage"
    inv_put "$h/HTTPStorages/$id.binarycookies" "cookies"
    inv_put "$h/Preferences/$id.plist" "<plist>prefs $1 $id</plist>"
    chmod 600 "$ROOT/$h/Preferences/$id.plist"
    inv_dir "$h/Saved Application State/$id.savedState"
    put "$h/Saved Application State/$id.savedState/windows.plist" "window state"
    inv_dir "$h/Logs/$id"
    put "$h/Logs/$id/app.log" "app log"
}

tree_noise() {
    put "Users/alice/Library/Caches/com.apple.Safari/Cache.db" "safari cache"
    put "Users/alice/Documents/It Hurts When IP notes.txt" "my notes, outside the search roots"
    put "Users/alice/Library/Caches/claude-cli-nodejs/-Users-alice-it-hurts-when-ip/session.txt" "claude cache"
    put "Users/alice/Library/Containers/com.example.other/ithurtswhenip-cache" "inside another app's container"
    put "Users/Mary Jane/Library/Mobile Documents/com~apple~CloudDocs/ItHurtsWhenIP-1.13.1.pkg" "old installer in iCloud"
}

tree_full() {
    local h="Users/Mary Jane/Library"
    tree_base
    tree_current_system
    user_all_items alice com.ithurtswhenip.desktop
    inv_dir "$h/Application Support/com.ithurtswhenip.desktop"
    put "$h/Application Support/com.ithurtswhenip.desktop/config.json" '{"user":"Mary Jane"}'
    inv_put "$h/Preferences/com.ithurtswhenip.desktop.plist" "<plist>mary prefs</plist>"
    inv_dir "$h/Caches/it-hurts-when-ip"
    put "$h/Caches/it-hurts-when-ip/Cache.db" "cache"
    inv_dir "$h/WebKit/it-hurts-when-ip"
    put "$h/WebKit/it-hurts-when-ip/data" "webkit"
    inv_put "$h/Preferences/it-hurts-when-ip.plist" "<plist>mary old prefs</plist>"
    inv_dir "$h/Caches/com.ithurtswhenip.app"
    put "$h/Caches/com.ithurtswhenip.app/Cache.db" "cache"
    tree_noise
    put "Library/Application Support/com.ithurtswhenip.unknown-thing/data.txt" "unknown, report only"
}

tree_legacy_user() {
    local h="Users/alice/Library"
    inv_dir "$h/Application Support/com.ipswitcher.switcher"
    put "$h/Application Support/com.ipswitcher.switcher/config.json" '{"legacy":true}'
    inv_put "$h/Preferences/com.ipswitcher.switcher.plist" "<plist>legacy prefs</plist>"
    inv_dir "$h/Caches/com.ipswitcher.app"
    put "$h/Caches/com.ipswitcher.app/Cache.db" "legacy cache"
}

tree_legacy_only() {
    tree_base
    tree_legacy_system
    tree_legacy_user
    put "Users/alice/Library/Caches/com.apple.Safari/Cache.db" "safari cache"
    put "Applications/IP Switcher.app/Contents/Info.plist" "old bundle name, report only"
}

tree_mixed() {
    tree_full
    tree_legacy_system
    tree_legacy_user
}

tree_empty() {
    tree_base
    mkdir -p "$ROOT/Users/alice/Library/Preferences"
    put "Users/alice/Library/Caches/com.apple.Safari/Cache.db" "safari cache"
}

BK_REL="Users/Shared/ItHurtsWhenIP-backups/$NOW"

# ---------------------------------------------------------------- tests

test_plan_full() {
    local p
    begin "1   --plan on full: golden, tree unchanged, no mutating calls, no backup folder"
    tree_full
    tree_sum "$ROOT" >"$T/before.sum"
    run_rescue --plan
    assert_rc 0
    err_empty
    golden plan-full
    tree_sum "$ROOT" >"$T/after.sum"
    same_file "$T/before.sum" "$T/after.sum"
    no_mutating_calls
    ns_read_only
    for p in "$ROOT/Users/Shared"/ItHurtsWhenIP-backups*; do
        if [ -e "$p" ] || [ -L "$p" ]; then bad "--plan created a backup folder: $(rel "$p")"; fi
    done
    end
}

test_plan_others() {
    local tree
    for tree in legacy-only mixed empty; do
        begin "2   --plan on $tree: golden, tree unchanged"
        case "$tree" in
            legacy-only) tree_legacy_only ;;
            mixed) tree_mixed ;;
            empty) tree_empty ;;
        esac
        tree_sum "$ROOT" >"$T/before.sum"
        run_rescue --plan
        assert_rc 0
        err_empty
        golden "plan-$tree"
        tree_sum "$ROOT" >"$T/after.sum"
        same_file "$T/before.sum" "$T/after.sum"
        no_mutating_calls
        ns_read_only
        end
    done
}

test_uninstall_full() {
    local bk p
    begin "3   --uninstall --yes --network leave on full: removed, backed up, verified"
    tree_full
    cp -Rp "$ROOT" "$T/pristine"
    run_rescue --uninstall --yes --network leave
    assert_rc 0
    err_empty
    inventory_gone
    out_lacks "FAILED"
    out_has "# failed steps: 0"
    out_has "VERIFY${TAB}ok${TAB}launchd:com.ithurtswhenip.helper"
    out_has "VERIFY${TAB}ok${TAB}receipt:com.ithurtswhenip.pkg"
    out_has "OK${TAB}NETWORK${TAB}choice${TAB}-${TAB}leave: no network settings were changed"
    bk="$ROOT/$BK_REL"
    out_has "# backup folder: /$BK_REL"
    present "$bk/plan.txt" "$bk/rescue.log" "$bk/network-before.txt"
    for p in \
        "Library/LaunchDaemons/com.ithurtswhenip.helper.plist" \
        "var/log/com.ithurtswhenip.helper.log" \
        "var/log/com.ithurtswhenip.helper.error.log" \
        "Library/Application Support/com.ithurtswhenip.helper/journal.json"; do
        same_file "$bk/system/$p" "$T/pristine/$p"
    done
    for p in \
        "alice/Library/Application Support/com.ithurtswhenip.desktop/config.json" \
        "alice/Library/Preferences/com.ithurtswhenip.desktop.plist" \
        "alice/Library/Logs/com.ithurtswhenip.desktop/app.log" \
        "Mary Jane/Library/Application Support/com.ithurtswhenip.desktop/config.json" \
        "Mary Jane/Library/Preferences/com.ithurtswhenip.desktop.plist" \
        "Mary Jane/Library/Preferences/it-hurts-when-ip.plist"; do
        same_file "$bk/users/$p" "$T/pristine/Users/$p"
    done
    if [ "$(stat -f '%Lp' "$bk/users/alice/Library/Preferences/com.ithurtswhenip.desktop.plist" 2>/dev/null)" != "644" ]; then
        bad "backup is not readable by users (chmod -R a+rX)"
    fi
    absent "$bk/system/Library/PrivilegedHelperTools" "$bk/system/Applications" "$bk/system/var/run" \
        "$bk/users/alice/Library/Caches" "$bk/users/alice/Library/WebKit" "$bk/users/alice/Library/HTTPStorages" \
        "$bk/users/alice/Library/Saved Application State" "$bk/users/Mary Jane/Library/Caches" \
        "$bk/users/Mary Jane/Library/WebKit"
    file_has "$bk/plan.txt" "# mode: uninstall"
    file_has "$bk/plan.txt" "REMOVE${TAB}app${TAB}/Applications/It Hurts When IP.app${TAB}-"
    file_has "$bk/rescue.log" "OK${TAB}REMOVE${TAB}app${TAB}/Applications/It Hurts When IP.app${TAB}-"
    file_has "$bk/rescue.log" "# failed steps: 0"
    file_has "$bk/network-before.txt" '$ networksetup -getinfo "Dante Primary"'
    file_has "$bk/network-before.txt" "Manual Configuration"
    present "$ROOT/Library/Application Support/com.ithurtswhenip.unknown-thing/data.txt"
    out_has "LEFTOVER${TAB}path${TAB}/Library/Application Support/com.ithurtswhenip.unknown-thing${TAB}reported only, not removed"
    out_has "# leftovers (reported only, never removed): 1"
    same_file "$ROOT/Users/alice/Documents/It Hurts When IP notes.txt" "$T/pristine/Users/alice/Documents/It Hurts When IP notes.txt"
    out_lacks "Documents"
    out_lacks "claude-cli-nodejs"
    out_lacks "Mobile Documents"
    out_lacks "com.example.other/ithurtswhenip-cache"
    present "$ROOT/Users/alice/Library/Caches/claude-cli-nodejs/-Users-alice-it-hurts-when-ip/session.txt" \
        "$ROOT/Users/alice/Library/Containers/com.example.other/ithurtswhenip-cache" \
        "$ROOT/Users/Mary Jane/Library/Mobile Documents/com~apple~CloudDocs/ItHurtsWhenIP-1.13.1.pkg" \
        "$ROOT/Users/alice/Library/Caches/com.apple.Safari/Cache.db" \
        "$ROOT/Library/LaunchDaemons/com.example.other.plist" \
        "$ROOT/Applications/Safari.app/Contents/Info.plist"
    file_has "$ROOT/.fake/launchctl.loaded" "com.example.other"
    file_lacks "$ROOT/.fake/launchctl.loaded" "com.ithurtswhenip.helper"
    file_has "$ROOT/.fake/pkgutil.receipts" "com.apple.pkg.Core"
    file_lacks "$ROOT/.fake/pkgutil.receipts" "com.ithurtswhenip.pkg"
    calls_have "launchctl bootout system/com.ithurtswhenip.helper"
    calls_lack "launchctl unload"
    calls_have "pkgutil --forget com.ithurtswhenip.pkg"
    calls_lack "pkgutil --forget com.apple.pkg.Core"
    ns_read_only
    end
}

test_uninstall_legacy_and_mixed() {
    local bk
    begin "4a  --uninstall --yes on legacy-only: legacy files, daemon and receipts cleaned"
    tree_legacy_only
    cp -Rp "$ROOT" "$T/pristine"
    run_rescue --uninstall --yes --network leave
    assert_rc 0
    err_empty
    inventory_gone
    out_lacks "FAILED"
    bk="$ROOT/$BK_REL"
    same_file "$bk/system/Library/LaunchDaemons/com.ipswitcher.helper.plist" "$T/pristine/Library/LaunchDaemons/com.ipswitcher.helper.plist"
    same_file "$bk/users/alice/Library/Application Support/com.ipswitcher.switcher/config.json" \
        "$T/pristine/Users/alice/Library/Application Support/com.ipswitcher.switcher/config.json"
    calls_have "launchctl bootout system/com.ipswitcher.helper"
    calls_have "pkgutil --forget com.ipswitcher.pkg"
    calls_have "pkgutil --forget com.ipswitcher.helper"
    calls_lack "launchctl bootout system/com.ithurtswhenip.helper"
    present "$ROOT/Applications/IP Switcher.app/Contents/Info.plist"
    out_has "LEFTOVER${TAB}path${TAB}/Applications/IP Switcher.app${TAB}reported only, not removed"
    ns_read_only
    end

    begin "4b  --uninstall --yes on mixed: current and legacy cleaned in one run"
    tree_mixed
    run_rescue --uninstall --yes --network leave
    assert_rc 0
    err_empty
    inventory_gone
    out_lacks "FAILED"
    calls_have "launchctl bootout system/com.ithurtswhenip.helper"
    calls_have "launchctl bootout system/com.ipswitcher.helper"
    calls_have "pkgutil --forget com.ithurtswhenip.pkg"
    calls_have "pkgutil --forget com.ipswitcher.pkg"
    calls_have "pkgutil --forget com.ipswitcher.helper"
    present "$ROOT/$BK_REL/users/alice/Library/Application Support/com.ipswitcher.switcher/config.json"
    ns_read_only
    end
}

test_bootout_fallback() {
    begin "5   bootout fails -> unload fallback, still exit 0"
    tree_full
    env_set IHWIP_FAKE_LAUNCHCTL_NO_BOOTOUT=1
    run_rescue --uninstall --yes
    assert_rc 0
    calls_have "launchctl bootout system/com.ithurtswhenip.helper"
    calls_have "launchctl unload $ROOT/Library/LaunchDaemons/com.ithurtswhenip.helper.plist"
    out_has "OK${TAB}STOP${TAB}launchd${TAB}com.ithurtswhenip.helper${TAB}stopped (unload fallback; bootout exit 5: Boot-out failed: 5: Input/output error)"
    file_lacks "$ROOT/.fake/launchctl.loaded" "com.ithurtswhenip.helper"
    inventory_gone
    end
}

count_entries() {
    local e n=0
    for e in "$1"/* "$1"/.[!.]*; do
        if [ -e "$e" ] || [ -L "$e" ]; then n=$((n + 1)); fi
    done
    printf '%s' "$n"
}

test_idempotent() {
    local n
    begin "6   second --uninstall --yes on a clean tree: exit 0, nothing to remove"
    tree_full
    run_rescue --uninstall --yes
    assert_rc 0
    run_rescue --uninstall --yes
    assert_rc 0
    err_empty
    out_has "NOTE${TAB}info${TAB}-${TAB}nothing to remove"
    out_has "# backup folder: none created (nothing to remove)"
    out_has "# failed steps: 0"
    out_lacks "FAILED"
    out_lacks "${TAB}REMOVE${TAB}"
    n=$(count_entries "$ROOT/Users/Shared/ItHurtsWhenIP-backups")
    if [ "$n" != "1" ]; then bad "expected 1 backup folder, found $n"; fi
    end
}

test_remove_fails() {
    local j="Library/Application Support/com.ithurtswhenip.helper"
    begin "7   a removal fails: exit 1, failure listed, rest removed, its backup kept"
    tree_full
    cp -Rp "$ROOT" "$T/pristine"
    chmod 555 "$ROOT/$j"
    run_rescue --uninstall --yes
    assert_rc 1
    out_count 2 "FAILED${TAB}REMOVE${TAB}dir${TAB}/$j${TAB}-${TAB}could not remove"
    out_count 2 "VERIFY${TAB}FAILED${TAB}/$j"
    out_has "# failed steps: 1; still present after verify: 1"
    present "$ROOT/$j/journal.json"
    same_file "$ROOT/$BK_REL/system/$j/journal.json" "$T/pristine/$j/journal.json"
    absent "$ROOT/Library/LaunchDaemons/com.ithurtswhenip.helper.plist" \
        "$ROOT/Applications/It Hurts When IP.app" \
        "$ROOT/Users/alice/Library/Application Support/com.ithurtswhenip.desktop"
    chmod 755 "$ROOT/$j"
    end

    local pref="Users/alice/Library/Preferences/com.ithurtswhenip.desktop.plist"
    begin "7b  a backup fails: that item is not removed, exit 1, rest removed"
    tree_full
    chmod 000 "$ROOT/$pref"
    run_rescue --uninstall --yes
    assert_rc 1
    out_has "FAILED${TAB}BACKUP${TAB}file${TAB}/$pref${TAB}users/alice/Library/Preferences/com.ithurtswhenip.desktop.plist${TAB}backup failed:"
    out_has "FAILED${TAB}REMOVE${TAB}file${TAB}/$pref${TAB}-${TAB}not removed because its backup failed"
    out_has "VERIFY${TAB}FAILED${TAB}/$pref"
    present "$ROOT/$pref"
    absent "$ROOT/Users/alice/Library/Application Support/com.ithurtswhenip.desktop" \
        "$ROOT/Library/LaunchDaemons/com.ithurtswhenip.helper.plist"
    chmod 600 "$ROOT/$pref"
    end
}

test_app_running() {
    begin "8a  app still running after pkill: bundle kept, exit 1, rest removed (10 s wait)"
    tree_full
    state_add procs.running it-hurts-when-ip
    env_set IHWIP_FAKE_PKILL_NOOP=1
    run_rescue --uninstall --yes
    assert_rc 1
    out_has "QUIT${TAB}app${TAB}it-hurts-when-ip${TAB}running"
    out_has "FAILED${TAB}QUIT${TAB}app${TAB}it-hurts-when-ip${TAB}running${TAB}still running after 10 s"
    out_has "FAILED${TAB}REMOVE${TAB}app${TAB}/Applications/It Hurts When IP.app${TAB}-${TAB}the app is still running"
    calls_have "pkill -x it-hurts-when-ip"
    present "$ROOT/Applications/It Hurts When IP.app/Contents/MacOS/it-hurts-when-ip"
    absent "$ROOT/Library/LaunchDaemons/com.ithurtswhenip.helper.plist" \
        "$ROOT/Users/alice/Library/Application Support/com.ithurtswhenip.desktop"
    end

    begin "8b  app running and quits on pkill: bundle removed, exit 0"
    tree_full
    state_add procs.running it-hurts-when-ip
    run_rescue --uninstall --yes
    assert_rc 0
    out_has "OK${TAB}QUIT${TAB}app${TAB}it-hurts-when-ip${TAB}quit"
    calls_have "pkill -x it-hurts-when-ip"
    absent "$ROOT/Applications/It Hurts When IP.app"
    inventory_gone
    end
}

refusal() { # <stderr text> <args...>: expects exit 2
    local want="$1"
    shift
    run_rescue "$@"
    assert_rc 2
    err_has "$want"
    reset_env
}

test_refusals() {
    begin "9   refusals exit 2 and leave the tree unchanged"
    tree_full
    tree_sum "$ROOT" >"$T/before.sum"
    refusal "--uninstall needs confirmation" --uninstall
    env_unset IHWIP_PKILL
    refusal "test mode needs IHWIP_PKILL" --plan
    env_set "IHWIP_ROOT=/"
    refusal "IHWIP_ROOT must not be /" --plan
    env_set "IHWIP_ROOT=$T/does-not-exist"
    refusal "IHWIP_ROOT is not an existing directory" --plan
    env_set "IHWIP_ROOT="
    refusal "IHWIP_ROOT must be an absolute path" --plan
    mkdir "$T/looks-real"
    ln -s /Library "$T/looks-real/Library"
    env_set "IHWIP_ROOT=$T/looks-real"
    refusal "IHWIP_ROOT points at the real system" --plan
    env_set "IHWIP_LAUNCHCTL=/bin/launchctl"
    refusal "IHWIP_LAUNCHCTL points into a system tools folder" --plan
    env_set "IHWIP_NOW=yesterday"
    refusal "IHWIP_NOW must look like" --plan
    refusal "unknown option: --frobnicate" --frobnicate
    err_has "Usage:"
    refusal "choose one action" --plan --uninstall
    refusal "missing value for --network" --plan --network
    refusal "unknown --network value: bogus" --plan --network bogus
    refusal "--network dhcp:Wi-Fi is not available in this build yet" --uninstall --yes --network dhcp:Wi-Fi
    refusal "--network dhcp:A,B is not available in this build yet" --uninstall --yes --network leave --network "dhcp:A,B"
    refusal "--network restore-journal is not available in this build yet" --uninstall --yes --network restore-journal
    refusal "--wait-pid needs a positive whole number" --plan --wait-pid abc
    refusal "--wait-pid needs a positive whole number" --plan --wait-pid 0
    refusal "--wait-pid is not available in this build yet" --uninstall --yes --wait-pid 123
    refusal "network repair arrives in a later version" --repair soft
    refusal "missing value for --log" --plan --log
    refusal "--log: folder does not exist" --plan --log "$T/no-such-dir/rescue.log"
    tree_sum "$ROOT" >"$T/after.sum"
    same_file "$T/before.sum" "$T/after.sum"
    no_mutating_calls
    absent "$T/no-such-dir"
    end
}

test_guard_probes() {
    begin "9b  guards via sourced functions: root ignores overrides; no root -> no --uninstall"
    run_probe 0 plan
    assert_rc 0
    out_has "R=[] TEST_MODE=0 NS=/usr/sbin/networksetup LAUNCHCTL=/bin/launchctl PKGUTIL=/usr/sbin/pkgutil PGREP=/usr/bin/pgrep PKILL=/usr/bin/pkill ROOTVAR=unset"
    err_has "running as root: ignoring IHWIP_* test overrides"
    run_probe 0 uninstall
    assert_rc 0
    out_has "TEST_MODE=0"
    run_probe 501 uninstall
    assert_rc 0
    out_has "R=[$ROOT] TEST_MODE=1 NS=$FAKE_NS LAUNCHCTL=$FAKES/launchctl"
    env_unset IHWIP_ROOT
    run_probe 501 uninstall
    assert_rc 2
    err_has "sudo bash ithurtswhenip-rescue.command --uninstall"
    err_has "IHWIP_ROOT is not set: ignoring IHWIP_* test overrides"
    if [ -s "$T/out.txt" ]; then bad "probe printed after a refusal"; fi
    run_probe 501 plan
    assert_rc 0
    out_has "R=[] TEST_MODE=0 NS=/usr/sbin/networksetup"
    reset_env
    end
}

test_no_action() {
    begin "10  no action, stdout not a terminal: prints the plan, exits 2"
    tree_full
    tree_sum "$ROOT" >"$T/before.sum"
    run_rescue
    assert_rc 2
    out_has "# mode: plan"
    out_has "# plan only: nothing has been changed"
    out_has "REMOVE${TAB}app${TAB}/Applications/It Hurts When IP.app${TAB}-"
    err_has "no action given"
    tree_sum "$ROOT" >"$T/after.sum"
    same_file "$T/before.sum" "$T/after.sum"
    no_mutating_calls
    ns_read_only
    end
}

test_manual_services() {
    begin "11a manual services (default fixtures): only Dante Primary, selected=no"
    tree_empty
    run_rescue --plan
    assert_rc 0
    out_count 1 "NETWORK${TAB}manual${TAB}"
    out_has "NETWORK${TAB}manual${TAB}Dante Primary${TAB}ip=192.168.10.21 mask=255.255.255.0 router=- selected=no"
    out_has "NETWORK${TAB}choice${TAB}-${TAB}leave"
    end

    begin "11b manual services (real Mac Studio fixtures): none listed, leave still offered"
    tree_empty
    env_set "IHWIP_FAKE_FIXTURES=$FIXTURES_ROOT/mac-studio"
    run_rescue --plan
    assert_rc 0
    out_count 0 "NETWORK${TAB}manual${TAB}"
    out_has "NETWORK${TAB}choice${TAB}-${TAB}leave"
    out_lacks "could not read"
    end
}

test_names_with_spaces() {
    local fx bk
    begin "12  spaces and / in service and user names parse correctly"
    fx="$T/fixtures"
    cp -R "$FIXTURES_ROOT/default" "$fx"
    printf '%s\n' "Manual Configuration" "IP address: 10.1.2.3" "Subnet mask: 255.255.0.0" "Router: 10.1.0.1" \
        "IPv6: Automatic" "IPv6 IP address: none" "IPv6 Router: none" "Ethernet Address: 00:11:22:33:44:55" \
        >"$fx/info/USB_10_100_1000_LAN.txt"
    env_set "IHWIP_FAKE_FIXTURES=$fx"
    tree_full
    run_rescue --uninstall --yes
    assert_rc 0
    out_count 2 "NETWORK${TAB}manual${TAB}"
    out_has "NETWORK${TAB}manual${TAB}USB 10/100/1000 LAN${TAB}ip=10.1.2.3 mask=255.255.0.0 router=10.1.0.1 selected=no"
    out_has "NETWORK${TAB}manual${TAB}Dante Primary${TAB}ip=192.168.10.21 mask=255.255.255.0 router=- selected=no"
    bk="$ROOT/$BK_REL"
    file_has "$bk/network-before.txt" '$ networksetup -getinfo "USB 10/100/1000 LAN"'
    file_has "$bk/network-before.txt" '$ networksetup -getinfo "Old Adapter"'
    file_has "$bk/network-before.txt" "IP address: 10.1.2.3"
    out_has "OK${TAB}REMOVE${TAB}dir${TAB}/Users/Mary Jane/Library/Application Support/com.ithurtswhenip.desktop${TAB}-"
    absent "$ROOT/Users/Mary Jane/Library/Application Support/com.ithurtswhenip.desktop"
    present "$bk/users/Mary Jane/Library/Application Support/com.ithurtswhenip.desktop/config.json"
    ns_read_only
    end
}

test_symlink_parent() {
    local c="Users/alice/Library/Caches"
    begin "15a symlinked Library/Caches: item skipped, its target survives, exit 1"
    tree_full
    mv "$ROOT/$c" "$ROOT/elsewhere-caches"
    ln -s "$ROOT/elsewhere-caches" "$ROOT/$c"
    run_rescue --plan
    assert_rc 0
    out_has "NOTE${TAB}info${TAB}/$c/com.ithurtswhenip.desktop${TAB}skipped: parent resolves outside expected location"
    out_lacks "REMOVE${TAB}dir${TAB}/$c/com.ithurtswhenip.desktop"
    run_rescue --uninstall --yes
    assert_rc 1
    out_has "FAILED${TAB}REMOVE${TAB}dir${TAB}/$c/com.ithurtswhenip.desktop${TAB}-${TAB}parent resolves outside expected location"
    present "$ROOT/elsewhere-caches/com.ithurtswhenip.desktop/Cache.db" "$ROOT/elsewhere-caches/com.apple.Safari/Cache.db"
    if [ ! -L "$ROOT/$c" ]; then bad "the Caches symlink itself should be left alone"; fi
    absent "$ROOT/Users/alice/Library/Application Support/com.ithurtswhenip.desktop" \
        "$ROOT/Library/LaunchDaemons/com.ithurtswhenip.helper.plist"
    end

    local a="Users/alice/Library/Application Support/com.ithurtswhenip.desktop"
    begin "15b item that is itself a symlink: only the link is removed and backed up"
    tree_full
    mv "$ROOT/$a" "$ROOT/elsewhere-support"
    ln -s "$ROOT/elsewhere-support" "$ROOT/$a"
    run_rescue --uninstall --yes
    assert_rc 0
    out_has "REMOVE${TAB}file${TAB}/$a${TAB}symlink: only the link is removed"
    if [ ! -L "$ROOT/$BK_REL/users/alice/Library/Application Support/com.ithurtswhenip.desktop" ]; then
        bad "the backup should hold the link itself, not a copy of its target"
    fi
    absent "$ROOT/$a"
    present "$ROOT/elsewhere-support/config.json"
    end
}

test_backup_folder() {
    local base="Users/Shared/ItHurtsWhenIP-backups"
    begin "16a backup base is a symlink: not used, ItHurtsWhenIP-backups.<ts> instead"
    tree_full
    mkdir "$ROOT/elsewhere-backups"
    ln -s "$ROOT/elsewhere-backups" "$ROOT/$base"
    run_rescue --plan
    assert_rc 0
    out_has "# backup folder: /$base.$NOW/$NOW"
    out_has "NOTE${TAB}info${TAB}backup-folder${TAB}/$base is a symlink, not a folder, or not owned by root: using /$base.$NOW instead"
    run_rescue --uninstall --yes
    assert_rc 0
    present "$ROOT/$base.$NOW/$NOW/plan.txt" "$ROOT/$base.$NOW/$NOW/system/Library/LaunchDaemons/com.ithurtswhenip.helper.plist"
    out_has "# backup folder: /$base.$NOW/$NOW"
    if [ "$(count_entries "$ROOT/elsewhere-backups")" != "0" ]; then bad "something was written through the symlink"; fi
    end

    begin "16b backup base is a plain file: not used"
    tree_full
    put "$base" "not a folder"
    run_rescue --plan
    assert_rc 0
    out_has "# backup folder: /$base.$NOW/$NOW"
    end

    begin "16c timestamp folder already exists: plain mkdir fails, -2 is used"
    tree_full
    mkdir -p "$ROOT/$base/$NOW"
    put "$base/$NOW/earlier.txt" "earlier run"
    run_rescue --uninstall --yes
    assert_rc 0
    present "$ROOT/$base/$NOW-2/plan.txt"
    out_has "# backup folder: /$base/$NOW-2"
    if [ "$(count_entries "$ROOT/$base/$NOW")" != "1" ]; then bad "the existing timestamp folder was written to"; fi
    present "$ROOT/$base/$NOW/earlier.txt"
    end
}

test_log_option() {
    begin "17  --log <path>: full output appended there"
    tree_full
    mkdir "$T/logs"
    run_rescue --uninstall --yes --log "$T/logs/custom.log"
    assert_rc 0
    file_has "$T/logs/custom.log" "# mode: uninstall"
    file_has "$T/logs/custom.log" "OK${TAB}REMOVE${TAB}app${TAB}/Applications/It Hurts When IP.app${TAB}-"
    file_has "$T/logs/custom.log" "# failed steps: 0"
    out_has "# log: $T/logs/custom.log"
    absent "$ROOT/$BK_REL/rescue.log"
    end
}

test_help_version() {
    begin "18  --help and --version (the only runs without IHWIP_ROOT)"
    run_rescue_bare --help
    assert_rc 0
    out_has "sudo bash ithurtswhenip-rescue.command --plan"
    out_has "--plan only reads: it never changes files"
    out_has "Exit codes:"
    out_has "--uninstall"
    out_has "--network leave"
    out_has "--wait-pid"
    out_has "--log <path>"
    out_has "--yes"
    run_rescue_bare --version
    assert_rc 0
    if [ "$(cat "$T/out.txt")" != "ithurtswhenip-rescue 1" ]; then bad "unexpected --version output: $(cat "$T/out.txt")"; fi
    end
}

test_syntax_and_lint() {
    local f
    begin "14  bash -n and shellcheck on the script, the runner and the fakes"
    for f in "$SCRIPT" "$HERE/run-tests.sh" "$FAKES"/*; do
        /bin/bash -n "$f" 2>"$T/bashn.txt" || bad "bash -n failed: ${f#"$REPO"/}: $(tr '\n' ' ' <"$T/bashn.txt")"
    done
    if command -v shellcheck >/dev/null 2>&1; then
        shellcheck "$SCRIPT" "$HERE/run-tests.sh" "$FAKES"/* >"$T/shellcheck.txt" 2>&1 ||
            bad "shellcheck: $(head -n 20 "$T/shellcheck.txt" | tr '\n' ' ')"
    else
        CUR_DETAILS="$CUR_DETAILS      (shellcheck not installed: skipped)$NL"
    fi
    end
}

test_no_real_tools() {
    local hit
    begin "13  PATH stubs: no run ever reached a real system tool"
    hit=$(/usr/bin/env -i "PATH=$TEST_PATH" /bin/bash -c 'command -v launchctl')
    if [ "$hit" != "$STUBS/launchctl" ]; then bad "stub PATH does not shadow launchctl (got: $hit)"; fi
    if grep -Fq "REAL TOOL REACHED" "$ALL_OUT"; then bad "output contains REAL TOOL REACHED"; fi
    if [ -e "$MARKER" ]; then bad "real-tool marker exists: $(tr '\n' ';' <"$MARKER")"; fi
    if [ "$(grep -c '^=== ' "$ALL_OUT")" -lt 20 ]; then bad "too few runs were captured to judge"; fi
    end
}

# ---------------------------------------------------------------- run

echo "rescue script tests (temp folder: $SUITE)"
test_plan_full
test_plan_others
test_uninstall_full
test_uninstall_legacy_and_mixed
test_bootout_fallback
test_idempotent
test_remove_fails
test_app_running
test_refusals
test_guard_probes
test_no_action
test_manual_services
test_names_with_spaces
test_symlink_parent
test_backup_folder
test_log_option
test_help_version
test_syntax_and_lint
test_no_real_tools

echo
echo "summary: $PASSED passed, $FAILED failed"
if [ "$FAILED" -ne 0 ]; then
    printf 'failed:\n%s' "$FAILED_NAMES"
    exit 1
fi
exit 0
