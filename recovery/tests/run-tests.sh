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
#    The journal parser is unit-tested the same way (sourced, parse only).
#  - The menu and the UNINSTALL prompt are driven from a file on stdin with the
#    test-mode-only IHWIP_TEST_STDIN_TTY=1.
#  - --wait-pid tests wait on `sleep` processes this runner starts itself. Every
#    exit path (EXIT/INT/TERM/HUP trap) stops those sleepers and waits for (or,
#    if stuck, stops) every stage B process the script started, so nothing
#    outlives the suite. The script itself never signals the waited process.

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

# ---------------------------------------------------------------- background processes

SLEEP_SECS=97 # an unusual duration, so a sleeper is recognised by its command line
SLEEPERS=""
SLEEPER=""
STAGE_B_PIDS=""
STAGE_B=""

proc_cmd() {
    ps -o command= -p "$1" 2>/dev/null | sed 's/[[:space:]]*$//'
}

sleeper_alive() {
    [ "$(proc_cmd "$1")" = "sleep $SLEEP_SECS" ]
}

stage_b_alive() {
    case "$(proc_cmd "$1")" in
        *"$SCRIPT"*) return 0 ;;
    esac
    return 1
}

# start_sleeper: a stand-in for the app process; sets SLEEPER.
start_sleeper() {
    sleep "$SLEEP_SECS" &
    SLEEPER=$!
    SLEEPERS="$SLEEPERS $SLEEPER"
}

stop_sleeper() {
    if sleeper_alive "$1"; then kill "$1" 2>/dev/null; fi
    wait "$1" 2>/dev/null
}

# shellcheck disable=SC2329 # invoked from cleanup (EXIT trap)
stop_all_procs() {
    local p k
    for p in $SLEEPERS; do
        if sleeper_alive "$p"; then kill "$p" 2>/dev/null; fi
    done
    for p in $STAGE_B_PIDS; do
        k=0
        while stage_b_alive "$p" && [ "$k" -lt 50 ]; do
            sleep 0.2
            k=$((k + 1))
        done
        if stage_b_alive "$p"; then
            kill "$p" 2>/dev/null
            echo "stage B process $p was still running at the end: stopped it" >&2
        fi
    done
}

# shellcheck disable=SC2329 # invoked by the EXIT trap below
cleanup() {
    stop_all_procs
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
trap 'exit 130' INT TERM HUP

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

# check_run <stdin file> <args...>: the rules every fake-root run must meet.
# A run that can change things (--uninstall, or a menu fed from a file) must
# use the fakes for every tool.
check_run() {
    local input="$1" root a v val can_change=0
    shift
    env_has IHWIP_ROOT || abort "run_rescue without IHWIP_ROOT"
    root=$(env_get IHWIP_ROOT)
    for a in "$@"; do
        if [ "$a" = "--uninstall" ]; then can_change=1; fi
    done
    if [ "$input" != "/dev/null" ]; then can_change=1; fi
    case "$root" in
        "$SUITE"/?*) ;;
        *)
            if [ $# -ne 1 ] || [ "$1" != "--plan" ] || [ "$input" != "/dev/null" ]; then
                abort "IHWIP_ROOT outside the suite folder is only allowed with --plan"
            fi
            ;;
    esac
    if [ "$can_change" -eq 1 ]; then
        for v in IHWIP_NETWORKSETUP IHWIP_LAUNCHCTL IHWIP_PKGUTIL IHWIP_PGREP IHWIP_PKILL; do
            val=$(env_get "$v") || abort "a run that can change things without $v"
            case "$val" in
                "$FAKES"/* | "$FAKE_NS") ;;
                *) abort "a run that can change things with a non-fake $v=$val" ;;
            esac
        done
    fi
}

# run_rescue <args...>: the usual way tests run the script with a fake root.
run_rescue() {
    run_rescue_in /dev/null "$@"
}

# run_rescue_in <stdin file> <args...>: the same, with stdin from a file.
run_rescue_in() {
    local input="$1"
    shift
    check_run "$input" "$@"
    /usr/bin/env -i "${ENVV[@]}" /bin/bash "$SCRIPT" "$@" >"$T/out.txt" 2>"$T/err.txt" <"$input"
    RC=$?
    collect_output
}

# run_rescue_piped <args...>: stdout and stderr share one pipe, so this only
# returns once every process holding them has closed them (a stage B that kept
# the caller's output open would make it hang). Sets PIPE_SECS.
run_rescue_piped() {
    local t0=$SECONDS
    check_run /dev/null "$@"
    /usr/bin/env -i "${ENVV[@]}" /bin/bash "$SCRIPT" "$@" 2>&1 </dev/null | cat >"$T/out.txt"
    RC=${PIPESTATUS[0]}
    PIPE_SECS=$((SECONDS - t0))
    : >"$T/err.txt"
    collect_output
}

# note_stage_b: picks up the stage B process the last run reported.
note_stage_b() {
    STAGE_B=$(sed -n 's/^# stage B process: \([0-9][0-9]*\)$/\1/p' "$T/out.txt" | head -n 1)
    if [ -n "$STAGE_B" ]; then
        STAGE_B_PIDS="$STAGE_B_PIDS $STAGE_B"
    else
        bad "no '# stage B process:' line"
    fi
}

# wait_for_line <file> <text> <max seconds>
wait_for_line() {
    local k=0
    while [ "$k" -lt $(($3 * 5)) ]; do
        if [ -f "$1" ] && grep -Fq -- "$2" "$1"; then return 0; fi
        sleep 0.2
        k=$((k + 1))
    done
    return 1
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
        tty=no
        if stdin_is_terminal; then tty=yes; fi
        printf "R=[%s] TEST_MODE=%s NS=%s LAUNCHCTL=%s PKGUTIL=%s PGREP=%s PKILL=%s ROOTVAR=%s TTY=%s WAIT=%s\n" \
            "$R" "$TEST_MODE" "$NS" "$LAUNCHCTL" "$PKGUTIL" "$PGREP" "$PKILL" "${IHWIP_ROOT-unset}" "$tty" "$WAIT_TIMEOUT"
    ' probe "$SCRIPT" "$1" "$2" >"$T/out.txt" 2>"$T/err.txt" </dev/null
    RC=$?
    collect_output
}

# run_json: sources the script (main does not run) and parses $T/j.json with
# load_journal only. Prints "STATE=… N=… REASON=…" and writes each parsed
# service name and getinfo text to $T/j.svc.<i> and $T/j.info.<i>.
run_json() {
    rm -f "$T"/j.svc.* "$T"/j.info.*
    # shellcheck disable=SC2016 # expanded by the inner bash
    /usr/bin/env -i "${ENVV[@]}" /bin/bash -c '
        . "$1" || exit 90
        LC_ALL=C
        R="$2"
        TEST_MODE=1
        JOURNAL_FILE="/j.json"
        load_journal
        printf "STATE=%s N=%s REASON=%s\n" "$J_STATE" "${#J_SVC[@]}" "$J_REASON"
        i=0
        while [ "$i" -lt ${#J_SVC[@]} ]; do
            printf "%s" "${J_SVC[i]}" >"$2/j.svc.$i"
            printf "%s" "${J_INFO[i]}" >"$2/j.info.$i"
            i=$((i + 1))
        done
    ' probe "$SCRIPT" "$T" >"$T/out.txt" 2>"$T/err.txt" </dev/null
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

# jsel_count <n> <yes|no>: NETWORK journal records with that selected= value.
jsel_count() {
    local c
    c=$(grep -F "NETWORK${TAB}journal${TAB}" "$T/out.txt" | grep -c " selected=$2\$")
    if [ "$c" -ne "$1" ]; then bad "expected $1 journal record(s) with selected=$2, found $c"; fi
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

# argv_line <networksetup args...>: how the fake logs that exact argv.
argv_line() {
    printf '%q ' networksetup "$@"
}

ns_writes() {
    local log="$T/ns-state/calls.log"
    [ -f "$log" ] || return 0
    grep -Ev '^networksetup -(listallnetworkservices|getinfo)( |$)' "$log"
}

# writes_are <argv line>...: every networksetup write so far, exactly and in order.
writes_are() {
    local a want="" got
    for a in "$@"; do want="$want$a$NL"; done
    got=$(ns_writes)
    if [ -n "$got" ]; then got="$got$NL"; fi
    if [ "$got" != "$want" ]; then
        bad "networksetup writes differ$NL        want: $(printf '%s' "$want" | tr '\n' ';')$NL        got:  $(printf '%s' "$got" | tr '\n' ';')"
    fi
}

line_no() {
    grep -nF -- "$1" "$T/out.txt" | head -n 1 | cut -d: -f1
}

# in_order <text>...: each text first appears in stdout after the previous one.
in_order() {
    local a n prev=0
    for a in "$@"; do
        n=$(line_no "$a")
        if [ -z "$n" ]; then
            bad "order check: stdout lacks: $a"
            return
        fi
        if [ "$n" -le "$prev" ]; then bad "out of order: $a"; fi
        prev=$n
    done
}

# input <line>...: the stdin file for run_rescue_in.
input() {
    printf '%s\n' "$@" >"$T/in.txt"
}

refused() { # <stderr text> <args...>: expects exit 2, keeps the environment
    local want="$1"
    shift
    run_rescue "$@"
    assert_rc 2
    err_has "$want"
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
JOURNAL_REL="Library/Application Support/com.ithurtswhenip.helper/journal.json"
FX=""

# fx_copy: a private copy of the default networksetup fixtures for this test.
fx_copy() {
    FX="$T/fixtures"
    cp -R "$FIXTURES_ROOT/default" "$FX" || abort "cannot copy fixtures"
    env_set "IHWIP_FAKE_FIXTURES=$FX"
}

# fx_name: the fake's info file name for a service.
fx_name() {
    printf '%s' "$1" | sed -E 's/[^A-Za-z0-9-]+/_/g'
}

# fx_info <service> <line>...: replaces its -getinfo text.
fx_info() {
    local svc="$1"
    shift
    printf '%s\n' "$@" >"$FX/info/$(fx_name "$svc").txt"
}

# fx_service <name as listed, a leading * = disabled> <getinfo line>...
fx_service() {
    local listed="$1"
    shift
    printf '%s\n' "$listed" >>"$FX/services.txt"
    fx_info "${listed#\*}" "$@"
}

journal_put() {
    printf '%s\n' "$1" >"$ROOT/$JOURNAL_REL"
}

# The main journal fixture (schema 1, as the Phase 2 helper will write it):
# a DHCP original now Manual, a Manual original (name with a space, comma and
# quotes) now DHCP, one that already matches, a Manual original without a
# router, and a service that no longer exists.
J_MAIN='{"schema":1,"services":[
  {"service":"Dante Primary","recorded_at":"2026-11-01T10:00:00Z","getinfo":"DHCP Configuration\nIP address: 192.168.10.99\nSubnet mask: 255.255.255.0\nRouter: 192.168.10.1\nClient ID: \nIPv6: Automatic\n","dns":"There aren'\''t any DNS Servers set on Dante Primary.","search":"There aren'\''t any Search Domains set on Dante Primary."},
  {"service":"Show \"A\", B","recorded_at":"2026-11-02T10:00:00Z","getinfo":"Manual Configuration\nIP address: 10.0.0.5\nSubnet mask: 255.255.255.0\nRouter: 10.0.0.1\n","dns":"10.0.0.1","search":"show.example"},
  {"service":"Wi-Fi","recorded_at":"2026-11-03T10:00:00Z","getinfo":"DHCP Configuration\nIP address: 10.20.30.40\nSubnet mask: 255.255.252.0\nRouter: 10.20.28.1\n","dns":"","search":""},
  {"service":"Thunderbolt Bridge","recorded_at":"2026-11-04T10:00:00Z","getinfo":"Manual Configuration\nIP address: 169.254.1.1\nSubnet mask: 255.255.0.0\nRouter: \n","dns":"","search":""},
  {"service":"Gone Adapter","recorded_at":"2026-11-05T10:00:00Z","getinfo":"Manual Configuration\nIP address: 172.16.0.9\nSubnet mask: 255.255.0.0\nRouter: 172.16.0.1\n","dns":"","search":""}
]}'

tree_journal() {
    tree_full
    fx_copy
    fx_service 'Show "A", B' "DHCP Configuration" "IP address: 192.168.50.7" "Subnet mask: 255.255.255.0" \
        "Router: 192.168.50.1" "Client ID: " "IPv6: Automatic" "Ethernet Address: 00:11:22:33:44:88"
    journal_put "$J_MAIN"
}

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
    refusal "--network dhcp: needs a service name" --plan --network dhcp:
    refusal "--network leave cannot be combined with other --network choices" --uninstall --yes --network leave --network "dhcp:A,B"
    refusal "--network leave cannot be combined with other --network choices" --uninstall --yes --network restore-journal --network leave
    refusal "--network leave cannot be combined with other --network choices" --plan --network leave --network "dhcp:Wi-Fi"
    refusal "--wait-pid needs a positive whole number" --plan --wait-pid abc
    refusal "--wait-pid needs a positive whole number" --plan --wait-pid 0
    refusal "--wait-pid needs --uninstall" --wait-pid 123
    refusal "--wait-pid needs --uninstall" --yes --network leave --wait-pid 123
    env_set "IHWIP_TEST_STDIN_TTY=yes"
    refusal "IHWIP_TEST_STDIN_TTY must be 1 when set, got: yes" --plan
    env_set "IHWIP_WAIT_TIMEOUT=0"
    refusal "IHWIP_WAIT_TIMEOUT must be 1 to 600" --plan
    env_set "IHWIP_WAIT_TIMEOUT=601"
    refusal "IHWIP_WAIT_TIMEOUT must be 1 to 600" --plan
    env_set "IHWIP_WAIT_TIMEOUT=5s"
    refusal "IHWIP_WAIT_TIMEOUT must be 1 to 600" --plan
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

    begin "9c  test-only IHWIP_TEST_STDIN_TTY / IHWIP_WAIT_TIMEOUT: used in test mode, ignored as root or without IHWIP_ROOT"
    env_set "IHWIP_TEST_STDIN_TTY=1"
    env_set "IHWIP_WAIT_TIMEOUT=5"
    run_probe 501 plan
    assert_rc 0
    out_has "TEST_MODE=1"
    out_has "TTY=yes WAIT=5"
    run_probe 0 plan
    assert_rc 0
    out_has "TEST_MODE=0"
    out_has "TTY=no WAIT=300"
    err_has "running as root: ignoring IHWIP_* test overrides"
    env_unset IHWIP_ROOT
    run_probe 501 plan
    assert_rc 0
    out_has "R=[] TEST_MODE=0"
    out_has "TTY=no WAIT=300"
    err_has "IHWIP_ROOT is not set: ignoring IHWIP_* test overrides"
    reset_env
    env_set "IHWIP_TEST_STDIN_TTY=1"
    env_unset IHWIP_PGREP
    run_probe 501 plan
    assert_rc 2
    err_has "test mode needs IHWIP_PGREP set to a fake tool"
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
    out_has "--network dhcp:\"<service>\""
    out_has "--network restore-journal"
    out_has "DNS servers and search domains"
    out_has "are not restored by this version"
    out_has "--wait-pid <pid>"
    out_has "This script never signals that"
    out_has "numbered menu"
    out_has "--log <path>"
    out_has "--yes"
    out_lacks "not available in this build"
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

# ---------------------------------------------------------------- network revert

test_no_journal_plan() {
    begin "20  no journal: --plan lists enabled Manual services, selects none (golden)"
    tree_full
    rm -f "$ROOT/$JOURNAL_REL"
    fx_copy
    fx_info "USB 10/100/1000 LAN" "Manual Configuration" "IP address: 10.1.2.3" "Subnet mask: 255.255.0.0" \
        "Router: 10.1.0.1" "IPv6: Automatic" "Ethernet Address: 00:11:22:33:44:55"
    fx_info "Old Adapter" "Manual Configuration" "IP address: 10.9.9.9" "Subnet mask: 255.255.255.0" "Router: " \
        "IPv6: Automatic" "Ethernet Address: 00:11:22:33:44:77"
    tree_sum "$ROOT" >"$T/before.sum"
    run_rescue --plan
    assert_rc 0
    err_empty
    golden plan-no-journal
    out_count 2 "NETWORK${TAB}manual${TAB}"
    out_has "NETWORK${TAB}manual${TAB}USB 10/100/1000 LAN${TAB}ip=10.1.2.3 mask=255.255.0.0 router=10.1.0.1 selected=no"
    out_has "NETWORK${TAB}manual${TAB}Dante Primary${TAB}ip=192.168.10.21 mask=255.255.255.0 router=- selected=no"
    out_lacks "selected=yes"
    out_lacks "Old Adapter"
    out_lacks "${TAB}journal${TAB}"
    out_lacks "NETWORK${TAB}set"
    out_has "NETWORK${TAB}choice${TAB}-${TAB}leave"
    tree_sum "$ROOT" >"$T/after.sum"
    same_file "$T/before.sum" "$T/after.sum"
    no_mutating_calls
    ns_read_only
    end
}

test_dhcp_pick() {
    local j="/Library/Application Support/com.ithurtswhenip.helper"
    begin "21  --network dhcp:\"Dante Primary\": exactly one -setdhcp, verified, nothing else written, in run order"
    tree_full
    run_rescue --uninstall --yes --network "dhcp:Dante Primary"
    assert_rc 0
    err_empty
    writes_are "$(argv_line -setdhcp "Dante Primary")"
    out_has "NETWORK${TAB}manual${TAB}Dante Primary${TAB}ip=192.168.10.21 mask=255.255.255.0 router=- selected=yes"
    out_has "NETWORK${TAB}setdhcp${TAB}Dante Primary${TAB}source=pick"
    out_has "NETWORK${TAB}choice${TAB}-${TAB}dhcp"
    out_has "OK${TAB}NETWORK${TAB}setdhcp${TAB}Dante Primary${TAB}source=pick"
    out_has "VERIFY${TAB}ok${TAB}network:Dante Primary${TAB}now=dhcp/169.254.99.99/255.255.0.0/- (no DHCP address yet: check the cable or the DHCP server)"
    out_lacks "FAILED"
    out_has "# failed steps: 0"
    inventory_gone
    file_has "$ROOT/$BK_REL/network-before.txt" "IP address: 192.168.10.21"
    file_has "$ROOT/$BK_REL/plan.txt" "NETWORK${TAB}setdhcp${TAB}Dante Primary${TAB}source=pick"
    in_order "OK${TAB}BACKUP${TAB}snapshot${TAB}network" \
        "OK${TAB}BACKUP${TAB}dir${TAB}$j" \
        "OK${TAB}NETWORK${TAB}setdhcp${TAB}Dante Primary" \
        "VERIFY${TAB}ok${TAB}network:Dante Primary" \
        "OK${TAB}STOP${TAB}launchd${TAB}com.ithurtswhenip.helper" \
        "OK${TAB}REMOVE${TAB}dir${TAB}$j" \
        "OK${TAB}FORGET${TAB}receipt${TAB}com.ithurtswhenip.pkg" \
        "VERIFY${TAB}ok${TAB}$j" \
        "# leftovers (reported only, never removed)"
    end
}

test_bad_picks() {
    begin "22  dhcp pick for an unknown or disabled service: exit 2, zero writes, tree unchanged"
    tree_full
    tree_sum "$ROOT" >"$T/before.sum"
    refused "--network dhcp:Nope: there is no network service named exactly \"Nope\"" --plan --network dhcp:Nope
    refused "--network dhcp:Nope: there is no network service named exactly \"Nope\"" --uninstall --yes --network dhcp:Nope
    refused "--network dhcp:Old Adapter: that service is disabled" --uninstall --yes --network "dhcp:Old Adapter"
    refused "named exactly \"*Old Adapter\"" --uninstall --yes --network "dhcp:*Old Adapter"
    refused "named exactly \"dante primary\"" --uninstall --yes --network "dhcp:dante primary"
    refused "named exactly \"Dante Primary \"" --uninstall --yes --network "dhcp:Dante Primary "
    refused "named exactly \"Dante*\"" --uninstall --yes --network "dhcp:Dante*"
    refused "named exactly \"Nope\"" --uninstall --yes --network "dhcp:Wi-Fi" --network "dhcp:Nope"
    err_has "Nothing has been changed."
    tree_sum "$ROOT" >"$T/after.sum"
    same_file "$T/before.sum" "$T/after.sum"
    ns_no_writes
    no_mutating_calls
    absent "$ROOT/Users/Shared/ItHurtsWhenIP-backups"
    end
}

test_journal_plan() {
    begin "23  journal: --plan shows original vs current for exactly the journaled services (goldens)"
    tree_journal
    tree_sum "$ROOT" >"$T/before.sum"
    run_rescue --plan
    assert_rc 0
    err_empty
    golden plan-journal
    out_count 5 "NETWORK${TAB}journal${TAB}"
    jsel_count 5 no
    out_has "NETWORK${TAB}journal${TAB}Show \"A\", B${TAB}original=manual/10.0.0.5/255.255.255.0/10.0.0.1 current=dhcp/192.168.50.7/255.255.255.0/192.168.50.1 selected=no"
    out_has "NETWORK${TAB}journal${TAB}Gone Adapter${TAB}original=manual/172.16.0.9/255.255.0.0/172.16.0.1 current=missing/-/-/- selected=no"
    out_lacks "NETWORK${TAB}set"
    out_has "NETWORK${TAB}choice${TAB}-${TAB}leave"
    run_rescue --plan --network restore-journal
    assert_rc 0
    err_empty
    golden plan-journal-restore
    out_count 5 "NETWORK${TAB}journal${TAB}"
    jsel_count 5 yes
    out_has "NOTE${TAB}info${TAB}journal${TAB}found: 5 service(s) recorded; restore covers IPv4 settings only (DNS servers and search domains are not restored by this version)"
    out_has "NETWORK${TAB}setdhcp${TAB}Dante Primary${TAB}source=journal"
    out_has "NETWORK${TAB}setmanual${TAB}Show \"A\", B${TAB}ip=10.0.0.5 mask=255.255.255.0 router=10.0.0.1 source=journal"
    out_has "NETWORK${TAB}unchanged${TAB}Wi-Fi${TAB}already matches the recorded original"
    out_has "NETWORK${TAB}setmanual${TAB}Thunderbolt Bridge${TAB}ip=169.254.1.1 mask=255.255.0.0 router=- source=journal"
    out_has "NETWORK${TAB}skip${TAB}Gone Adapter${TAB}service no longer exists: not restored"
    out_has "NETWORK${TAB}choice${TAB}-${TAB}restore-journal"
    out_lacks "USB 10/100/1000 LAN"
    tree_sum "$ROOT" >"$T/after.sum"
    same_file "$T/before.sum" "$T/after.sum"
    no_mutating_calls
    ns_read_only
    end
}

# A journal byte-for-byte as the helper writes it. The helper's unit test
# recorded_journal_matches_the_shared_fixture pins the same file, so this
# checks the helper's writer against this script's reader.
HELPER_JOURNAL="$HERE/fixtures/journal-from-helper.json"

test_helper_journal_plan() {
    local cafe i want
    cafe="Caf"$'\303\251'" Ethernet"
    begin "23b journal written by the helper: --plan shows exactly its 5 services; the parser reads them byte-exact"
    [ -f "$HELPER_JOURNAL" ] || abort "missing $HELPER_JOURNAL"
    tree_full
    fx_copy
    fx_service 'Show "A", B' "DHCP Configuration" "IP address: 192.168.50.7" "Subnet mask: 255.255.255.0" \
        "Router: 192.168.50.1" "Client ID: " "IPv6: Automatic" "Ethernet Address: 00:11:22:33:44:88"
    cp "$HELPER_JOURNAL" "$ROOT/$JOURNAL_REL" || abort "cannot install $HELPER_JOURNAL"
    tree_sum "$ROOT" >"$T/before.sum"
    run_rescue --plan
    assert_rc 0
    err_empty
    out_has "NOTE${TAB}info${TAB}journal${TAB}found: 5 service(s) recorded; restore covers IPv4 settings only (DNS servers and search domains are not restored by this version)"
    out_lacks "not used ("
    out_count 5 "NETWORK${TAB}journal${TAB}"
    jsel_count 5 no
    out_has "NETWORK${TAB}journal${TAB}Dante Primary${TAB}original=manual/192.168.10.21/255.255.255.0/- current=manual/192.168.10.21/255.255.255.0/- selected=no"
    out_has "NETWORK${TAB}journal${TAB}Show \"A\", B${TAB}original=dhcp/192.168.50.7/255.255.255.0/192.168.50.1 current=dhcp/192.168.50.7/255.255.255.0/192.168.50.1 selected=no"
    out_has "NETWORK${TAB}journal${TAB}$cafe${TAB}original=manual/10.50.0.5/255.255.0.0/10.50.0.1 current=missing/-/-/- selected=no"
    out_has "NETWORK${TAB}journal${TAB}Wi-Fi${TAB}original=dhcp/10.20.30.40/255.255.252.0/10.20.28.1 current=dhcp/10.20.30.40/255.255.252.0/10.20.28.1 selected=no"
    out_has "NETWORK${TAB}journal${TAB}USB 10/100/1000 LAN${TAB}original=dhcp/192.168.1.42/255.255.255.0/192.168.1.1 current=dhcp/192.168.1.42/255.255.255.0/192.168.1.1 selected=no"
    tree_sum "$ROOT" >"$T/after.sum"
    same_file "$T/before.sum" "$T/after.sum"
    no_mutating_calls
    ns_read_only
    # The parser on the same file: J_STATE ok, names and a multi-line getinfo exact.
    cp "$HELPER_JOURNAL" "$T/j.json" || abort "cannot copy $HELPER_JOURNAL"
    jcase helper-journal "STATE=ok N=5 REASON="
    i=0
    for want in "Dante Primary" 'Show "A", B' "$cafe" "Wi-Fi" "USB 10/100/1000 LAN"; do
        printf '%s' "$want" >"$T/want.svc"
        same_file "$T/j.svc.$i" "$T/want.svc"
        i=$((i + 1))
    done
    printf 'Manual Configuration\nIP address: 192.168.10.21\nSubnet mask: 255.255.255.0\nRouter: \nIPv6: Automatic\nIPv6 IP address: none\nIPv6 Router: none\nEthernet Address: 00:11:22:33:44:66\n' >"$T/want.info"
    same_file "$T/j.info.0" "$T/want.info"
    end
}

test_journal_restore() {
    begin "24  --network restore-journal: exact argv per service, quoted name intact, missing service skipped (exit 1)"
    tree_journal
    cp "$ROOT/$JOURNAL_REL" "$T/journal.orig"
    run_rescue --uninstall --yes --network restore-journal
    assert_rc 1
    err_empty
    writes_are "$(argv_line -setdhcp "Dante Primary")" \
        "$(argv_line -setmanual 'Show "A", B' 10.0.0.5 255.255.255.0 10.0.0.1)" \
        "$(argv_line -setmanual "Thunderbolt Bridge" 169.254.1.1 255.255.0.0 "")"
    out_has "OK${TAB}NETWORK${TAB}setdhcp${TAB}Dante Primary${TAB}source=journal"
    out_has "OK${TAB}NETWORK${TAB}setmanual${TAB}Show \"A\", B${TAB}ip=10.0.0.5 mask=255.255.255.0 router=10.0.0.1 source=journal"
    out_has "VERIFY${TAB}ok${TAB}network:Show \"A\", B${TAB}now=manual/10.0.0.5/255.255.255.0/10.0.0.1"
    out_has "VERIFY${TAB}ok${TAB}network:Thunderbolt Bridge${TAB}now=manual/169.254.1.1/255.255.0.0/-"
    out_has "VERIFY${TAB}ok${TAB}network:Dante Primary${TAB}now=dhcp/"
    out_has "OK${TAB}NETWORK${TAB}unchanged${TAB}Wi-Fi${TAB}already matches the recorded original"
    out_lacks "network:Wi-Fi"
    out_has "FAILED${TAB}NETWORK${TAB}skip${TAB}Gone Adapter${TAB}-${TAB}service no longer exists: not restored"
    out_has "# failed steps: 1; still present after verify: 0"
    out_has "NOTE${TAB}info${TAB}journal${TAB}found: 5 service(s) recorded; restore covers IPv4 settings only (DNS servers and search domains are not restored by this version)"
    file_has "$T/ns-state/info/$(fx_name 'Show "A", B').txt" "IP address: 10.0.0.5"
    same_file "$ROOT/$BK_REL/system/$JOURNAL_REL" "$T/journal.orig"
    inventory_gone
    end
}

test_journal_overlap() {
    begin "25  restore-journal plus a dhcp pick for a journaled service: the pick wins, noted in the plan"
    tree_journal
    run_rescue --plan --network restore-journal --network "dhcp:Thunderbolt Bridge"
    assert_rc 0
    out_has "NOTE${TAB}info${TAB}Thunderbolt Bridge${TAB}picked for DHCP and also in the journal: the DHCP pick wins"
    out_has "NETWORK${TAB}setdhcp${TAB}Thunderbolt Bridge${TAB}source=pick"
    out_lacks "NETWORK${TAB}setmanual${TAB}Thunderbolt Bridge"
    out_has "NETWORK${TAB}journal${TAB}Thunderbolt Bridge${TAB}original=manual/169.254.1.1/255.255.0.0/- current=dhcp/169.254.12.34/255.255.0.0/- selected=no"
    out_has "NETWORK${TAB}journal${TAB}Dante Primary${TAB}original=dhcp/192.168.10.99/255.255.255.0/192.168.10.1 current=manual/192.168.10.21/255.255.255.0/- selected=yes"
    out_has "NETWORK${TAB}choice${TAB}-${TAB}restore-journal+dhcp"
    in_order "NETWORK${TAB}setdhcp${TAB}Thunderbolt Bridge" "NETWORK${TAB}setdhcp${TAB}Dante Primary" "NETWORK${TAB}choice"
    ns_no_writes
    run_rescue --uninstall --yes --network restore-journal --network "dhcp:Thunderbolt Bridge"
    assert_rc 1
    writes_are "$(argv_line -setdhcp "Thunderbolt Bridge")" \
        "$(argv_line -setdhcp "Dante Primary")" \
        "$(argv_line -setmanual 'Show "A", B' 10.0.0.5 255.255.255.0 10.0.0.1)"
    out_has "VERIFY${TAB}ok${TAB}network:Thunderbolt Bridge${TAB}now=dhcp/"
    end
}

test_journal_fallback() {
    local kind json reason
    for kind in unknown-schema corrupt; do
        case "$kind" in
            unknown-schema)
                json='{"schema":2,"services":[{"service":"Dante Primary","getinfo":"DHCP Configuration"}]}'
                reason="unknown schema 2"
                ;;
            corrupt)
                json='{"schema":1,"services":[{"service":"Dante Primary","getinfo":"DHCP Conf'
                reason="not valid JSON: control character inside a string" # truncated mid-string; journal_put adds a line break
                ;;
        esac
        begin "26  $kind journal: NOTE, Manual services listed (none selected), restore-journal exit 2, file backed up"
        tree_full
        journal_put "$json"
        cp "$ROOT/$JOURNAL_REL" "$T/journal.orig"
        tree_sum "$ROOT" >"$T/before.sum"
        run_rescue --plan
        assert_rc 0
        err_empty
        out_has "NOTE${TAB}info${TAB}journal${TAB}not used ($reason); it is backed up with the helper folder, and services on Manual settings are listed instead"
        out_has "NETWORK${TAB}manual${TAB}Dante Primary${TAB}ip=192.168.10.21 mask=255.255.255.0 router=- selected=no"
        out_lacks "${TAB}journal${TAB}Dante Primary"
        out_has "NETWORK${TAB}choice${TAB}-${TAB}leave"
        refused "--network restore-journal: the journal could not be used ($reason). Nothing has been changed." \
            --uninstall --yes --network restore-journal
        refused "--network restore-journal: the journal could not be used ($reason)" \
            --plan --network restore-journal --network "dhcp:Dante Primary"
        tree_sum "$ROOT" >"$T/after.sum"
        same_file "$T/before.sum" "$T/after.sum"
        ns_no_writes
        no_mutating_calls
        run_rescue --uninstall --yes
        assert_rc 0
        out_has "OK${TAB}NETWORK${TAB}choice${TAB}-${TAB}leave: no network settings were changed"
        same_file "$ROOT/$BK_REL/system/$JOURNAL_REL" "$T/journal.orig"
        ns_read_only
        inventory_gone
        end
    done

    begin "27  restore-journal with no journal: exit 2 before anything changes"
    tree_full
    rm -f "$ROOT/$JOURNAL_REL"
    tree_sum "$ROOT" >"$T/before.sum"
    refused "--network restore-journal: no journal was found" --uninstall --yes --network restore-journal
    refused "--network restore-journal: no journal was found" --plan --network restore-journal
    tree_sum "$ROOT" >"$T/after.sum"
    same_file "$T/before.sum" "$T/after.sum"
    ns_no_writes
    no_mutating_calls
    end
}

test_journal_injection() {
    local evil bad
    evil="'; rm -rf / \$(touch $T/PWNED1) \`touch $T/PWNED2\`"
    begin "28  injection-style journal values: literal name intact, bad addresses skipped with zero writes"
    tree_full
    fx_copy
    fx_service "$evil" "Manual Configuration" "IP address: 10.7.7.7" "Subnet mask: 255.255.255.0" "Router: " "IPv6: Automatic"
    for bad in "Bad IP 1" "Bad IP 2" "Bad Mask" "Bad Router" "Leading Zero"; do
        fx_service "$bad" "DHCP Configuration" "IP address: 192.168.77.7" "Subnet mask: 255.255.255.0" "Router: 192.168.77.1"
    done
    journal_put "{\"schema\":1,\"services\":[
      {\"service\":\"$evil\",\"getinfo\":\"DHCP Configuration\\n\"},
      {\"service\":\"Bad IP 1\",\"getinfo\":\"Manual Configuration\\nIP address: \$(touch $T/PWNED3)\\nSubnet mask: 255.255.255.0\\nRouter: \\n\"},
      {\"service\":\"Bad IP 2\",\"getinfo\":\"Manual Configuration\\nIP address: 10.0.0.5; rm -rf /\\nSubnet mask: 255.255.255.0\\nRouter: \\n\"},
      {\"service\":\"Bad Mask\",\"getinfo\":\"Manual Configuration\\nIP address: 10.0.0.6\\nSubnet mask: 255.0.255.0\\nRouter: \\n\"},
      {\"service\":\"Bad Router\",\"getinfo\":\"Manual Configuration\\nIP address: 10.0.0.7\\nSubnet mask: 255.255.255.0\\nRouter: 10.0.0.1 -x\\n\"},
      {\"service\":\"Leading Zero\",\"getinfo\":\"Manual Configuration\\nIP address: 010.0.0.8\\nSubnet mask: 255.255.255.0\\nRouter: \\n\"}
    ]}"
    run_rescue --plan --network restore-journal
    assert_rc 0
    out_has "NETWORK${TAB}setdhcp${TAB}$evil${TAB}source=journal"
    out_has "NETWORK${TAB}journal${TAB}Bad IP 1${TAB}original=manual/invalid/255.255.255.0/- current=dhcp/192.168.77.7/255.255.255.0/192.168.77.1 selected=yes"
    out_has "NETWORK${TAB}journal${TAB}Bad Router${TAB}original=manual/10.0.0.7/255.255.255.0/invalid"
    out_count 5 "${TAB}recorded address, mask or router is not valid IPv4: not restored"
    run_rescue --uninstall --yes --network restore-journal
    assert_rc 1
    writes_are "$(argv_line -setdhcp "$evil")"
    out_has "VERIFY${TAB}ok${TAB}network:$evil${TAB}now=dhcp/"
    out_count 10 "FAILED${TAB}NETWORK${TAB}skip${TAB}" # 5 steps, repeated in the final failure list
    run_rescue --plan --network "dhcp:$evil"
    assert_rc 0
    out_has "NETWORK${TAB}setdhcp${TAB}$evil${TAB}source=pick"
    absent "$T/PWNED1" "$T/PWNED2" "$T/PWNED3"
    end

    begin "28b journal with a line break in a service name: not used at all, restore-journal exit 2"
    tree_full
    journal_put '{"schema":1,"services":[{"service":"Dante Primary\nrm -rf /","getinfo":"DHCP Configuration"}]}'
    tree_sum "$ROOT" >"$T/before.sum"
    run_rescue --plan
    assert_rc 0
    out_has "NOTE${TAB}info${TAB}journal${TAB}not used (schema 1 but a service name contains a control character)"
    refused "the journal could not be used (schema 1 but a service name contains a control character)" \
        --uninstall --yes --network restore-journal
    tree_sum "$ROOT" >"$T/after.sum"
    same_file "$T/before.sum" "$T/after.sum"
    ns_no_writes
    end
}

test_plan_network_values() {
    begin "29  --plan with every --network value changes nothing (checksum, zero writes)"
    tree_journal
    tree_sum "$ROOT" >"$T/before.sum"
    run_rescue --plan --network leave
    assert_rc 0
    out_has "NETWORK${TAB}choice${TAB}-${TAB}leave"
    run_rescue --plan --network "dhcp:Dante Primary"
    assert_rc 0
    out_has "NETWORK${TAB}setdhcp${TAB}Dante Primary${TAB}source=pick"
    run_rescue --plan --network restore-journal
    assert_rc 0
    out_has "NETWORK${TAB}choice${TAB}-${TAB}restore-journal"
    run_rescue --plan --network restore-journal --network "dhcp:Wi-Fi" --network "dhcp:Wi-Fi"
    assert_rc 0
    out_count 1 "NETWORK${TAB}setdhcp${TAB}Wi-Fi${TAB}source=pick"
    out_has "NOTE${TAB}info${TAB}Wi-Fi${TAB}picked for DHCP and also in the journal: the DHCP pick wins"
    refused "named exactly \"Nope\"" --plan --network "dhcp:Nope"
    refused "--network leave cannot be combined" --plan --network leave --network restore-journal
    run_rescue --network "dhcp:Dante Primary"
    assert_rc 2
    out_has "NETWORK${TAB}setdhcp${TAB}Dante Primary${TAB}source=pick"
    err_has "no action given"
    tree_sum "$ROOT" >"$T/after.sum"
    same_file "$T/before.sum" "$T/after.sum"
    ns_no_writes
    no_mutating_calls
    absent "$ROOT/Users/Shared/ItHurtsWhenIP-backups"
    end
}

test_network_edges() {
    begin "29b network change with nothing installed: backup folder and snapshot first, then the write"
    tree_empty
    run_rescue --uninstall --yes --network "dhcp:Dante Primary"
    assert_rc 0
    err_empty
    out_has "NOTE${TAB}info${TAB}-${TAB}nothing to remove"
    out_has "# backup folder: /$BK_REL"
    writes_are "$(argv_line -setdhcp "Dante Primary")"
    present "$ROOT/$BK_REL/network-before.txt" "$ROOT/$BK_REL/plan.txt" "$ROOT/$BK_REL/rescue.log"
    file_has "$ROOT/$BK_REL/network-before.txt" "IP address: 192.168.10.21"
    file_has "$ROOT/$BK_REL/rescue.log" "VERIFY${TAB}ok${TAB}network:Dante Primary"
    no_mutating_calls_except_ns
    end

    begin "29c a networksetup write fails: FAILED plus VERIFY FAILED, the uninstall continues, exit 1"
    tree_full
    env_set "IHWIP_FAKE_FAIL=setdhcp"
    run_rescue --uninstall --yes --network "dhcp:Dante Primary"
    assert_rc 1
    writes_are "$(argv_line -setdhcp "Dante Primary")"
    out_has "FAILED${TAB}NETWORK${TAB}setdhcp${TAB}Dante Primary${TAB}source=pick${TAB}networksetup exit 1: ** Error: The parameters were not valid."
    out_has "VERIFY${TAB}FAILED${TAB}network:Dante Primary${TAB}now=manual/192.168.10.21/255.255.255.0/-, expected dhcp"
    out_has "# failed steps: 1; still present after verify: 1"
    inventory_gone
    end
}

# no_mutating_calls_except_ns: no launchctl/pkgutil/pkill changes (networksetup writes are checked separately).
no_mutating_calls_except_ns() {
    if [ -f "$T/calls.log" ] && grep -Eq '^(launchctl (bootout|unload)|pkgutil --forget|pkill) ' "$T/calls.log"; then
        bad "mutating fake call(s): $(grep -E '^(launchctl (bootout|unload)|pkgutil --forget|pkill) ' "$T/calls.log" | tr '\n' ';')"
    fi
}

# ---------------------------------------------------------------- --wait-pid

test_wait_pid() {
    local bk
    begin "30  --wait-pid: stage A now, app and user data stay until the process exits; stage B survives HUP and the caller"
    tree_full
    state_add procs.running it-hurts-when-ip
    start_sleeper
    env_set "IHWIP_WAIT_TIMEOUT=60"
    bk="$ROOT/$BK_REL"
    run_rescue_piped --uninstall --yes --wait-pid "$SLEEPER"
    note_stage_b
    assert_rc 0
    if [ "$PIPE_SECS" -ge 10 ]; then bad "the caller's output pipe stayed open ${PIPE_SECS} s (stage B kept it)"; fi
    out_has "NOTE${TAB}info${TAB}wait-pid${TAB}two stages: stage B waits for process $SLEEPER to exit (up to 60 s), then removes the app bundle and per-user data"
    out_has "# result: stage A"
    out_has "# failed steps: 0"
    out_has "# stage B: waits for process $SLEEPER to exit (up to 60 s), then removes the app bundle and per-user data; its result is appended to /$BK_REL/rescue.log"
    out_lacks "OK${TAB}REMOVE${TAB}app"
    out_lacks "OK${TAB}QUIT"
    out_lacks "# leftovers"
    calls_lack "pkill"
    absent "$ROOT/Library/LaunchDaemons/com.ithurtswhenip.helper.plist" \
        "$ROOT/Library/PrivilegedHelperTools/com.ithurtswhenip.helper" \
        "$ROOT/var/run/com.ithurtswhenip.helper.sock" \
        "$ROOT/Library/Application Support/com.ithurtswhenip.helper"
    file_lacks "$ROOT/.fake/launchctl.loaded" "com.ithurtswhenip.helper"
    file_lacks "$ROOT/.fake/pkgutil.receipts" "com.ithurtswhenip.pkg"
    present "$ROOT/Applications/It Hurts When IP.app/Contents/MacOS/it-hurts-when-ip" \
        "$ROOT/Users/alice/Library/Application Support/com.ithurtswhenip.desktop/config.json" \
        "$ROOT/Users/Mary Jane/Library/Preferences/com.ithurtswhenip.desktop.plist" \
        "$bk/users/alice/Library/Application Support/com.ithurtswhenip.desktop/config.json"
    if [ -n "$STAGE_B" ]; then
        kill -HUP "$STAGE_B" 2>/dev/null
        sleep 1
        stage_b_alive "$STAGE_B" || bad "stage B did not survive SIGHUP"
    fi
    sleep 1
    present "$ROOT/Applications/It Hurts When IP.app/Contents/MacOS/it-hurts-when-ip" \
        "$ROOT/Users/alice/Library/Application Support/com.ithurtswhenip.desktop/config.json"
    file_lacks "$bk/rescue.log" "# stage B exit:"
    calls_lack "pkill"
    stop_sleeper "$SLEEPER"
    wait_for_line "$bk/rescue.log" "# stage B exit:" 20 || bad "stage B did not finish within 20 s of the process exiting"
    file_has "$bk/rescue.log" "# stage B: process $SLEEPER has exited"
    file_has "$bk/rescue.log" "OK${TAB}REMOVE${TAB}app${TAB}/Applications/It Hurts When IP.app${TAB}-"
    file_has "$bk/rescue.log" "# result: stage B"
    file_has "$bk/rescue.log" "VERIFY${TAB}ok${TAB}/Applications/It Hurts When IP.app"
    file_has "$bk/rescue.log" "VERIFY${TAB}ok${TAB}/Users/alice/Library/Application Support/com.ithurtswhenip.desktop"
    file_has "$bk/rescue.log" "VERIFY${TAB}ok${TAB}/Library/LaunchDaemons/com.ithurtswhenip.helper.plist"
    file_has "$bk/rescue.log" "VERIFY${TAB}ok${TAB}launchd:com.ithurtswhenip.helper"
    file_has "$bk/rescue.log" "# leftovers (reported only, never removed): 1"
    file_has "$bk/rescue.log" "# stage B exit: 0"
    if [ "$(grep -n '^# result: stage A$' "$bk/rescue.log" | cut -d: -f1)" -gt "$(grep -n '^# stage B: started' "$bk/rescue.log" | cut -d: -f1)" ]; then
        bad "stage B wrote to the log before stage A had finished"
    fi
    calls_have "pkill -x it-hurts-when-ip"
    inventory_gone
    end
}

test_wait_pid_timeout() {
    local bk
    begin "31  --wait-pid timeout: stage B gives up, app bundle and user data kept and reported, exit 1 in the log"
    tree_full
    start_sleeper
    env_set "IHWIP_WAIT_TIMEOUT=1"
    bk="$ROOT/$BK_REL"
    run_rescue --uninstall --yes --wait-pid "$SLEEPER"
    note_stage_b
    assert_rc 0
    out_has "# stage B: waits for process $SLEEPER to exit (up to 1 s)"
    wait_for_line "$bk/rescue.log" "# stage B exit:" 20 || bad "stage B did not finish within 20 s"
    file_has "$bk/rescue.log" "# stage B: process $SLEEPER is still running after 1 s: the app bundle and per-user data are left in place"
    file_has "$bk/rescue.log" "FAILED${TAB}REMOVE${TAB}app${TAB}/Applications/It Hurts When IP.app${TAB}-${TAB}not removed: process $SLEEPER still running after 1 s"
    file_has "$bk/rescue.log" "FAILED${TAB}REMOVE${TAB}dir${TAB}/Users/alice/Library/Application Support/com.ithurtswhenip.desktop${TAB}-${TAB}not removed: process $SLEEPER still running after 1 s"
    file_has "$bk/rescue.log" "VERIFY${TAB}FAILED${TAB}/Applications/It Hurts When IP.app"
    file_has "$bk/rescue.log" "# stage B exit: 1"
    present "$ROOT/Applications/It Hurts When IP.app/Contents/MacOS/it-hurts-when-ip" \
        "$ROOT/Users/alice/Library/Application Support/com.ithurtswhenip.desktop/config.json"
    absent "$ROOT/Library/LaunchDaemons/com.ithurtswhenip.helper.plist"
    sleeper_alive "$SLEEPER" || bad "the waited process was disturbed"
    stop_sleeper "$SLEEPER"
    end
}

# gone_pid: prints a pid that is not running right now.
gone_pid() {
    local p
    sleep 0 &
    p=$!
    wait "$p" 2>/dev/null
    printf '%s' "$p"
}

test_wait_pid_gone() {
    local p j="Library/Application Support/com.ithurtswhenip.helper"
    begin "32  --wait-pid for a process that is not running: said so, stage B runs straight away; --log gets stage B"
    tree_full
    p=$(gone_pid)
    if ps -p "$p" >/dev/null 2>&1; then abort "pid $p is unexpectedly running"; fi
    run_rescue --plan --wait-pid "$p"
    assert_rc 0
    out_has "NOTE${TAB}info${TAB}wait-pid${TAB}process $p is not running, so stage B removes the app bundle and per-user data straight after stage A"
    mkdir "$T/logs"
    run_rescue --uninstall --yes --wait-pid "$p" --log "$T/logs/app.log"
    note_stage_b
    assert_rc 0
    out_has "# stage B: process $p is not running (treated as already exited), so stage B starts now and removes the app bundle and per-user data; its result is appended to $T/logs/app.log"
    wait_for_line "$T/logs/app.log" "# stage B exit:" 20 || bad "stage B did not finish within 20 s"
    file_has "$T/logs/app.log" "# stage B: process $p was not running when stage A started: continuing now"
    file_has "$T/logs/app.log" "# stage B exit: 0"
    absent "$ROOT/$BK_REL/rescue.log"
    inventory_gone
    end

    begin "32b stage A fails: exit 1 for stage A, stage B still removes the bundle and reports its own result"
    tree_full
    chmod 555 "$ROOT/$j"
    p=$(gone_pid)
    run_rescue --uninstall --yes --wait-pid "$p"
    note_stage_b
    assert_rc 1
    out_has "FAILED${TAB}REMOVE${TAB}dir${TAB}/$j${TAB}-${TAB}could not remove"
    out_has "# result: stage A"
    wait_for_line "$ROOT/$BK_REL/rescue.log" "# stage B exit:" 20 || bad "stage B did not finish within 20 s"
    absent "$ROOT/Applications/It Hurts When IP.app" "$ROOT/Users/alice/Library/Application Support/com.ithurtswhenip.desktop"
    file_has "$ROOT/$BK_REL/rescue.log" "OK${TAB}REMOVE${TAB}app${TAB}/Applications/It Hurts When IP.app${TAB}-"
    file_has "$ROOT/$BK_REL/rescue.log" "VERIFY${TAB}FAILED${TAB}/$j"
    file_has "$ROOT/$BK_REL/rescue.log" "# stage B exit: 1"
    chmod 755 "$ROOT/$j"
    end
}

# ---------------------------------------------------------------- menu and prompt

test_menu() {
    begin "33a menu (piped, test mode): show the plan, then quit: exit 2, nothing changed"
    tree_full
    env_set "IHWIP_TEST_STDIN_TTY=1"
    tree_sum "$ROOT" >"$T/before.sum"
    input 1 4
    run_rescue_in "$T/in.txt"
    assert_rc 2
    out_has "1) Show the plan (changes nothing)"
    out_has "# mode: plan"
    out_has "NETWORK${TAB}manual${TAB}Dante Primary${TAB}ip=192.168.10.21 mask=255.255.255.0 router=- selected=no"
    out_count 2 "Choose 1-4:"
    err_has "cancelled: nothing has been changed"
    tree_sum "$ROOT" >"$T/after.sum"
    same_file "$T/before.sum" "$T/after.sum"
    no_mutating_calls
    end

    begin "33b menu: uninstall with one service picked for DHCP (same plan, UNINSTALL, run)"
    tree_full
    env_set "IHWIP_TEST_STDIN_TTY=1"
    input 2 9 x 1 d UNINSTALL
    run_rescue_in "$T/in.txt"
    assert_rc 0
    out_has "[ ] 1) Dante Primary   192.168.10.21 / 255.255.255.0"
    out_has "[x] 1) Dante Primary   192.168.10.21 / 255.255.255.0"
    out_has "Not a choice: 9"
    out_has "Not a choice: x"
    out_has "# mode: uninstall"
    out_has "NETWORK${TAB}manual${TAB}Dante Primary${TAB}ip=192.168.10.21 mask=255.255.255.0 router=- selected=yes"
    out_has "Type UNINSTALL to continue:"
    out_has "OK${TAB}NETWORK${TAB}setdhcp${TAB}Dante Primary${TAB}source=pick"
    writes_are "$(argv_line -setdhcp "Dante Primary")"
    file_has "$ROOT/$BK_REL/rescue.log" "# confirmed by typing UNINSTALL"
    inventory_gone
    end

    begin "33c menu: quit, and end of input, change nothing (exit 2)"
    tree_full
    env_set "IHWIP_TEST_STDIN_TTY=1"
    tree_sum "$ROOT" >"$T/before.sum"
    input 4
    run_rescue_in "$T/in.txt"
    assert_rc 2
    err_has "cancelled: nothing has been changed"
    : >"$T/in.txt"
    run_rescue_in "$T/in.txt"
    assert_rc 2
    err_has "cancelled: nothing has been changed"
    input 2 1 q
    run_rescue_in "$T/in.txt"
    assert_rc 2
    input 2 1 d no
    run_rescue_in "$T/in.txt"
    assert_rc 2
    out_has "Type UNINSTALL to continue:"
    err_has "cancelled: nothing has been changed"
    tree_sum "$ROOT" >"$T/after.sum"
    same_file "$T/before.sum" "$T/after.sum"
    no_mutating_calls
    end

    begin "33d menu: journal restore ticked, same restore writes as the flag"
    tree_journal
    env_set "IHWIP_TEST_STDIN_TTY=1"
    input 2 r d UNINSTALL
    run_rescue_in "$T/in.txt"
    assert_rc 1
    out_has "[x] r) Restore the settings the app recorded before it first changed them (5 service(s); IPv4 only, not DNS)"
    out_has "NETWORK${TAB}choice${TAB}-${TAB}restore-journal"
    writes_are "$(argv_line -setdhcp "Dante Primary")" \
        "$(argv_line -setmanual 'Show "A", B' 10.0.0.5 255.255.255.0 10.0.0.1)" \
        "$(argv_line -setmanual "Thunderbolt Bridge" 169.254.1.1 255.255.0.0 "")"
    inventory_gone
    end

    begin "33e menu option 3: uninstall, network left exactly as it is"
    tree_full
    env_set "IHWIP_TEST_STDIN_TTY=1"
    input 3 UNINSTALL
    run_rescue_in "$T/in.txt"
    assert_rc 0
    out_has "OK${TAB}NETWORK${TAB}choice${TAB}-${TAB}leave: no network settings were changed"
    ns_read_only
    inventory_gone
    end

    begin "33f no menu without the test hook: piped input is ignored, plan only, exit 2"
    tree_full
    tree_sum "$ROOT" >"$T/before.sum"
    input 2 1 d UNINSTALL
    run_rescue_in "$T/in.txt"
    assert_rc 2
    out_lacks "Choose 1-4"
    out_has "# mode: plan"
    err_has "no action given"
    run_rescue_in "$T/in.txt" --uninstall
    assert_rc 2
    err_has "--uninstall needs confirmation"
    tree_sum "$ROOT" >"$T/after.sum"
    same_file "$T/before.sum" "$T/after.sum"
    no_mutating_calls
    end
}

test_confirm_prompt() {
    begin "34  Type UNINSTALL prompt (piped, test mode): a wrong answer cancels, UNINSTALL proceeds"
    tree_full
    env_set "IHWIP_TEST_STDIN_TTY=1"
    tree_sum "$ROOT" >"$T/before.sum"
    input uninstall
    run_rescue_in "$T/in.txt" --uninstall --network "dhcp:Dante Primary"
    assert_rc 2
    out_has "Type UNINSTALL to continue:"
    err_has "cancelled: nothing has been changed"
    tree_sum "$ROOT" >"$T/after.sum"
    same_file "$T/before.sum" "$T/after.sum"
    no_mutating_calls
    input UNINSTALL
    run_rescue_in "$T/in.txt" --uninstall --network "dhcp:Dante Primary"
    assert_rc 0
    writes_are "$(argv_line -setdhcp "Dante Primary")"
    inventory_gone
    end
}

# ---------------------------------------------------------------- journal parser

# jcase <label> <expected first output line prefix>: parses $T/j.json.
jcase() {
    local got
    run_json
    got=$(head -n 1 "$T/out.txt")
    case "$got" in
        "$2"*) ;;
        *) bad "$1: got '$got', want '$2…'" ;;
    esac
    if [ "$RC" -ne 0 ]; then bad "$1: probe exit $RC: $(tr '\n' ' ' <"$T/err.txt")"; fi
}

# jtext <label> <json>  <expected prefix>
jtext() {
    printf '%s' "$2" >"$T/j.json"
    jcase "$1" "$3"
}

test_journal_parser() {
    local i big
    begin "35  journal parser (sourced): escapes, order, unknown keys, and strict rejection"
    jtext escapes '{"schema":1,"services":[{"service":"A \"q\" \\ \/ caf\u00e9 \ud83d\ude00 x","getinfo":"L1\nL2\tT\r\b\f end","recorded_at":"t","dns":"","search":""}]}' "STATE=ok N=1 REASON="
    printf 'A "q" \\ / caf\303\251 \360\237\230\200 x' >"$T/want.svc"
    printf 'L1\nL2\tT\r\b\f end' >"$T/want.info"
    same_file "$T/j.svc.0" "$T/want.svc"
    same_file "$T/j.info.0" "$T/want.info"
    jtext order-and-unknown-keys $'\r\n  { "services" : [ { "extra":{"a":[1,2,{"b":null}],"c":true}, "getinfo":"DHCP Configuration", "service":"Wi-Fi", "n":-1.5e3, "f":false } ] , "writer":"helper 2.0", "schema" : 1 }\n' "STATE=ok N=1"
    if [ "$(cat "$T/j.svc.0" 2>/dev/null)" != "Wi-Fi" ]; then bad "order-and-unknown-keys: service not parsed"; fi
    jtext empty-list '{"schema":1,"services":[]}' "STATE=ok N=0"
    jtext schema-2 '{"schema":2,"services":[]}' "STATE=invalid N=0 REASON=unknown schema 2"
    jtext schema-string '{"schema":"1","services":[]}' "STATE=invalid N=0 REASON=unknown schema (not a number)"
    jtext schema-float '{"schema":1.0,"services":[]}' "STATE=invalid N=0 REASON=unknown schema 1.0"
    jtext no-schema '{"services":[]}' "STATE=invalid N=0 REASON=no schema number"
    jtext no-services '{"schema":1}' "STATE=invalid N=0 REASON=no services list"
    jtext dup-schema '{"schema":1,"schema":1,"services":[]}' "STATE=invalid N=0 REASON=not valid JSON: \"schema\" appears twice"
    jtext dup-key '{"schema":1,"services":[{"service":"A","service":"B","getinfo":""}]}' "STATE=invalid N=0 REASON=schema 1 but \"service\" appears twice in an entry"
    jtext dup-service '{"schema":1,"services":[{"service":"A","getinfo":""},{"service":"A","getinfo":""}]}' "STATE=invalid N=0 REASON=schema 1 but a service is listed twice"
    jtext truncated '{"schema":1,"services":[{"service":"Dante' "STATE=invalid N=0 REASON=not valid JSON: unterminated string"
    jtext trailing '{"schema":1,"services":[]} x' "STATE=invalid N=0 REASON=not valid JSON: unexpected text after the end"
    jtext two-objects '{"schema":1,"services":[]}{}' "STATE=invalid N=0 REASON=not valid JSON: unexpected text after the end"
    jtext raw-newline $'{"schema":1,"services":[{"service":"A\nB","getinfo":""}]}' "STATE=invalid N=0 REASON=not valid JSON: control character inside a string"
    jtext name-newline '{"schema":1,"services":[{"service":"A\nB","getinfo":""}]}' "STATE=invalid N=0 REASON=schema 1 but a service name contains a control character"
    jtext lone-surrogate '{"schema":1,"services":[{"service":"A\ud83d x","getinfo":""}]}' "STATE=invalid N=0 REASON=not valid JSON: unpaired surrogate"
    jtext nul-escape '{"schema":1,"services":[{"service":"A\u0000","getinfo":""}]}' "STATE=invalid N=0 REASON=not valid JSON: NUL character in a string"
    jtext bad-escape '{"schema":1,"services":[{"service":"A\x41","getinfo":""}]}' "STATE=invalid N=0 REASON=not valid JSON: unknown escape in a string"
    jtext number-name '{"schema":1,"services":[{"service":5,"getinfo":""}]}' "STATE=invalid N=0 REASON=schema 1 but \"service\" is not a string"
    jtext null-dns '{"schema":1,"services":[{"service":"A","getinfo":"","dns":null}]}' "STATE=invalid N=0 REASON=schema 1 but \"dns\" is not a string"
    jtext no-getinfo '{"schema":1,"services":[{"service":"A"}]}' "STATE=invalid N=0 REASON=schema 1 but an entry has no getinfo text"
    jtext empty-name '{"schema":1,"services":[{"service":"","getinfo":""}]}' "STATE=invalid N=0 REASON=schema 1 but an entry has no service name"
    jtext services-object '{"schema":1,"services":{}}' "STATE=invalid N=0 REASON=schema 1 but expected '['"
    jtext not-object '[1]' "STATE=invalid N=0 REASON=not valid JSON: expected '{'"
    jtext empty-file '' "STATE=invalid N=0 REASON=not valid JSON: expected '{'"
    jtext bare-word '{"schema":1,"services":[],"x":nope}' "STATE=invalid N=0 REASON=not valid JSON: unexpected word"
    big=""
    i=0
    while [ "$i" -lt 40 ]; do
        big="${big}["
        i=$((i + 1))
    done
    jtext too-deep "{\"schema\":1,\"services\":[],\"x\":$big}" "STATE=invalid N=0 REASON=not valid JSON: nested too deeply"
    printf '{"schema":1,\000"services":[]}' >"$T/j.json"
    jcase nul-byte "STATE=invalid N=0 REASON=contains NUL bytes"
    head -c 70000 /dev/zero | tr '\0' ' ' >"$T/j.json"
    jcase too-large "STATE=invalid N=0 REASON=larger than 64 KB"
    rm -f "$T/j.json"
    printf '{"schema":1,"services":[]}' >"$T/elsewhere.json"
    ln -s "$T/elsewhere.json" "$T/j.json"
    jcase symlink "STATE=invalid N=0 REASON=not a regular file"
    rm -f "$T/j.json"
    jcase missing "STATE=none N=0 REASON="
    end

    local t0 svcs=""
    begin "35b journal parser speed: 40 services with real-sized getinfo text parse in under 5 s"
    i=0
    while [ "$i" -lt 40 ]; do
        if [ -n "$svcs" ]; then svcs="$svcs,"; fi
        svcs="$svcs{\"service\":\"Service $i\",\"recorded_at\":\"2026-11-01T10:00:00Z\",\"getinfo\":\"Manual Configuration\\nIP address: 10.0.$i.5\\nSubnet mask: 255.255.255.0\\nRouter: 10.0.$i.1\\nIPv6: Automatic\\nIPv6 IP address: none\\nIPv6 Router: none\\nEthernet Address: 00:11:22:33:44:55\\n\",\"dns\":\"There aren't any DNS Servers set on Service $i.\",\"search\":\"There aren't any Search Domains set on Service $i.\"}"
        i=$((i + 1))
    done
    printf '{"schema":1,"services":[%s]}' "$svcs" >"$T/j.json"
    t0=$SECONDS
    jcase forty "STATE=ok N=40"
    if [ $((SECONDS - t0)) -ge 5 ]; then bad "parsing took $((SECONDS - t0)) s"; fi
    end
}

test_stage_b_finished() {
    local p
    begin "36  every stage B process started by the suite has finished"
    for p in $STAGE_B_PIDS; do
        if stage_b_alive "$p"; then bad "stage B process $p is still running"; fi
    done
    if [ -z "$STAGE_B_PIDS" ]; then bad "no stage B process was recorded"; fi
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
test_no_journal_plan
test_dhcp_pick
test_bad_picks
test_journal_plan
test_helper_journal_plan
test_journal_restore
test_journal_overlap
test_journal_fallback
test_journal_injection
test_plan_network_values
test_network_edges
test_wait_pid
test_wait_pid_timeout
test_wait_pid_gone
test_menu
test_confirm_prompt
test_journal_parser
test_stage_b_finished
test_syntax_and_lint
test_no_real_tools

echo
echo "summary: $PASSED passed, $FAILED failed"
if [ "$FAILED" -ne 0 ]; then
    printf 'failed:\n%s' "$FAILED_NAMES"
    exit 1
fi
exit 0
