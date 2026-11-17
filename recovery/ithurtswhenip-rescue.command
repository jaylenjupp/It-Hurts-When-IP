#!/bin/bash
#
# ithurtswhenip-rescue.command - standalone uninstall and rescue for
# "It Hurts When IP" (and old "IP Switcher" installs).
#
# Runs on: any Mac with the app installed, in Terminal. It needs nothing but
# macOS built-ins and bash 3.2, and it never needs the app or its helper.
#
#     sudo bash ithurtswhenip-rescue.command               # numbered menu
#     sudo bash ithurtswhenip-rescue.command --plan        # shows every step, changes nothing
#     sudo bash ithurtswhenip-rescue.command --uninstall   # removes everything (asks first)
#     sudo bash ithurtswhenip-rescue.command --uninstall --network dhcp:"Wi-Fi"
#     sudo bash ithurtswhenip-rescue.command --repair soft # flush DNS, renew DHCP leases
#     sudo bash ithurtswhenip-rescue.command --repair service:"Wi-Fi"
#
# Every removal is planned, backed up to /Users/Shared/ItHurtsWhenIP-backups/<UTC time>/,
# executed, then verified. Unknown files that merely look related are reported,
# never deleted. Network settings are left exactly as they are unless you pick
# a change: --network dhcp:"<service>" (that service only, repeatable) and/or
# --network restore-journal (the IPv4 settings the app recorded before it first
# changed a service). --wait-pid <pid> (used by the app) does everything except
# the app bundle and per-user data now, and removes those in the background
# once that process has exited.
#
# --repair is its own action: "soft" flushes the DNS cache and renews the DHCP
# lease of every enabled DHCP service that has an attached network device;
# "service:<name>" backs up one service's settings, switches it to DHCP, clears
# its DNS servers and search domains and turns it off and on again. Both are
# planned, confirmed (type REPAIR), backed up and verified like --uninstall.
#
# Test mode (never as root): IHWIP_ROOT=<fake tree> plus IHWIP_NETWORKSETUP,
# IHWIP_LAUNCHCTL, IHWIP_PKGUTIL, IHWIP_PGREP, IHWIP_PKILL, IHWIP_IPCONFIG,
# IHWIP_DSCACHEUTIL and IHWIP_KILLALL pointing at fakes, optional
# IHWIP_NOW=YYYYMMDDTHHMMSSZ, IHWIP_TEST_STDIN_TTY=1 (treat piped stdin as a
# terminal, to drive the menu and the UNINSTALL/REPAIR prompts),
# IHWIP_WAIT_TIMEOUT=<1-600> (seconds stage B waits) and IHWIP_REPAIR_WAIT=<1-60>
# (seconds a repair waits for addresses). See recovery/tests/run-tests.sh.
# As root, or without IHWIP_ROOT, every IHWIP_* variable is ignored.
#
# Exit codes: 0 ok, 1 partial (some steps failed, listed at the end), 2 refused.

set -u

RESCUE_VERSION=1
SCRIPT_NAME="ithurtswhenip-rescue.command"
TAB=$'\t'
NL=$'\n'

APP_PROC="it-hurts-when-ip"
APP_BUNDLE="/Applications/It Hurts When IP.app"
JOURNAL_FILE="/Library/Application Support/com.ithurtswhenip.helper/journal.json"
JOURNAL_MAX_BYTES=65536
JSON_MAX_DEPTH=32
LABELS="com.ithurtswhenip.helper com.ipswitcher.helper"
RECEIPTS="com.ithurtswhenip.pkg com.ipswitcher.pkg com.ipswitcher.helper"
USER_IDS="com.ithurtswhenip.desktop com.ithurtswhenip.app com.ithurtswhenip.switcher com.ipswitcher.switcher com.ipswitcher.app it-hurts-when-ip"
BACKUP_PARENT="/Users/Shared"
BACKUP_NAME="ItHurtsWhenIP-backups"
QUIT_POLLS=20 # x 0.5 s = 10 s
NET_VERIFY_POLLS=3 # -getinfo reads, 1 s apart
WAIT_TIMEOUT_DEFAULT=300
REPAIR_WAIT_DEFAULT=10 # seconds a repair's verify waits for DHCP addresses (1 s polls)

REAL_NETWORKSETUP="/usr/sbin/networksetup"
REAL_LAUNCHCTL="/bin/launchctl"
REAL_PKGUTIL="/usr/sbin/pkgutil"
REAL_PGREP="/usr/bin/pgrep"
REAL_PKILL="/usr/bin/pkill"
REAL_IPCONFIG="/usr/sbin/ipconfig"
REAL_DSCACHEUTIL="/usr/bin/dscacheutil"
REAL_KILLALL="/usr/bin/killall"
PS_TOOL="/bin/ps" # read-only process lookups for --wait-pid

OVERRIDE_VARS="IHWIP_ROOT IHWIP_NETWORKSETUP IHWIP_LAUNCHCTL IHWIP_PKGUTIL IHWIP_PGREP IHWIP_PKILL IHWIP_IPCONFIG IHWIP_DSCACHEUTIL IHWIP_KILLALL IHWIP_NOW IHWIP_TEST_STDIN_TTY IHWIP_WAIT_TIMEOUT IHWIP_REPAIR_WAIT"
TOOL_VARS="IHWIP_NETWORKSETUP IHWIP_LAUNCHCTL IHWIP_PKGUTIL IHWIP_PGREP IHWIP_PKILL IHWIP_IPCONFIG IHWIP_DSCACHEUTIL IHWIP_KILLALL"

# System inventory, in plan order: path | back up first (y/n) | kind (file|dir|socket|app).
SYSTEM_ITEMS="
/Library/LaunchDaemons/com.ithurtswhenip.helper.plist|y|file
/Library/PrivilegedHelperTools/com.ithurtswhenip.helper|n|file
/var/run/com.ithurtswhenip.helper.sock|n|socket
/var/log/com.ithurtswhenip.helper.log|y|file
/var/log/com.ithurtswhenip.helper.error.log|y|file
/Library/Application Support/com.ithurtswhenip.helper|y|dir
/Applications/It Hurts When IP.app|n|app
/Library/LaunchDaemons/com.ipswitcher.helper.plist|y|file
/Library/PrivilegedHelperTools/com.ipswitcher.helper|n|file
/var/run/com.ipswitcher.helper.sock|n|socket
/var/log/com.ipswitcher.helper.log|y|file
/var/log/com.ipswitcher.helper.error.log|y|file
"

# Per-user inventory (relative to each home, @ID@ = each of USER_IDS): path | back up first.
USER_ITEMS="
Library/Application Support/@ID@|y
Library/Caches/@ID@|n
Library/WebKit/@ID@|n
Library/HTTPStorages/@ID@|n
Library/HTTPStorages/@ID@.binarycookies|n
Library/Preferences/@ID@.plist|y
Library/Saved Application State/@ID@.savedState|n
Library/Logs/@ID@|y
"

# ---------------------------------------------------------------- output / log

LOG_FILE=""
PENDING_LOG=""

log_line() {
    if [ -n "$LOG_FILE" ]; then
        printf '%s\n' "$1" >>"$LOG_FILE" 2>/dev/null || true
    else
        PENDING_LOG="$PENDING_LOG$1$NL"
    fi
}

open_log() {
    LOG_FILE="$1"
    if [ -n "$PENDING_LOG" ]; then
        printf '%s' "$PENDING_LOG" >>"$LOG_FILE" 2>/dev/null || true
        PENDING_LOG=""
    fi
}

say() {
    printf '%s\n' "$1"
    log_line "$1"
}

notice() {
    printf 'rescue: note: %s\n' "$1" >&2
}

refuse() {
    printf 'rescue: %s\n' "$1" >&2
    log_line "# refused: $1"
    exit 2
}

one_line() {
    local v="$1"
    v="${v%%"$NL"*}"
    v="${v//"$TAB"/ }"
    printf '%s' "$v"
}

# ---------------------------------------------------------------- help / args

print_usage() {
    cat <<EOF
Usage: sudo bash $SCRIPT_NAME [--plan | --uninstall] [options]
       sudo bash $SCRIPT_NAME [--plan] --repair soft|service:"<service>" [--yes] [--log <path>]
       sudo bash $SCRIPT_NAME            (numbered menu, in Terminal)
       bash $SCRIPT_NAME --help
EOF
}

print_help() {
    cat <<EOF
It Hurts When IP rescue script, version $RESCUE_VERSION

Removes It Hurts When IP completely (app, privileged helper, settings and old
"IP Switcher" files), without needing the app or its helper to work. Network
settings are only changed if you choose a change below.

Usage: sudo bash $SCRIPT_NAME [--plan | --uninstall] [options]

Actions:
  --plan, --dry-run     Print every step --uninstall would take with the
                        options given. Changes nothing.
  --uninstall           Back up, then remove everything listed by --plan.
                        Needs sudo. Asks you to type UNINSTALL unless --yes.
  --repair ...          Repair network settings (see below). Needs sudo. Asks
                        you to type REPAIR unless --yes.
  (no action)           In Terminal: a numbered menu (show the plan, uninstall
                        with a network choice, repair the network, quit).
                        Otherwise: print the plan and exit 2.

Network options (may be repeated; the default is leave):
  --network leave       Leave network settings exactly as they are. Cannot be
                        combined with the other --network choices.
  --network dhcp:"<service>"
                        Switch this one service to DHCP. The name must match
                        exactly (see the NETWORK lines of --plan) and the
                        service must be enabled. Services you do not name are
                        not touched.
  --network restore-journal
                        Put back the IPv4 settings (DHCP, or manual address,
                        mask and router) the app recorded before it first
                        changed each service. DNS servers and search domains
                        are not restored by this version. A service that is
                        also named with dhcp: gets DHCP. Services that already
                        match their recorded settings are not touched.

Network repair (its own action: it cannot be combined with --uninstall,
--network or --wait-pid; add --plan to only see what it would do):
  --repair soft         Flush the DNS cache and ask every enabled service that
                        is on DHCP for a fresh lease (ipconfig set <device>
                        DHCP). Services on Manual (static) settings, disabled
                        services and services without an attached network
                        device are not touched. No settings are changed.
  --repair service:"<service>"
                        Reset one enabled service: switch it to DHCP, clear its
                        DNS servers and search domains, then turn it off and on
                        again. The plan shows the current values first, and
                        they are backed up before anything changes.
  --repair full         A full network reset is not part of this version.

Other options:
  --yes                 Do not ask for confirmation (the app passes this after
                        asking you itself).
  --log <path>          Also append all output to <path>; its folder must exist.
                        Default for --uninstall and --repair: rescue.log in the
                        backup folder.
  --wait-pid <pid>      With --uninstall (used by the app): stage A does
                        everything except the app bundle and per-user data now;
                        stage B removes those in the background once process
                        <pid> has exited (it waits up to $WAIT_TIMEOUT_DEFAULT s) and appends
                        its result to the log. This script never signals that
                        process. With --plan it only shows the two stages.
  --help                Show this help.
  --version             Print the script version.

Exit codes:
  0  everything worked
  1  some steps failed (each one is listed at the end)
  2  refused: bad arguments, not run with sudo, cancelled or quit, a --network
     or --repair choice that does not fit this Mac, or no action given outside
     Terminal

Example (changes nothing):
  sudo bash $SCRIPT_NAME --plan

--plan only reads: it never changes files, background services, receipts or
network settings. A repair whose only problem is that a service has no DHCP
address yet still exits 0; the result lists it under warnings.
Backups go to $BACKUP_PARENT/$BACKUP_NAME/<UTC time>/ and are kept.
EOF
}

usage_error() {
    printf 'rescue: %s\n\n' "$1" >&2
    print_usage >&2
    exit 2
}

set_action() {
    if [ -n "$ACTION" ] && [ "$ACTION" != "$1" ]; then
        usage_error "choose one action: --plan or --uninstall"
    fi
    ACTION="$1"
}

need_value() {
    if [ $# -lt 2 ]; then
        usage_error "missing value for $1"
    fi
    case "$2" in
        "" | --*) usage_error "missing value for $1" ;;
    esac
}

check_network_value() {
    local svc
    case "$1" in
        leave | restore-journal) ;;
        dhcp:*)
            svc="${1#dhcp:}"
            if [ -z "$svc" ]; then
                usage_error "--network dhcp: needs a service name, e.g. --network dhcp:\"Wi-Fi\""
            fi
            case "$svc" in
                *"$TAB"* | *"$NL"*) usage_error "service names must not contain tabs or line breaks" ;;
            esac
            ;;
        *) usage_error "unknown --network value: $1 (use leave, restore-journal or dhcp:<service>)" ;;
    esac
}

# check_repair_value <value>: the one place that knows the --repair tiers.
# Sets REPAIR_TIER (soft|service) and REPAIR_SVC.
check_repair_value() {
    local svc
    case "$1" in
        soft) REPAIR_TIER=soft ;;
        service:*)
            svc="${1#service:}"
            if [ -z "$svc" ]; then
                usage_error "--repair service: needs a service name, e.g. --repair service:\"Wi-Fi\""
            fi
            case "$svc" in
                *"$TAB"* | *"$NL"*) usage_error "service names must not contain tabs or line breaks" ;;
            esac
            REPAIR_TIER=service
            REPAIR_SVC="$svc"
            ;;
        full)
            # Phase 3.2 fills this in (typed confirm word: RESET).
            refuse "--repair full (full network reset) is not part of this version of the script. Nothing has been changed."
            ;;
        *) usage_error "unknown --repair value: $1 (use soft or service:<service>)" ;;
    esac
}

set_repair() {
    if [ -n "$REPAIR_TIER" ]; then
        usage_error "choose one --repair"
    fi
    case "$1" in
        "" | --*) usage_error "missing value for --repair" ;;
    esac
    REPAIR_ARG="$1"
    check_repair_value "$1"
}

parse_args() {
    ACTION=""
    REPAIR_TIER=""
    REPAIR_SVC=""
    REPAIR_ARG=""
    YES=0
    LOG_ARG=""
    LOG_GIVEN=0
    WAIT_PID=""
    SHOW_HELP=0
    SHOW_VERSION=0
    NET_CHOICES=""
    NET_GIVEN=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --plan | --dry-run) set_action plan ;;
            --uninstall) set_action uninstall ;;
            --network)
                need_value "$@"
                check_network_value "$2"
                NET_CHOICES="$NET_CHOICES$2$NL"
                NET_GIVEN=1
                shift
                ;;
            --yes) YES=1 ;;
            --log)
                need_value "$@"
                LOG_ARG="$2"
                LOG_GIVEN=1
                shift
                ;;
            --wait-pid)
                need_value "$@"
                case "$2" in
                    *[!0-9]* | 0*) usage_error "--wait-pid needs a positive whole number, got: $2" ;;
                esac
                if [ ${#2} -gt 10 ]; then
                    usage_error "--wait-pid value is too large: $2"
                fi
                WAIT_PID="$2"
                shift
                ;;
            --repair)
                need_value "$@"
                set_repair "$2"
                shift
                ;;
            --repair=*) set_repair "${1#--repair=}" ;;
            -h | --help) SHOW_HELP=1 ;;
            --version) SHOW_VERSION=1 ;;
            *) usage_error "unknown option: $1" ;;
        esac
        shift
    done
}

# check_network_choices: the rules that need no system state (the flags and the
# menu both end up here). Reads NET_CHOICES (one choice per line); sets
# NET_RESTORE (0/1), PICKS (dhcp: service names, de-duplicated, in the order
# given) and NET_LABEL (leave|restore-journal|dhcp|restore-journal+dhcp).
check_network_choices() {
    local c svc i dup leave=0 other=0
    NET_RESTORE=0
    PICKS=()
    while IFS= read -r c; do
        [ -n "$c" ] || continue
        case "$c" in
            leave) leave=1 ;;
            restore-journal)
                NET_RESTORE=1
                other=1
                ;;
            dhcp:*)
                other=1
                svc="${c#dhcp:}"
                dup=0
                i=0
                while [ "$i" -lt ${#PICKS[@]} ]; do
                    if [ "${PICKS[i]}" = "$svc" ]; then dup=1; fi
                    i=$((i + 1))
                done
                if [ "$dup" -eq 0 ]; then
                    PICKS[${#PICKS[@]}]="$svc"
                fi
                ;;
        esac
    done <<<"$NET_CHOICES"
    if [ "$leave" -eq 1 ] && [ "$other" -eq 1 ]; then
        usage_error "--network leave cannot be combined with other --network choices"
    fi
    NET_LABEL="leave"
    if [ "$NET_RESTORE" -eq 1 ] && [ ${#PICKS[@]} -gt 0 ]; then
        NET_LABEL="restore-journal+dhcp"
    elif [ "$NET_RESTORE" -eq 1 ]; then
        NET_LABEL="restore-journal"
    elif [ ${#PICKS[@]} -gt 0 ]; then
        NET_LABEL="dhcp"
    fi
}

# ---------------------------------------------------------------- guards

is_real_tool() {
    local dir="${1%/*}"
    [ -n "$dir" ] || dir="/"
    dir=$(cd -P -- "$dir" 2>/dev/null && pwd -P) || return 0
    case "$dir" in
        / | /bin | /sbin | /usr/bin | /usr/sbin | /usr/libexec | /usr/local/bin | /usr/local/sbin | /opt/homebrew/bin | /opt/homebrew/sbin) return 0 ;;
    esac
    return 1
}

looks_like_real_system() {
    local x a b
    for x in Library Users Applications private/var var; do
        [ -e "$1/$x" ] || continue
        a=$(stat -L -f '%d:%i' "$1/$x" 2>/dev/null) || continue
        b=$(stat -L -f '%d:%i' "/$x" 2>/dev/null) || continue
        if [ "$a" = "$b" ]; then
            return 0
        fi
    done
    return 1
}

# apply_guards <euid>: decides the effective root and tools.
#  - root: every IHWIP_* override is dropped, real paths and tools.
#  - not root, IHWIP_ROOT set: test mode; the root must be a real directory
#    other than / and every tool must be overridden by a non-system executable.
#  - not root, no IHWIP_ROOT: real paths and tools (plan/help/version only).
apply_guards() {
    local euid="$1" v val res any=0
    NS="$REAL_NETWORKSETUP"
    LAUNCHCTL="$REAL_LAUNCHCTL"
    PKGUTIL="$REAL_PKGUTIL"
    PGREP="$REAL_PGREP"
    PKILL="$REAL_PKILL"
    IPCONFIG="$REAL_IPCONFIG"
    DSCACHEUTIL="$REAL_DSCACHEUTIL"
    KILLALL="$REAL_KILLALL"
    R=""
    TEST_MODE=0
    WAIT_TIMEOUT="$WAIT_TIMEOUT_DEFAULT"
    REPAIR_WAIT="$REPAIR_WAIT_DEFAULT"

    if [ "$euid" = "0" ] || [ -z "${IHWIP_ROOT+x}" ]; then
        for v in $OVERRIDE_VARS; do
            if [ -n "${!v+x}" ]; then
                any=1
                unset "$v"
            fi
        done
        if [ "$any" -eq 1 ]; then
            if [ "$euid" = "0" ]; then
                notice "running as root: ignoring IHWIP_* test overrides"
            else
                notice "IHWIP_ROOT is not set: ignoring IHWIP_* test overrides"
            fi
        fi
        return 0
    fi

    val="$IHWIP_ROOT"
    case "$val" in
        /*) ;;
        *) refuse "IHWIP_ROOT must be an absolute path to an existing directory" ;;
    esac
    if [ ! -d "$val" ]; then
        refuse "IHWIP_ROOT is not an existing directory: $val"
    fi
    res=$(cd -P -- "$val" 2>/dev/null && pwd -P) || refuse "IHWIP_ROOT cannot be resolved: $val"
    if [ -z "$res" ] || [ "$res" = "/" ]; then
        refuse "IHWIP_ROOT must not be /"
    fi
    if looks_like_real_system "$res"; then
        refuse "IHWIP_ROOT points at the real system: $val"
    fi
    for v in $TOOL_VARS; do
        if [ -z "${!v+x}" ] || [ -z "${!v}" ]; then
            refuse "test mode needs $v set to a fake tool (a fake root is never paired with real tools)"
        fi
        val="${!v}"
        case "$val" in
            /*) ;;
            *) refuse "$v must be an absolute path: $val" ;;
        esac
        if [ -L "$val" ]; then
            refuse "$v must not be a symlink: $val"
        fi
        if [ ! -f "$val" ] || [ ! -x "$val" ]; then
            refuse "$v is not an executable file: $val"
        fi
        if is_real_tool "$val"; then
            refuse "$v points into a system tools folder: $val"
        fi
    done
    if [ -n "${IHWIP_NOW+x}" ]; then
        case "$IHWIP_NOW" in
            [0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]T[0-9][0-9][0-9][0-9][0-9][0-9]Z) ;;
            *) refuse "IHWIP_NOW must look like 20261115T120000Z, got: $IHWIP_NOW" ;;
        esac
    fi
    if [ -n "${IHWIP_TEST_STDIN_TTY+x}" ] && [ "$IHWIP_TEST_STDIN_TTY" != "1" ]; then
        refuse "IHWIP_TEST_STDIN_TTY must be 1 when set, got: $IHWIP_TEST_STDIN_TTY"
    fi
    if [ -n "${IHWIP_WAIT_TIMEOUT+x}" ]; then
        case "$IHWIP_WAIT_TIMEOUT" in
            [1-9] | [1-9][0-9] | [1-5][0-9][0-9] | 600) WAIT_TIMEOUT="$IHWIP_WAIT_TIMEOUT" ;;
            *) refuse "IHWIP_WAIT_TIMEOUT must be 1 to 600 (seconds), got: $IHWIP_WAIT_TIMEOUT" ;;
        esac
    fi
    if [ -n "${IHWIP_REPAIR_WAIT+x}" ]; then
        case "$IHWIP_REPAIR_WAIT" in
            [1-9] | [1-5][0-9] | 60) REPAIR_WAIT="$IHWIP_REPAIR_WAIT" ;;
            *) refuse "IHWIP_REPAIR_WAIT must be 1 to 60 (seconds), got: $IHWIP_REPAIR_WAIT" ;;
        esac
    fi
    R="$res"
    TEST_MODE=1
    NS="$IHWIP_NETWORKSETUP"
    LAUNCHCTL="$IHWIP_LAUNCHCTL"
    PKGUTIL="$IHWIP_PKGUTIL"
    PGREP="$IHWIP_PGREP"
    PKILL="$IHWIP_PKILL"
    IPCONFIG="$IHWIP_IPCONFIG"
    DSCACHEUTIL="$IHWIP_DSCACHEUTIL"
    KILLALL="$IHWIP_KILLALL"
    return 0
}

# mode_allowed <action> <euid>: --uninstall and --repair need root, or test mode.
mode_allowed() {
    if [ "$1" = "uninstall" ] && [ "$2" != "0" ] && [ "$TEST_MODE" -ne 1 ]; then
        refuse "--uninstall needs administrator rights. Run:  sudo bash $SCRIPT_NAME --uninstall"
    fi
    if [ "$1" = "repair" ] && [ "$2" != "0" ] && [ "$TEST_MODE" -ne 1 ]; then
        local hint="soft"
        if [ -n "${REPAIR_ARG:-}" ]; then hint=$(printf '%q' "$REPAIR_ARG"); fi
        refuse "--repair needs administrator rights. Run:  sudo bash $SCRIPT_NAME --repair $hint"
    fi
    return 0
}

# stdin_is_terminal: a real terminal, or (test mode only) IHWIP_TEST_STDIN_TTY=1.
stdin_is_terminal() {
    if [ -t 0 ]; then
        return 0
    fi
    if [ "$TEST_MODE" -eq 1 ] && [ "${IHWIP_TEST_STDIN_TTY:-}" = "1" ]; then
        return 0
    fi
    return 1
}

setup_log() {
    local dir
    [ "$LOG_GIVEN" -eq 1 ] || return 0
    case "$LOG_ARG" in
        */*)
            dir="${LOG_ARG%/*}"
            [ -n "$dir" ] || dir="/"
            ;;
        *) dir="." ;;
    esac
    if [ ! -d "$dir" ]; then
        refuse "--log: folder does not exist: $dir"
    fi
    if [ -L "$LOG_ARG" ] || { [ -e "$LOG_ARG" ] && [ ! -f "$LOG_ARG" ]; }; then
        refuse "--log: not a regular file (or a symlink): $LOG_ARG"
    fi
    if ! { : >>"$LOG_ARG"; } 2>/dev/null; then
        refuse "--log: cannot write to $LOG_ARG"
    fi
    open_log "$LOG_ARG"
}

init_time() {
    if [ "$TEST_MODE" -eq 1 ] && [ -n "${IHWIP_NOW:-}" ]; then
        TS="$IHWIP_NOW"
    else
        TS=$(date -u +%Y%m%dT%H%M%SZ)
    fi
}

# ---------------------------------------------------------------- inventory

# list_user_dirs: home folder names under /Users (no Shared, Guest, dot-dirs
# or symlinks), unsorted. (A function because bash 3.2 cannot parse `case`
# inside `$( )`.)
list_user_dirs() {
    local d name
    for d in "$R/Users"/*; do
        [ -d "$d" ] || continue
        [ -L "$d" ] && continue
        name="${d##*/}"
        case "$name" in
            Shared | Guest) continue ;;
        esac
        printf '%s\n' "$name"
    done
}

collect_users() {
    local name list
    USERS=()
    list=$(list_user_dirs | LC_ALL=C sort)
    while IFS= read -r name; do
        if [ -n "$name" ]; then
            USERS[${#USERS[@]}]="$name"
        fi
    done <<<"$list"
}

# add_item <path> <backup y/n> <kind hint> <anchor> <backup rel> <stage A|B>.
# Stage B (the app bundle and per-user data) waits for --wait-pid, if given.
add_item() {
    local n=${#ITEM_PATH[@]}
    ITEM_PATH[n]="$1"
    ITEM_BACKUP[n]="$2"
    ITEM_HINT[n]="$3"
    ITEM_ANCHOR[n]="$4"
    ITEM_BREL[n]="$5"
    ITEM_STAGE[n]="$6"
    ITEM_EXISTS[n]=0
    ITEM_KIND[n]=""
    ITEM_LINK[n]=0
    ITEM_UNSAFE[n]=""
    ITEM_SKIP[n]=""
    INV_LIST="$INV_LIST$1$NL"
}

anchor_of() {
    local rest="${1#/}" first
    first="${rest%%/*}"
    if [ "$first" = "var" ]; then
        rest="${rest#var/}"
        printf '/var/%s' "${rest%%/*}"
    else
        printf '/%s' "$first"
    fi
}

build_inventory() {
    local p b k u id t rel i stage
    ITEM_PATH=()
    ITEM_BACKUP=()
    ITEM_HINT=()
    ITEM_ANCHOR=()
    ITEM_BREL=()
    ITEM_STAGE=()
    ITEM_EXISTS=()
    ITEM_KIND=()
    ITEM_LINK=()
    ITEM_UNSAFE=()
    ITEM_SKIP=()
    INV_LIST="$NL"
    while IFS='|' read -r p b k; do
        [ -n "$p" ] || continue
        stage=A
        if [ "$k" = "app" ]; then stage=B; fi
        add_item "$p" "$b" "$k" "$(anchor_of "$p")" "system$p" "$stage"
    done <<<"$SYSTEM_ITEMS"
    collect_users
    i=0
    while [ "$i" -lt ${#USERS[@]} ]; do
        u="${USERS[i]}"
        for id in $USER_IDS; do
            while IFS='|' read -r t b; do
                [ -n "$t" ] || continue
                rel="${t//@ID@/$id}"
                add_item "/Users/$u/$rel" "$b" "" "/Users/$u" "users/$u/$rel" B
            done <<<"$USER_ITEMS"
        done
        i=$((i + 1))
    done
}

# stage_match <item index> <all|A|B>
stage_match() {
    [ "$2" = "all" ] || [ "${ITEM_STAGE[$1]}" = "$2" ]
}

# parent_ok <path> <anchor>: the item's parent folder must resolve to exactly
# where it is expected (resolved anchor + the literal rest), so a symlinked
# folder on the way can never redirect a backup or a removal. Sets PARENT_REASON.
parent_ok() {
    local full="$R$1" anchor_full="$R$2" parent rel ares pres
    PARENT_REASON=""
    parent="${full%/*}"
    case "$parent" in
        "$anchor_full") rel="" ;;
        "$anchor_full"/*) rel="${parent#"$anchor_full"}" ;;
        *)
            PARENT_REASON="parent resolves outside expected location"
            return 1
            ;;
    esac
    ares=$(cd -P -- "$anchor_full" 2>/dev/null && pwd -P) || {
        PARENT_REASON="cannot resolve $2"
        return 1
    }
    pres=$(cd -P -- "$parent" 2>/dev/null && pwd -P) || {
        PARENT_REASON="cannot resolve parent folder"
        return 1
    }
    if [ "$pres" != "$ares$rel" ]; then
        PARENT_REASON="parent resolves outside expected location"
        return 1
    fi
    if [ "$TEST_MODE" -eq 1 ]; then
        case "$pres/" in
            "$R"/*) ;;
            *)
                PARENT_REASON="parent resolves outside expected location"
                return 1
                ;;
        esac
    fi
    return 0
}

set_kind() {
    local i="$1" full="$R${ITEM_PATH[$1]}"
    ITEM_LINK[i]=0
    if [ -L "$full" ]; then
        ITEM_LINK[i]=1
    fi
    case "${ITEM_HINT[i]}" in
        app | socket)
            ITEM_KIND[i]="${ITEM_HINT[i]}"
            return 0
            ;;
    esac
    if [ -L "$full" ]; then
        ITEM_KIND[i]="file"
    elif [ -d "$full" ]; then
        ITEM_KIND[i]="dir"
    else
        ITEM_KIND[i]="file"
    fi
}

exists() {
    [ -e "$1" ] || [ -L "$1" ]
}

label_loaded() {
    local out rc
    out=$("$LAUNCHCTL" print "system/$1" 2>&1 </dev/null)
    rc=$?
    [ "$rc" -eq 0 ] || return 1
    case "$out" in
        *"Could not find service"*) return 1 ;;
    esac
    return 0
}

read_receipts() {
    PKGS_TEXT=$("$PKGUTIL" --pkgs 2>/dev/null </dev/null)
    PKGS_RC=$?
}

receipt_present() {
    case "$NL$PKGS_TEXT$NL" in
        *"$NL$1$NL"*) return 0 ;;
    esac
    return 1
}

app_running() {
    "$PGREP" -x "$APP_PROC" >/dev/null 2>&1 </dev/null
}

# pid_start <pid>: prints the start time of a live (non-zombie) process, or
# nothing. Read-only: uses ps, never kill (not even kill -0).
pid_start() {
    local info stat
    info=$("$PS_TOOL" -o stat= -o lstart= -p "$1" 2>/dev/null </dev/null) || return 0
    info=$(trim "$info")
    [ -n "$info" ] || return 0
    stat="${info%%[[:space:]]*}"
    case "$stat" in
        Z*) return 0 ;;
    esac
    trim "${info#"$stat"}"
}

# wait_gone <pid> <start time> <limit s>: 0 once the process has exited (or the
# pid now belongs to a different process), 1 after the limit. Never signals it.
wait_gone() {
    local t0=$SECONDS now
    [ -n "$2" ] || return 0
    while :; do
        now=$(pid_start "$1")
        if [ "$now" != "$2" ]; then
            return 0
        fi
        if [ $((SECONDS - t0)) -ge "$3" ]; then
            return 1
        fi
        sleep 0.5
    done
}

ns_error_text() {
    case "$1" in
        *"** Error"* | *"is not a recognized network service"*) return 0 ;;
    esac
    return 1
}

trim() {
    local v="$1"
    v="${v#"${v%%[![:space:]]*}"}"
    v="${v%"${v##*[![:space:]]}"}"
    printf '%s' "$v"
}

# read_network: read-only. Fills NET_SVC / NET_EN (1 enabled, 0 disabled) /
# NET_INFO / NET_RC; NET_ERR on failure.
read_network() {
    local out rc line n en
    NET_SVC=()
    NET_EN=()
    NET_INFO=()
    NET_RC=()
    NET_ERR=""
    out=$("$NS" -listallnetworkservices 2>&1 </dev/null)
    rc=$?
    NET_LIST_TEXT="$out"
    NET_LIST_RC=$rc
    if [ "$rc" -ne 0 ] || ns_error_text "$out"; then
        NET_ERR="could not list network services (exit $rc): $(one_line "$out")"
        return 1
    fi
    while IFS= read -r line; do
        case "$line" in
            "" | "An asterisk"* | "** "*) continue ;;
        esac
        en=1
        case "$line" in
            \**)
                en=0
                line="${line#\*}"
                ;;
        esac
        [ -n "$line" ] || continue
        n=${#NET_SVC[@]}
        NET_SVC[n]="$line"
        NET_EN[n]=$en
        NET_INFO[n]=$("$NS" -getinfo "$line" 2>&1 </dev/null)
        NET_RC[n]=$?
    done <<<"$out"
    return 0
}

# svc_index <name>: exact match in NET_SVC; sets SVC_IDX.
svc_index() {
    local i=0
    SVC_IDX=-1
    while [ "$i" -lt ${#NET_SVC[@]} ]; do
        if [ "${NET_SVC[i]}" = "$1" ]; then
            SVC_IDX=$i
            return 0
        fi
        i=$((i + 1))
    done
    return 1
}

# service_listed <name> <-listallnetworkservices text>: exact match, enabled or not.
service_listed() {
    local line
    while IFS= read -r line; do
        line="${line#\*}"
        if [ "$line" = "$1" ]; then
            return 0
        fi
    done <<<"$2"
    return 1
}

# parse_info <getinfo text>: sets INFO_MANUAL INFO_IP INFO_MASK INFO_ROUTER.
parse_info() {
    local line v
    INFO_MANUAL=0
    INFO_IP="-"
    INFO_MASK="-"
    INFO_ROUTER="-"
    while IFS= read -r line; do
        case "$line" in
            Manual*) INFO_MANUAL=1 ;;
            "IP address:"*)
                v=$(trim "${line#IP address:}")
                if [ "$INFO_IP" = "-" ] && [ -n "$v" ]; then INFO_IP="$v"; fi
                ;;
            "Subnet mask:"*)
                v=$(trim "${line#Subnet mask:}")
                if [ "$INFO_MASK" = "-" ] && [ -n "$v" ]; then INFO_MASK="$v"; fi
                ;;
            "Router:"*)
                v=$(trim "${line#Router:}")
                if [ "$INFO_ROUTER" = "-" ] && [ -n "$v" ]; then INFO_ROUTER="$v"; fi
                ;;
        esac
    done <<<"$1"
}

# norm_addr <value>: "-" for empty, "(null)" and "none"; otherwise unchanged.
norm_addr() {
    case "$1" in
        "" | - | "(null)" | none) printf '%s' "-" ;;
        *) printf '%s' "$1" ;;
    esac
}

# info_view <getinfo text>: strict reading for restore and verify. IV_MODE is
# dhcp or manual only when the first line is exactly "DHCP Configuration" or
# "Manual Configuration" (anything else: unknown). IV_IP IV_MASK IV_ROUTER use
# "-" for empty, "(null)" and "none".
info_view() {
    local line first=""
    while IFS= read -r line; do
        line=$(trim "$line")
        if [ -n "$line" ]; then
            first="$line"
            break
        fi
    done <<<"$1"
    case "$first" in
        "DHCP Configuration") IV_MODE=dhcp ;;
        "Manual Configuration") IV_MODE=manual ;;
        *) IV_MODE=unknown ;;
    esac
    parse_info "$1"
    IV_IP=$(norm_addr "$INFO_IP")
    IV_MASK=$(norm_addr "$INFO_MASK")
    IV_ROUTER=$(norm_addr "$INFO_ROUTER")
}

# is_ipv4 <value>: dotted quad, 0-255, no leading zeros. Pattern checks only.
is_ipv4() {
    local rest o n=0
    case "$1" in
        "" | *[!0-9.]* | .* | *. | *..*) return 1 ;;
    esac
    rest="$1."
    while [ -n "$rest" ]; do
        o="${rest%%.*}"
        rest="${rest#*.}"
        n=$((n + 1))
        case "$o" in
            [0-9] | [1-9][0-9] | 1[0-9][0-9] | 2[0-4][0-9] | 25[0-5]) ;;
            *) return 1 ;;
        esac
    done
    [ "$n" -eq 4 ]
}

# is_mask <value>: IPv4, not 0.0.0.0, ones then zeros. Arithmetic only runs on
# values is_ipv4 has already reduced to plain decimal octets.
is_mask() {
    local rest o m=0 inv
    is_ipv4 "$1" || return 1
    rest="$1."
    while [ -n "$rest" ]; do
        o="${rest%%.*}"
        rest="${rest#*.}"
        m=$(((m << 8) | o))
    done
    [ "$m" -ne 0 ] || return 1
    inv=$((~m & 0xFFFFFFFF))
    [ $((inv & (inv + 1))) -eq 0 ]
}

# addr_disp <value>: for plan records: "-" stays, IPv4 stays, anything else "invalid".
addr_disp() {
    if [ "$1" = "-" ] || is_ipv4 "$1"; then
        printf '%s' "$1"
    else
        printf 'invalid'
    fi
}

# view_text: IV_* as <mode>/<ip>/<mask>/<router>.
view_text() {
    printf '%s/%s/%s/%s' "$IV_MODE" "$(addr_disp "$IV_IP")" "$(addr_disp "$IV_MASK")" "$(addr_disp "$IV_ROUTER")"
}

# current_text <service>: its current settings as <mode>/<ip>/<mask>/<router>
# (mode missing or unreadable when it cannot be read).
current_text() {
    if ! svc_index "$1"; then
        printf 'missing/-/-/-'
    elif [ "${NET_RC[SVC_IDX]}" -ne 0 ] || ns_error_text "${NET_INFO[SVC_IDX]}"; then
        printf 'unreadable/-/-/-'
    else
        info_view "${NET_INFO[SVC_IDX]}"
        view_text
    fi
}

# ---------------------------------------------------------------- journal (read-only)
#
# The helper's change journal (schema 1, written from Phase 2 on):
#   {"schema":1,"services":[{"service":"…","recorded_at":"…","getinfo":"…","dns":"…","search":"…"}]}
# getinfo/dns/search hold raw networksetup text. Parsed here in plain bash: the
# content is never executed or evaluated; values are only compared, printed
# (addresses only after validation) and passed to networksetup as validated argv.
# Results: J_STATE none|ok|invalid, J_REASON, and J_SVC[] J_INFO[] (recorded_at,
# dns and search are checked to be strings but not used by this version).

JSON_WS=$' \t\n\r'

json_fail() {
    if [ -z "$JERR" ]; then JERR="$1"; fi
    return 1
}

json_ws() {
    local ws="${JREST%%[!"$JSON_WS"]*}"
    JREST="${JREST:${#ws}}"
}

# json_expect <char>
json_expect() {
    json_ws
    if [ "${JREST:0:1}" != "$1" ]; then
        json_fail "expected '$1'"
        return 1
    fi
    JREST="${JREST:1}"
}

json_hex4() {
    local h="${JREST:0:4}"
    case "$h" in
        [0-9A-Fa-f][0-9A-Fa-f][0-9A-Fa-f][0-9A-Fa-f]) ;;
        *)
            json_fail "bad \\u escape"
            return 1
            ;;
    esac
    JREST="${JREST:4}"
    JHEX=$((16#$h))
}

# json_utf8 <code point>: sets JUTF to its UTF-8 bytes.
json_utf8() {
    local cp="$1" fmt="" b
    if [ "$cp" -lt 128 ]; then
        set -- "$cp"
    elif [ "$cp" -lt 2048 ]; then
        set -- $((192 | (cp >> 6))) $((128 | (cp & 63)))
    elif [ "$cp" -lt 65536 ]; then
        set -- $((224 | (cp >> 12))) $((128 | ((cp >> 6) & 63))) $((128 | (cp & 63)))
    else
        set -- $((240 | (cp >> 18))) $((128 | ((cp >> 12) & 63))) $((128 | ((cp >> 6) & 63))) $((128 | (cp & 63)))
    fi
    for b in "$@"; do
        fmt="$fmt\\$(((b >> 6) & 7))$(((b >> 3) & 7))$((b & 7))"
    done
    # shellcheck disable=SC2059 # fmt is only backslashes and octal digits built above
    printf -v JUTF "$fmt"
}

json_unicode() {
    local hi lo
    json_hex4 || return 1
    hi=$JHEX
    if [ "$hi" -ge 55296 ] && [ "$hi" -le 56319 ]; then
        if [ "${JREST:0:2}" != '\u' ]; then
            json_fail "unpaired surrogate in a \\u escape"
            return 1
        fi
        JREST="${JREST:2}"
        json_hex4 || return 1
        lo=$JHEX
        if [ "$lo" -lt 56320 ] || [ "$lo" -gt 57343 ]; then
            json_fail "unpaired surrogate in a \\u escape"
            return 1
        fi
        hi=$((65536 + ((hi - 55296) << 10) + (lo - 56320)))
    elif [ "$hi" -ge 56320 ] && [ "$hi" -le 57343 ]; then
        json_fail "unpaired surrogate in a \\u escape"
        return 1
    elif [ "$hi" -eq 0 ]; then
        json_fail "NUL character in a string"
        return 1
    fi
    json_utf8 "$hi"
    JSTR="$JSTR$JUTF"
}

# json_string: JREST starts with a quote; sets JSTR. Consumes whole runs of
# plain characters at a time, so long strings stay fast in bash 3.2.
json_string() {
    local chunk e
    JSTR=""
    JREST="${JREST:1}"
    while :; do
        chunk="${JREST%%[\"\\]*}"
        case "$chunk" in
            *[[:cntrl:]]*)
                json_fail "control character inside a string"
                return 1
                ;;
        esac
        JSTR="$JSTR$chunk"
        JREST="${JREST:${#chunk}}"
        case "${JREST:0:1}" in
            '"')
                JREST="${JREST:1}"
                return 0
                ;;
            \\)
                e="${JREST:1:1}"
                JREST="${JREST:2}"
                case "$e" in
                    '"' | \\ | /) JSTR="$JSTR$e" ;;
                    b) JSTR="$JSTR"$'\b' ;;
                    f) JSTR="$JSTR"$'\f' ;;
                    n) JSTR="$JSTR$NL" ;;
                    r) JSTR="$JSTR"$'\r' ;;
                    t) JSTR="$JSTR$TAB" ;;
                    u) json_unicode || return 1 ;;
                    *)
                        json_fail "unknown escape in a string"
                        return 1
                        ;;
                esac
                ;;
            *)
                json_fail "unterminated string"
                return 1
                ;;
        esac
    done
}

json_number() {
    JNUM="${JREST%%[!0-9eE.+-]*}"
    case "$JNUM" in
        "" | [!0-9-]*)
            json_fail "bad number"
            return 1
            ;;
    esac
    JREST="${JREST:${#JNUM}}"
}

# json_word <true|false|null>
json_word() {
    case "$JREST" in
        "$1"*) JREST="${JREST:${#1}}" ;;
        *)
            json_fail "unexpected word"
            return 1
            ;;
    esac
}

# json_key: a quoted key followed by ':'; sets JKEY.
json_key() {
    json_ws
    if [ "${JREST:0:1}" != '"' ]; then
        json_fail "expected a quoted key"
        return 1
    fi
    json_string || return 1
    JKEY="$JSTR"
    json_expect ':'
}

# json_more <closer>: after a member or element: ',' sets JMORE=1, the closer JMORE=0.
json_more() {
    json_ws
    case "${JREST:0:1}" in
        ,)
            JREST="${JREST:1}"
            JMORE=1
            ;;
        "$1")
            JREST="${JREST:1}"
            JMORE=0
            ;;
        *)
            json_fail "expected ',' or '$1'"
            return 1
            ;;
    esac
}

# json_skip <depth>: parses and discards one value of any type.
json_skip() {
    local d="$1"
    if [ "$d" -gt "$JSON_MAX_DEPTH" ]; then
        json_fail "nested too deeply"
        return 1
    fi
    json_ws
    case "${JREST:0:1}" in
        '"') json_string ;;
        '{')
            JREST="${JREST:1}"
            json_ws
            if [ "${JREST:0:1}" = "}" ]; then
                JREST="${JREST:1}"
                return 0
            fi
            while :; do
                json_key || return 1
                json_skip $((d + 1)) || return 1
                json_more '}' || return 1
                [ "$JMORE" -eq 1 ] || return 0
            done
            ;;
        '[')
            JREST="${JREST:1}"
            json_ws
            if [ "${JREST:0:1}" = "]" ]; then
                JREST="${JREST:1}"
                return 0
            fi
            while :; do
                json_skip $((d + 1)) || return 1
                json_more ']' || return 1
                [ "$JMORE" -eq 1 ] || return 0
            done
            ;;
        t) json_word true ;;
        f) json_word false ;;
        n) json_word null ;;
        - | [0-9]) json_number ;;
        "")
            json_fail "unexpected end of text"
            return 1
            ;;
        *)
            json_fail "unexpected character"
            return 1
            ;;
    esac
}

# journal_entry: one {"service":…,"getinfo":…} object (schema 1). Known keys must
# be strings and appear once; unknown keys are skipped.
journal_entry() {
    local key svc="" info="" seen=" " i
    json_expect '{' || return 1
    json_ws
    if [ "${JREST:0:1}" = "}" ]; then
        JREST="${JREST:1}"
    else
        while :; do
            json_key || return 1
            key="$JKEY"
            case "$key" in
                service | getinfo | recorded_at | dns | search)
                    case "$seen" in
                        *" $key "*)
                            json_fail "\"$key\" appears twice in an entry"
                            return 1
                            ;;
                    esac
                    seen="$seen$key "
                    json_ws
                    if [ "${JREST:0:1}" != '"' ]; then
                        json_fail "\"$key\" is not a string"
                        return 1
                    fi
                    json_string || return 1
                    case "$key" in
                        service) svc="$JSTR" ;;
                        getinfo) info="$JSTR" ;;
                    esac
                    ;;
                *) json_skip 3 || return 1 ;;
            esac
            json_more '}' || return 1
            [ "$JMORE" -eq 1 ] || break
        done
    fi
    if [ -z "$svc" ]; then
        json_fail "an entry has no service name"
        return 1
    fi
    case "$svc" in
        *[[:cntrl:]]*)
            json_fail "a service name contains a control character"
            return 1
            ;;
    esac
    case "$seen" in
        *" getinfo "*) ;;
        *)
            json_fail "an entry has no getinfo text"
            return 1
            ;;
    esac
    i=0
    while [ "$i" -lt ${#J_SVC[@]} ]; do
        if [ "${J_SVC[i]}" = "$svc" ]; then
            json_fail "a service is listed twice"
            return 1
        fi
        i=$((i + 1))
    done
    i=${#J_SVC[@]}
    J_SVC[i]="$svc"
    J_INFO[i]="$info"
}

journal_services() {
    json_expect '[' || return 1
    json_ws
    if [ "${JREST:0:1}" = "]" ]; then
        JREST="${JREST:1}"
        return 0
    fi
    while :; do
        journal_entry || return 1
        json_more ']' || return 1
        [ "$JMORE" -eq 1 ] || return 0
    done
}

# journal_top <1|2>: the top-level object. Pass 1 checks the syntax and reads
# "schema"; pass 2 (schema 1 only) reads "services" strictly.
journal_top() {
    local key s_schema=0 s_services=0
    json_expect '{' || return 1
    json_ws
    if [ "${JREST:0:1}" = "}" ]; then
        JREST="${JREST:1}"
    else
        while :; do
            json_key || return 1
            key="$JKEY"
            case "$key" in
                schema)
                    if [ "$s_schema" -eq 1 ]; then
                        json_fail "\"schema\" appears twice"
                        return 1
                    fi
                    s_schema=1
                    json_ws
                    case "${JREST:0:1}" in
                        - | [0-9])
                            json_number || return 1
                            J_SCHEMA="$JNUM"
                            ;;
                        *)
                            json_skip 1 || return 1
                            J_SCHEMA="(not a number)"
                            ;;
                    esac
                    ;;
                services)
                    if [ "$s_services" -eq 1 ]; then
                        json_fail "\"services\" appears twice"
                        return 1
                    fi
                    s_services=1
                    if [ "$1" = "2" ]; then
                        journal_services || return 1
                    else
                        json_skip 1 || return 1
                    fi
                    ;;
                *) json_skip 1 || return 1 ;;
            esac
            json_more '}' || return 1
            [ "$JMORE" -eq 1 ] || break
        done
    fi
    json_ws
    if [ -n "$JREST" ]; then
        json_fail "unexpected text after the end"
        return 1
    fi
    J_HAS_SCHEMA=$s_schema
    J_HAS_SERVICES=$s_services
}

journal_invalid() {
    J_STATE=invalid
    J_REASON="$1"
    J_SVC=()
    J_INFO=()
}

# journal_parse <text>: sets J_STATE ok|invalid, J_REASON and the J_* arrays.
journal_parse() {
    J_SVC=()
    J_INFO=()
    J_SCHEMA=""
    J_HAS_SCHEMA=0
    J_HAS_SERVICES=0
    JERR=""
    JREST="$1"
    if ! journal_top 1; then
        journal_invalid "not valid JSON: $JERR"
        return 0
    fi
    if [ "$J_HAS_SCHEMA" -ne 1 ]; then
        journal_invalid "no schema number"
        return 0
    fi
    if [ "$J_SCHEMA" != "1" ]; then
        journal_invalid "unknown schema $(one_line "$J_SCHEMA")"
        return 0
    fi
    if [ "$J_HAS_SERVICES" -ne 1 ]; then
        journal_invalid "no services list"
        return 0
    fi
    JERR=""
    JREST="$1"
    if ! journal_top 2; then
        journal_invalid "schema 1 but $JERR"
        return 0
    fi
    J_STATE=ok
    J_REASON=""
}

# load_journal: read-only. Needs LC_ALL=C (byte lengths).
load_journal() {
    local f="$R$JOURNAL_FILE" size text
    J_STATE=none
    J_REASON=""
    J_SVC=()
    J_INFO=()
    exists "$f" || return 0
    if [ -L "$f" ] || [ ! -f "$f" ]; then
        journal_invalid "not a regular file"
        return 0
    fi
    size=$(stat -f '%z' "$f" 2>/dev/null)
    case "$size" in
        "" | *[!0-9]*)
            journal_invalid "cannot be read"
            return 0
            ;;
    esac
    if [ "$size" -gt "$JOURNAL_MAX_BYTES" ]; then
        journal_invalid "larger than 64 KB"
        return 0
    fi
    if [ ! -r "$f" ] || ! text=$(cat -- "$f" 2>/dev/null && printf x); then
        journal_invalid "cannot be read"
        return 0
    fi
    text="${text%x}"
    if [ "${#text}" -ne "$size" ]; then
        journal_invalid "contains NUL bytes"
        return 0
    fi
    journal_parse "$text"
}

# ---------------------------------------------------------------- network choices

# add_action <kind> <service> <detail> [ip mask router]: kind is setdhcp,
# setmanual (writes), unchanged or skip.
add_action() {
    local n=${#NA_KIND[@]}
    NA_KIND[n]="$1"
    NA_SVC[n]="$2"
    NA_DETAIL[n]="$3"
    NA_IP[n]="${4:-}"
    NA_MASK[n]="${5:-}"
    NA_ROUTER[n]="${6:-}"
    case "$1" in
        setdhcp | setmanual) NET_WORK=1 ;;
    esac
}

is_picked() {
    local i=0
    while [ "$i" -lt ${#PICKS[@]} ]; do
        if [ "${PICKS[i]}" = "$1" ]; then
            return 0
        fi
        i=$((i + 1))
    done
    return 1
}

# plan_restore <journal index>: one restore action for a journaled service.
plan_restore() {
    local j="$1" svc om oip omask orouter
    svc="${J_SVC[j]}"
    info_view "${J_INFO[j]}"
    om="$IV_MODE"
    oip="$IV_IP"
    omask="$IV_MASK"
    orouter="$IV_ROUTER"
    if ! svc_index "$svc"; then
        add_action skip "$svc" "service no longer exists: not restored"
        return 0
    fi
    case "$om" in
        dhcp) ;;
        manual)
            if ! is_ipv4 "$oip" || ! is_mask "$omask" || { [ "$orouter" != "-" ] && ! is_ipv4 "$orouter"; }; then
                add_action skip "$svc" "recorded address, mask or router is not valid IPv4: not restored"
                return 0
            fi
            ;;
        *)
            add_action skip "$svc" "recorded settings are neither DHCP nor Manual: not restored"
            return 0
            ;;
    esac
    if [ "${NET_RC[SVC_IDX]}" -eq 0 ] && ! ns_error_text "${NET_INFO[SVC_IDX]}"; then
        info_view "${NET_INFO[SVC_IDX]}"
        if [ "$om" = "dhcp" ] && [ "$IV_MODE" = "dhcp" ]; then
            add_action unchanged "$svc" "already matches the recorded original"
            return 0
        fi
        if [ "$om" = "manual" ] && [ "$IV_MODE" = "manual" ] && [ "$IV_IP" = "$oip" ] &&
            [ "$IV_MASK" = "$omask" ] && [ "$IV_ROUTER" = "$orouter" ]; then
            add_action unchanged "$svc" "already matches the recorded original"
            return 0
        fi
    fi
    if [ "$om" = "dhcp" ]; then
        add_action setdhcp "$svc" "source=journal"
    else
        add_action setmanual "$svc" "ip=$oip mask=$omask router=$orouter source=journal" "$oip" "$omask" "$orouter"
    fi
}

# resolve_network: checks the choices against the current services and the
# journal (refusing with exit 2 before anything is changed) and builds the
# ordered action list: dhcp: picks first, then the journal in journal order.
resolve_network() {
    local i j svc
    NA_KIND=()
    NA_SVC=()
    NA_DETAIL=()
    NA_IP=()
    NA_MASK=()
    NA_ROUTER=()
    J_SEL=()
    OVERLAP=()
    NET_WORK=0
    if { [ ${#PICKS[@]} -gt 0 ] || [ "$NET_RESTORE" -eq 1 ]; } && [ -n "$NET_ERR" ]; then
        refuse "cannot check the --network choices: $NET_ERR"
    fi
    i=0
    while [ "$i" -lt ${#PICKS[@]} ]; do
        svc="${PICKS[i]}"
        if ! svc_index "$svc"; then
            refuse "--network dhcp:$svc: there is no network service named exactly \"$svc\" (see the NETWORK lines of --plan). Nothing has been changed."
        fi
        if [ "${NET_EN[SVC_IDX]}" -ne 1 ]; then
            refuse "--network dhcp:$svc: that service is disabled; only enabled services can be switched. Nothing has been changed."
        fi
        add_action setdhcp "$svc" "source=pick"
        i=$((i + 1))
    done
    if [ "$NET_RESTORE" -eq 1 ]; then
        case "$J_STATE" in
            none) refuse "--network restore-journal: no journal was found, so nothing was recorded to restore (pick services with --network dhcp:\"<service>\" instead). Nothing has been changed." ;;
            invalid) refuse "--network restore-journal: the journal could not be used ($J_REASON). Nothing has been changed." ;;
        esac
    fi
    j=0
    while [ "$j" -lt ${#J_SVC[@]} ]; do
        J_SEL[j]=no
        if [ "$NET_RESTORE" -eq 1 ]; then
            svc="${J_SVC[j]}"
            if is_picked "$svc"; then
                OVERLAP[${#OVERLAP[@]}]="$svc"
            else
                J_SEL[j]=yes
                plan_restore "$j"
            fi
        fi
        j=$((j + 1))
    done
    HAS_WORK=$FILES_WORK
    if [ "$NET_WORK" -eq 1 ]; then
        HAS_WORK=1
    fi
}

# ---------------------------------------------------------------- repair choices

# value_list <networksetup -getdnsservers|-getsearchdomains text>: the values,
# one per line in the input, joined with "," for plan records ("-" for none).
value_list() {
    local line v out=""
    while IFS= read -r line; do
        v=$(trim "$(one_line "$line")")
        [ -n "$v" ] || continue
        case "$v" in
            "There aren't any "*) continue ;;
        esac
        if [ -n "$out" ]; then out="$out,"; fi
        out="$out$v"
    done <<<"$1"
    printf '%s' "${out:--}"
}

# is_bsd_device <name>: a plain device name like en7 or bridge0 (it goes to
# ipconfig as one argv element; anything else is treated as "no device").
is_bsd_device() {
    case "$1" in
        "" | *[!A-Za-z0-9]*) return 1 ;;
    esac
    [ ${#1} -le 15 ]
}

# read_devices: read-only. SO_NAMES[]/SO_DEVS[] from -listnetworkserviceorder
# (each service's own device, also for renamed services: "(n) Name" or
# "(*) Name", then "(Hardware Port: X, Device: enN)"; an empty device, e.g. a
# VPN, stays empty). HW_DEVS: the "Device:" names from -listallhardwareports,
# one per line between newlines. TODO(verify) on the travel Mac: that
# -listallhardwareports lists only attached hardware (on the Mac Studio the
# unplugged USB adapters' devices are missing from it). If it is wrong, a
# service is skipped with a note, never renewed blindly. DEV_ERR on failure.
read_devices() {
    local out rc line name="" dev have=0
    SO_NAMES=()
    SO_DEVS=()
    HW_DEVS="$NL"
    DEV_ERR=""
    out=$("$NS" -listnetworkserviceorder 2>&1 </dev/null)
    rc=$?
    if [ "$rc" -ne 0 ] || ns_error_text "$out"; then
        DEV_ERR="could not read the service order (exit $rc): $(one_line "$out")"
        return 1
    fi
    while IFS= read -r line; do
        case "$line" in
            "(Hardware Port: "*)
                [ "$have" -eq 1 ] || continue
                dev=""
                case "$line" in
                    *", Device: "*")")
                        dev="${line##*, Device: }"
                        dev=$(trim "${dev%)}")
                        ;;
                esac
                SO_NAMES[${#SO_NAMES[@]}]="$name"
                SO_DEVS[${#SO_DEVS[@]}]="$dev"
                have=0
                ;;
            "("*") "*)
                name="${line#*) }"
                name="${name#\*}"
                have=1
                ;;
        esac
    done <<<"$out"
    out=$("$NS" -listallhardwareports 2>&1 </dev/null)
    rc=$?
    if [ "$rc" -ne 0 ] || ns_error_text "$out"; then
        DEV_ERR="could not list the network hardware (exit $rc): $(one_line "$out")"
        return 1
    fi
    while IFS= read -r line; do
        case "$line" in
            "Device: "*)
                dev=$(trim "${line#Device: }")
                if [ -n "$dev" ]; then HW_DEVS="$HW_DEVS$dev$NL"; fi
                ;;
        esac
    done <<<"$out"
    return 0
}

# device_of <service>: prints its device from SO_* (empty when unknown).
device_of() {
    local i=0
    while [ "$i" -lt ${#SO_NAMES[@]} ]; do
        if [ "${SO_NAMES[i]}" = "$1" ]; then
            printf '%s' "${SO_DEVS[i]}"
            return 0
        fi
        i=$((i + 1))
    done
}

# add_repair <kind> <subject> <detail> [device]
add_repair() {
    local n=${#RA_KIND[@]}
    RA_KIND[n]="$1"
    RA_SVC[n]="$2"
    RA_DETAIL[n]="$3"
    RA_DEV[n]="${4:-}"
}

# resolve_soft: flush + HUP, then one renew per enabled DHCP service with an
# attached device; every other service gets a "leave" record with the reason.
resolve_soft() {
    local i svc dev renewed="$NL" why
    add_repair flushcache dns-cache "dscacheutil -flushcache"
    add_repair hup mDNSResponder "killall -HUP mDNSResponder"
    read_devices
    i=0
    while [ "$i" -lt ${#NET_SVC[@]} ]; do
        svc="${NET_SVC[i]}"
        why=""
        if [ "${NET_EN[i]}" -ne 1 ]; then
            why="disabled: not touched"
        elif [ "${NET_RC[i]}" -ne 0 ] || ns_error_text "${NET_INFO[i]}"; then
            why="settings could not be read: not touched"
        else
            info_view "${NET_INFO[i]}"
            case "$IV_MODE" in
                manual) why="manual (static) settings: not touched" ;;
                unknown) why="neither DHCP nor Manual settings (for example a VPN): not touched" ;;
            esac
        fi
        if [ -z "$why" ]; then
            if [ -n "$DEV_ERR" ]; then
                why="$DEV_ERR: not renewed"
            else
                dev=$(device_of "$svc")
                if ! is_bsd_device "$dev"; then
                    why="no network device (for example a VPN): not renewed"
                else
                    case "$HW_DEVS" in
                        *"$NL$dev$NL"*)
                            case "$renewed" in
                                *"$NL$dev$NL"*) why="device $dev is already renewed for another service" ;;
                            esac
                            ;;
                        *) why="device $dev is not attached: not renewed" ;;
                    esac
                fi
            fi
        fi
        if [ -n "$why" ]; then
            add_repair leave "$svc" "$why"
        else
            add_repair renew "$svc" "device=$dev now=$(view_text)" "$dev"
            renewed="$renewed$dev$NL"
        fi
        i=$((i + 1))
    done
}

# resolve_service: refuses (exit 2, nothing changed) unless REPAIR_SVC is an
# enabled listed service on DHCP or Manual settings whose DNS servers and
# search domains can be read (they are backed up and shown first).
resolve_service() {
    local svc="$REPAIR_SVC" rc
    if ! svc_index "$svc"; then
        refuse "--repair service:$svc: there is no network service named exactly \"$svc\" (see -listallnetworkservices). Nothing has been changed."
    fi
    if [ "${NET_EN[SVC_IDX]}" -ne 1 ]; then
        refuse "--repair service:$svc: that service is disabled; only enabled services can be reset. Nothing has been changed."
    fi
    if [ "${NET_RC[SVC_IDX]}" -ne 0 ] || ns_error_text "${NET_INFO[SVC_IDX]}"; then
        refuse "--repair service:$svc: its settings could not be read: $(one_line "${NET_INFO[SVC_IDX]}"). Nothing has been changed."
    fi
    info_view "${NET_INFO[SVC_IDX]}"
    if [ "$IV_MODE" = "unknown" ]; then
        refuse "--repair service:$svc: it has neither DHCP nor Manual settings (for example a VPN), so it cannot be reset here. Nothing has been changed."
    fi
    SR_NOW=$(view_text)
    SR_DNS_TEXT=$("$NS" -getdnsservers "$svc" 2>&1 </dev/null)
    rc=$?
    if [ "$rc" -ne 0 ] || ns_error_text "$SR_DNS_TEXT"; then
        refuse "--repair service:$svc: its DNS servers could not be read (exit $rc: $(one_line "$SR_DNS_TEXT")), so they cannot be backed up. Nothing has been changed."
    fi
    SR_SEARCH_TEXT=$("$NS" -getsearchdomains "$svc" 2>&1 </dev/null)
    rc=$?
    if [ "$rc" -ne 0 ] || ns_error_text "$SR_SEARCH_TEXT"; then
        refuse "--repair service:$svc: its search domains could not be read (exit $rc: $(one_line "$SR_SEARCH_TEXT")), so they cannot be backed up. Nothing has been changed."
    fi
    SR_DNS=$(value_list "$SR_DNS_TEXT")
    SR_SEARCH=$(value_list "$SR_SEARCH_TEXT")
    add_repair current "$svc" "now=$SR_NOW dns=$SR_DNS search=$SR_SEARCH"
    add_repair setdhcp "$svc" "was=$SR_NOW"
    add_repair cleardns "$svc" "was=$SR_DNS"
    add_repair clearsearch "$svc" "was=$SR_SEARCH"
    add_repair disable "$svc" -
    add_repair enable "$svc" -
}

# resolve_repair: the one path for the flags and the menu. Reads REPAIR_TIER
# and REPAIR_SVC; refuses (exit 2) before anything is changed.
resolve_repair() {
    RA_KIND=()
    RA_SVC=()
    RA_DETAIL=()
    RA_DEV=()
    HAS_WORK=1
    if [ -n "$NET_ERR" ]; then
        refuse "cannot plan the repair: $NET_ERR. Nothing has been changed."
    fi
    case "$REPAIR_TIER" in
        soft) resolve_soft ;;
        service) resolve_service ;;
        *) refuse "no repair chosen" ;;
    esac
}

# ---------------------------------------------------------------- state

is_inventory_or_inside() {
    local a="$1"
    while [ -n "$a" ]; do
        case "$INV_LIST" in
            *"$NL$a$NL"*) return 0 ;;
        esac
        a="${a%/*}"
    done
    return 1
}

# search_tree <display dir> <max depth> <sys|user>: prints matching display paths.
# Matches are not descended into. In a user's Library, iCloud Drive and the
# Claude CLI cache are skipped, and other apps' containers are matched by name
# but never opened (opening them can trigger macOS privacy prompts).
search_tree() {
    local base="$R$1" rel
    [ -d "$base" ] || return 0
    [ -L "$base" ] && return 0
    (
        cd -- "$base" 2>/dev/null || exit 0
        if [ "$3" = "user" ]; then
            find . -maxdepth "$2" \
                \( -path './Mobile Documents' -o -path './Caches/claude-cli-nodejs' \) -prune -o \
                \( -iname '*ithurtswhenip*' -o -iname '*ipswitcher*' -o -iname '*it-hurts-when-ip*' \
                -o -iname '*It Hurts When IP*' -o -iname '*IP Switcher*' \) -print -prune -o \
                \( -path './Containers/*' -o -path './Group Containers/*' \) -prune 2>/dev/null
        else
            find . -maxdepth "$2" \
                \( -iname '*ithurtswhenip*' -o -iname '*ipswitcher*' -o -iname '*it-hurts-when-ip*' \
                -o -iname '*It Hurts When IP*' -o -iname '*IP Switcher*' \) -print -prune 2>/dev/null
        fi
    ) | while IFS= read -r rel; do
        [ "$rel" = "." ] && continue
        printf '%s\n' "$1${rel#.}"
    done
}

find_leftovers() {
    local i found
    {
        search_tree "/Library" 3 sys
        search_tree "/Applications" 1 sys
        search_tree "/var/run" 2 sys
        search_tree "/var/log" 2 sys
        i=0
        while [ "$i" -lt ${#USERS[@]} ]; do
            search_tree "/Users/${USERS[i]}/Library" 3 user
            i=$((i + 1))
        done
    } | LC_ALL=C sort -u | while IFS= read -r found; do
        [ -n "$found" ] || continue
        is_inventory_or_inside "$found" && continue
        printf '%s\n' "$found"
    done
}

owned_ok() {
    local o
    o=$(stat -f '%u' "$1" 2>/dev/null) || return 1
    [ "$o" = "0" ] || [ "$o" = "$EUID_NOW" ]
}

# choose_backup_base: read-only. /Users/Shared is world-writable, so a
# pre-existing ItHurtsWhenIP-backups that is a symlink, not a folder, or owned
# by someone else is never used.
choose_backup_base() {
    local std="$BACKUP_PARENT/$BACKUP_NAME" full
    full="$R$std"
    BACKUP_NOTE=""
    if [ -L "$full" ] || { [ -e "$full" ] && [ ! -d "$full" ]; } || { [ -d "$full" ] && ! owned_ok "$full"; }; then
        BACKUP_BASE="$std.$TS"
        BACKUP_NOTE="$std is a symlink, not a folder, or not owned by root: using $BACKUP_BASE instead"
    else
        BACKUP_BASE="$std"
    fi
    BACKUP_DIR_DISPLAY="$BACKUP_BASE/$TS"
}

# collect_state: read-only. FILES_WORK = something to stop, quit, remove or
# forget; HAS_WORK becomes 1 too when resolve_network plans a network write.
collect_state() {
    local i n label id
    FILES_WORK=0
    n=${#ITEM_PATH[@]}
    i=0
    while [ "$i" -lt "$n" ]; do
        if exists "$R${ITEM_PATH[i]}"; then
            ITEM_EXISTS[i]=1
            FILES_WORK=1
            set_kind "$i"
            if ! parent_ok "${ITEM_PATH[i]}" "${ITEM_ANCHOR[i]}"; then
                ITEM_UNSAFE[i]="$PARENT_REASON"
            fi
        fi
        i=$((i + 1))
    done
    PLANNED_LABELS=""
    for label in $LABELS; do
        if label_loaded "$label"; then
            PLANNED_LABELS="$PLANNED_LABELS $label "
            FILES_WORK=1
        fi
    done
    read_receipts
    PLANNED_RECEIPTS=""
    for id in $RECEIPTS; do
        if receipt_present "$id"; then
            PLANNED_RECEIPTS="$PLANNED_RECEIPTS $id "
            FILES_WORK=1
        fi
    done
    APP_RUNNING=0
    if app_running; then
        APP_RUNNING=1
        FILES_WORK=1
    fi
    HAS_WORK=$FILES_WORK
    read_network
    load_journal
    LEFTOVERS=$(find_leftovers)
    choose_backup_base
    WAIT_START=""
    if [ -n "$WAIT_PID" ]; then
        WAIT_START=$(pid_start "$WAIT_PID")
    fi
}

# ---------------------------------------------------------------- plan

rec() {
    printf '%s\t%s\t%s\t%s\n' "$1" "$2" "$3" "$4"
}

# render_plan <plan|uninstall>: pure; prints the header and one record per line.
#
# Record contract (the app parses these): every record has exactly 4
# tab-separated fields, TYPE KIND SUBJECT DETAIL, in this order:
#   NOTE     info       -|backup-folder|network|<service>|<path>  <free text>
#   BACKUP   snapshot   network     network-before.txt
#   BACKUP   file|dir   <path>      <path inside the backup folder>
#   NETWORK  manual     <service>   ip=<ip> mask=<mask> router=<router|-> selected=yes|no
#            enabled services on Manual settings; selected=yes: named with --network dhcp:
#   NOTE     info       journal     found: <N> service(s) recorded; ... | not used (<reason>); ...
#   NETWORK  journal    <service>   original=<m>/<ip>/<mask>/<router> current=<m>/<ip>/<mask>/<router> selected=yes|no
#            m = dhcp|manual|unknown (|missing|unreadable for current); "-" = none;
#            "invalid" = not IPv4. selected=yes: restore-journal chosen and not
#            overridden by a dhcp: pick for the same service
#   NOTE     info       <service>   picked for DHCP and also in the journal: the DHCP pick wins
#   NETWORK  setdhcp    <service>   source=pick|journal                       (a planned write)
#   NETWORK  setmanual  <service>   ip=<ip> mask=<mask> router=<router|-> source=journal (a planned write)
#   NETWORK  unchanged  <service>   already matches the recorded original    (no write)
#   NETWORK  skip       <service>   <reason>                                 (not restored: partial)
#   NETWORK  choice     -           leave|restore-journal|dhcp|restore-journal+dhcp
#   NOTE     info       wait-pid    <how the two stages run>
#   STOP     launchd    <label>     loaded
#   QUIT     app        <process>   running
#   REMOVE   file|dir|socket|app  <path>  -|symlink: only the link is removed
#   FORGET   receipt    <id>        -
#   NOTE     info       <path>      skipped: <reason>
#   LEFTOVER path       <path>      reported only, not removed
# Execution lines: OK <TYPE> <KIND> <SUBJECT> <DETAIL>; FAILED <TYPE> <KIND>
# <SUBJECT> <DETAIL> <reason>; VERIFY ok|FAILED <subject> (network:<service>
# lines add a 4th field: now=<m>/<ip>/<mask>/<router>[ note]).
#
# Repair plans (--repair, printed by render_repair_plan) use the same header
# plus "# repair: soft|service:<name>", the same 4 fields, and these records:
#   REPAIR   tier         soft|service   -|<service>
#   NOTE     info         backup-folder  <why the usual backup folder is not used>
#   BACKUP   snapshot     network        network-before.txt
#   BACKUP   snapshot     <service>      service-before.txt   (service: its getinfo, DNS, search)
#   soft:
#   REPAIR   flushcache   dns-cache      dscacheutil -flushcache
#   REPAIR   hup          mDNSResponder  killall -HUP mDNSResponder
#   REPAIR   renew        <service>      device=<bsd device> now=<m>/<ip>/<mask>/<router>
#                                        (a planned "ipconfig set <device> DHCP")
#   REPAIR   leave        <service>      <reason>: not touched | not renewed  (no write; not a failure)
#   service:
#   REPAIR   current      <service>      now=<m>/<ip>/<mask>/<router> dns=<a,b|-> search=<a,b|->
#   REPAIR   setdhcp      <service>      was=<m>/<ip>/<mask>/<router>
#   REPAIR   cleardns     <service>      was=<a,b|->
#   REPAIR   clearsearch  <service>      was=<a,b|->
#   REPAIR   disable      <service>      -
#   REPAIR   enable       <service>      -
#   "# summary: backup=<n> repair=<n> leave=<n>" (repair = planned writes)
# Repair execution lines: OK|FAILED REPAIR <kind> <subject> <detail>[ <reason>]
# as above, then one line per renewed or reset service:
#   VERIFY   ok|warn|FAILED   repair:<service>   now=<m>/<ip>/<mask>/<router>[ dns=<list> search=<list> enabled=yes|no][; <note>]
#   warn = the settings are right but there is no DHCP address yet (none, or
#   link-local 169.254.x.x): reported, not a failure (exit 0 if nothing failed).
# The result ends with "# failed steps: <n>[; failed checks: <n>]" and
# "# warnings: <n>", each followed by the lines it counts.
render_plan() {
    local i j k n p d label id sel cb=0 cn=0 cs=0 cq=0 cr=0 cf=0 cl=0 cx=0
    printf '# It Hurts When IP rescue script, version %s\n' "$RESCUE_VERSION"
    printf '# mode: %s\n' "$1"
    if [ "$TEST_MODE" -eq 1 ]; then
        printf '# test root: %s\n' "$R"
    fi
    if [ "$HAS_WORK" -eq 1 ]; then
        printf '# backup folder: %s\n' "$BACKUP_DIR_DISPLAY"
    else
        printf '# backup folder: none needed\n'
    fi
    if [ "$1" = "plan" ]; then
        printf '# plan only: nothing has been changed\n'
    else
        printf '# nothing has been changed yet\n'
    fi

    if [ "$FILES_WORK" -eq 0 ]; then
        rec NOTE info - "nothing to remove"
    fi

    n=${#ITEM_PATH[@]}
    if [ "$HAS_WORK" -eq 1 ]; then
        if [ -n "$BACKUP_NOTE" ]; then
            rec NOTE info backup-folder "$BACKUP_NOTE"
        fi
        rec BACKUP snapshot network network-before.txt
        cb=$((cb + 1))
        i=0
        while [ "$i" -lt "$n" ]; do
            if [ "${ITEM_EXISTS[i]}" = 1 ] && [ "${ITEM_BACKUP[i]}" = y ] && [ -z "${ITEM_UNSAFE[i]}" ]; then
                rec BACKUP "${ITEM_KIND[i]}" "${ITEM_PATH[i]}" "${ITEM_BREL[i]}"
                cb=$((cb + 1))
            fi
            i=$((i + 1))
        done
    fi

    if [ -n "$NET_ERR" ]; then
        rec NOTE info network "$NET_ERR"
    else
        i=0
        while [ "$i" -lt ${#NET_SVC[@]} ]; do
            if [ "${NET_RC[i]}" -ne 0 ] || ns_error_text "${NET_INFO[i]}"; then
                rec NOTE info "${NET_SVC[i]}" "could not read its settings: $(one_line "${NET_INFO[i]}")"
            else
                parse_info "${NET_INFO[i]}"
                if [ "$INFO_MANUAL" -eq 1 ] && [ "${NET_EN[i]}" -eq 1 ]; then
                    sel=no
                    if is_picked "${NET_SVC[i]}"; then sel=yes; fi
                    rec NETWORK manual "${NET_SVC[i]}" "ip=$INFO_IP mask=$INFO_MASK router=$INFO_ROUTER selected=$sel"
                    cn=$((cn + 1))
                fi
            fi
            i=$((i + 1))
        done
    fi
    case "$J_STATE" in
        ok)
            rec NOTE info journal "found: ${#J_SVC[@]} service(s) recorded; restore covers IPv4 settings only (DNS servers and search domains are not restored by this version)"
            j=0
            while [ "$j" -lt ${#J_SVC[@]} ]; do
                info_view "${J_INFO[j]}"
                p=$(view_text)
                rec NETWORK journal "${J_SVC[j]}" "original=$p current=$(current_text "${J_SVC[j]}") selected=${J_SEL[j]}"
                j=$((j + 1))
            done
            ;;
        invalid)
            rec NOTE info journal "not used ($J_REASON); it is backed up with the helper folder, and services on Manual settings are listed instead"
            ;;
    esac
    k=0
    while [ "$k" -lt ${#OVERLAP[@]} ]; do
        rec NOTE info "${OVERLAP[k]}" "picked for DHCP and also in the journal: the DHCP pick wins"
        k=$((k + 1))
    done
    k=0
    while [ "$k" -lt ${#NA_KIND[@]} ]; do
        rec NETWORK "${NA_KIND[k]}" "${NA_SVC[k]}" "${NA_DETAIL[k]}"
        k=$((k + 1))
    done
    rec NETWORK choice - "$NET_LABEL"

    if [ -n "$WAIT_PID" ] && [ "$FILES_WORK" -eq 1 ]; then
        if [ -n "$WAIT_START" ]; then
            rec NOTE info wait-pid "two stages: stage B waits for process $WAIT_PID to exit (up to $WAIT_TIMEOUT s), then removes the app bundle and per-user data"
        else
            rec NOTE info wait-pid "process $WAIT_PID is not running, so stage B removes the app bundle and per-user data straight after stage A"
        fi
    fi

    for label in $LABELS; do
        case "$PLANNED_LABELS" in
            *" $label "*)
                rec STOP launchd "$label" loaded
                cs=$((cs + 1))
                ;;
        esac
    done
    if [ "$APP_RUNNING" -eq 1 ]; then
        rec QUIT app "$APP_PROC" running
        cq=$((cq + 1))
    fi
    i=0
    while [ "$i" -lt "$n" ]; do
        if [ "${ITEM_EXISTS[i]}" = 1 ] && [ -z "${ITEM_UNSAFE[i]}" ]; then
            d="-"
            if [ "${ITEM_LINK[i]}" = 1 ]; then
                d="symlink: only the link is removed"
            fi
            rec REMOVE "${ITEM_KIND[i]}" "${ITEM_PATH[i]}" "$d"
            cr=$((cr + 1))
        fi
        i=$((i + 1))
    done
    for id in $RECEIPTS; do
        case "$PLANNED_RECEIPTS" in
            *" $id "*)
                rec FORGET receipt "$id" -
                cf=$((cf + 1))
                ;;
        esac
    done
    i=0
    while [ "$i" -lt "$n" ]; do
        if [ "${ITEM_EXISTS[i]}" = 1 ] && [ -n "${ITEM_UNSAFE[i]}" ]; then
            rec NOTE info "${ITEM_PATH[i]}" "skipped: ${ITEM_UNSAFE[i]}"
            cx=$((cx + 1))
        fi
        i=$((i + 1))
    done
    while IFS= read -r p; do
        [ -n "$p" ] || continue
        rec LEFTOVER path "$p" "reported only, not removed"
        cl=$((cl + 1))
    done <<<"$LEFTOVERS"
    printf '# summary: backup=%s network-manual=%s stop=%s quit=%s remove=%s forget=%s skipped=%s leftover=%s\n' \
        "$cb" "$cn" "$cs" "$cq" "$cr" "$cf" "$cx" "$cl"
}

# render_repair_plan <plan|repair>: pure; the repair plan (records above).
render_repair_plan() {
    local k cb=0 cr=0 cl=0 subj
    printf '# It Hurts When IP rescue script, version %s\n' "$RESCUE_VERSION"
    printf '# mode: %s\n' "$1"
    if [ "$REPAIR_TIER" = "service" ]; then
        printf '# repair: service:%s\n' "$REPAIR_SVC"
    else
        printf '# repair: %s\n' "$REPAIR_TIER"
    fi
    if [ "$TEST_MODE" -eq 1 ]; then
        printf '# test root: %s\n' "$R"
    fi
    printf '# backup folder: %s\n' "$BACKUP_DIR_DISPLAY"
    if [ "$1" = "plan" ]; then
        printf '# plan only: nothing has been changed\n'
    else
        printf '# nothing has been changed yet\n'
    fi
    subj=-
    if [ "$REPAIR_TIER" = "service" ]; then subj="$REPAIR_SVC"; fi
    rec REPAIR tier "$REPAIR_TIER" "$subj"
    if [ -n "$BACKUP_NOTE" ]; then
        rec NOTE info backup-folder "$BACKUP_NOTE"
    fi
    rec BACKUP snapshot network network-before.txt
    cb=$((cb + 1))
    if [ "$REPAIR_TIER" = "service" ]; then
        rec BACKUP snapshot "$REPAIR_SVC" service-before.txt
        cb=$((cb + 1))
    fi
    k=0
    while [ "$k" -lt ${#RA_KIND[@]} ]; do
        rec REPAIR "${RA_KIND[k]}" "${RA_SVC[k]}" "${RA_DETAIL[k]}"
        case "${RA_KIND[k]}" in
            leave) cl=$((cl + 1)) ;;
            current) ;;
            *) cr=$((cr + 1)) ;;
        esac
        k=$((k + 1))
    done
    printf '# summary: backup=%s repair=%s leave=%s\n' "$cb" "$cr" "$cl"
}

# ---------------------------------------------------------------- execution

FAIL_COUNT=0
VFAIL_COUNT=0
FAIL_LIST=""
BACKUP_DIR=""
APP_STILL_RUNNING=0

step_ok() {
    say "OK${TAB}$1${TAB}$2${TAB}$3${TAB}$4"
}

step_fail() {
    local line="FAILED${TAB}$1${TAB}$2${TAB}$3${TAB}$4${TAB}$5"
    say "$line"
    FAIL_COUNT=$((FAIL_COUNT + 1))
    FAIL_LIST="$FAIL_LIST$line$NL"
}

# verify_ok <subject> [detail]
verify_ok() {
    if [ $# -ge 2 ]; then
        say "VERIFY${TAB}ok${TAB}$1${TAB}$2"
    else
        say "VERIFY${TAB}ok${TAB}$1"
    fi
}

# verify_fail <subject> [detail]
verify_fail() {
    local line="VERIFY${TAB}FAILED${TAB}$1"
    if [ $# -ge 2 ]; then
        line="$line${TAB}$2"
    fi
    say "$line"
    VFAIL_COUNT=$((VFAIL_COUNT + 1))
    FAIL_LIST="$FAIL_LIST$line$NL"
}

# confirm <word>: the user must type <word> exactly (UNINSTALL, REPAIR).
confirm() {
    local word="$1" answer=""
    printf 'Type %s to continue: ' "$word"
    IFS= read -r answer || answer=""
    if [ "$answer" != "$word" ]; then
        printf '\n'
        refuse "cancelled: nothing has been changed"
    fi
    log_line "# confirmed by typing $word"
}

# create_backup_dir: plain mkdir only (never -p) under /Users/Shared, with the
# folder checks repeated right before use. Sets BACKUP_DIR; returns 1 on failure.
create_backup_dir() {
    local parent_full="$R$BACKUP_PARENT" base_full dir_full pres ures
    choose_backup_base
    if [ -n "$BACKUP_NOTE" ]; then
        say "NOTE${TAB}info${TAB}backup-folder${TAB}$BACKUP_NOTE"
    fi
    BACKUP_FAIL_REASON=""
    pres=$(cd -P -- "$parent_full" 2>/dev/null && pwd -P)
    ures=$(cd -P -- "$R/Users" 2>/dev/null && pwd -P)
    if [ -z "$pres" ] || [ -z "$ures" ] || [ "$pres" != "$ures/Shared" ]; then
        BACKUP_FAIL_REASON="$BACKUP_PARENT is missing or resolves outside expected location"
        return 1
    fi
    base_full="$R$BACKUP_BASE"
    if ! exists "$base_full"; then
        if ! mkdir -- "$base_full" 2>/dev/null; then
            BACKUP_FAIL_REASON="could not create $BACKUP_BASE"
            return 1
        fi
        chmod 755 "$base_full" 2>/dev/null
    fi
    if [ -L "$base_full" ] || [ ! -d "$base_full" ] || ! owned_ok "$base_full"; then
        BACKUP_FAIL_REASON="$BACKUP_BASE is a symlink, not a folder, or not owned by root"
        return 1
    fi
    dir_full="$base_full/$TS"
    if ! mkdir -- "$dir_full" 2>/dev/null; then
        dir_full="$base_full/$TS-2"
        if ! mkdir -- "$dir_full" 2>/dev/null; then
            BACKUP_FAIL_REASON="could not create $BACKUP_BASE/$TS or $BACKUP_BASE/$TS-2"
            return 1
        fi
    fi
    BACKUP_DIR="$dir_full"
    BACKUP_DIR_DISPLAY="${dir_full#"$R"}"
    return 0
}

do_snapshot() {
    local f="$BACKUP_DIR/network-before.txt" i
    read_network
    if ! {
        printf '# network-before.txt: read-only snapshot by %s version %s, %s\n' "$SCRIPT_NAME" "$RESCUE_VERSION" "$TS"
        printf '\n$ networksetup -listallnetworkservices\n%s\n(exit %s)\n' "$NET_LIST_TEXT" "$NET_LIST_RC"
        i=0
        while [ "$i" -lt ${#NET_SVC[@]} ]; do
            printf '\n$ networksetup -getinfo "%s"\n%s\n(exit %s)\n' "${NET_SVC[i]}" "${NET_INFO[i]}" "${NET_RC[i]}"
            i=$((i + 1))
        done
    } >"$f" 2>/dev/null; then
        step_fail BACKUP snapshot network network-before.txt "could not write the snapshot"
    elif [ -n "$NET_ERR" ]; then
        step_fail BACKUP snapshot network network-before.txt "$NET_ERR"
    else
        step_ok BACKUP snapshot network network-before.txt
    fi
}

backup_one() {
    local i="$1" p src dest err rc
    p="${ITEM_PATH[i]}"
    src="$R$p"
    dest="$BACKUP_DIR/${ITEM_BREL[i]}"
    if ! exists "$src"; then
        return 0
    fi
    if ! parent_ok "$p" "${ITEM_ANCHOR[i]}"; then
        ITEM_UNSAFE[i]="$PARENT_REASON"
        return 0
    fi
    # -P: a symlinked item is copied as the link itself, never followed.
    err=$({ mkdir -p -- "${dest%/*}" && cp -RPp -- "$src" "$dest"; } 2>&1)
    rc=$?
    if [ "$rc" -ne 0 ] || ! exists "$dest"; then
        ITEM_SKIP[i]="not removed because its backup failed"
        step_fail BACKUP "${ITEM_KIND[i]}" "$p" "${ITEM_BREL[i]}" "backup failed: $(one_line "${err:-copy is missing}")"
        return 0
    fi
    step_ok BACKUP "${ITEM_KIND[i]}" "$p" "${ITEM_BREL[i]}"
}

do_backups() {
    local i=0 n=${#ITEM_PATH[@]}
    while [ "$i" -lt "$n" ]; do
        if [ "${ITEM_EXISTS[i]}" = 1 ] && [ "${ITEM_BACKUP[i]}" = y ] && [ -z "${ITEM_UNSAFE[i]}" ]; then
            backup_one "$i"
        fi
        i=$((i + 1))
    done
}

# expected_text <action index>
expected_text() {
    if [ "${NA_KIND[$1]}" = "setdhcp" ]; then
        printf 'dhcp'
    else
        printf 'manual/%s/%s/%s' "${NA_IP[$1]}" "${NA_MASK[$1]}" "${NA_ROUTER[$1]}"
    fi
}

# verify_network <action index> <reads>: re-reads -getinfo (1 s apart) until the
# configuration matches. Checks the mode (and ip/mask/router for setmanual),
# not whether a DHCP lease has arrived yet.
verify_network() {
    local k="$1" svc="${NA_SVC[$1]}" t=0 out rc ok now note=""
    while :; do
        ok=0
        out=$("$NS" -getinfo "$svc" 2>&1 </dev/null)
        rc=$?
        if [ "$rc" -eq 0 ] && ! ns_error_text "$out"; then
            info_view "$out"
            now=$(view_text)
            if [ "${NA_KIND[k]}" = "setdhcp" ]; then
                if [ "$IV_MODE" = "dhcp" ]; then ok=1; fi
            elif [ "$IV_MODE" = "manual" ] && [ "$IV_IP" = "${NA_IP[k]}" ] && [ "$IV_MASK" = "${NA_MASK[k]}" ]; then
                if [ "${NA_ROUTER[k]}" = "-" ] || [ "$IV_ROUTER" = "${NA_ROUTER[k]}" ]; then ok=1; fi
            fi
        else
            now="unreadable ($(one_line "$out"))"
        fi
        [ "$ok" -eq 1 ] && break
        t=$((t + 1))
        [ "$t" -lt "$2" ] || break
        sleep 1
    done
    if [ "$ok" -eq 1 ]; then
        if [ "${NA_KIND[k]}" = "setdhcp" ]; then
            case "$IV_IP" in
                - | 169.254.*) note=" (no DHCP address yet: check the cable or the DHCP server)" ;;
            esac
        fi
        verify_ok "network:$svc" "now=$now$note"
    else
        verify_fail "network:$svc" "now=$now, expected $(expected_text "$k")"
    fi
}

# net_write <action index> <current -listallnetworkservices text>: exactly one
# networksetup write, argv only, values validated again right before use.
net_write() {
    local k="$1" svc kind detail out rc router
    svc="${NA_SVC[k]}"
    kind="${NA_KIND[k]}"
    detail="${NA_DETAIL[k]}"
    if ! service_listed "$svc" "$2"; then
        step_fail NETWORK "$kind" "$svc" "$detail" "the service is no longer listed: nothing was changed"
        return 0
    fi
    if [ "$kind" = "setdhcp" ]; then
        out=$("$NS" -setdhcp "$svc" 2>&1 </dev/null)
        rc=$?
    else
        router="${NA_ROUTER[k]}"
        if [ "$router" = "-" ]; then router=""; fi
        if ! is_ipv4 "${NA_IP[k]}" || ! is_mask "${NA_MASK[k]}" || { [ -n "$router" ] && ! is_ipv4 "$router"; }; then
            step_fail NETWORK "$kind" "$svc" "$detail" "address, mask or router is not valid IPv4: nothing was changed"
            return 0
        fi
        # No router: pass "" as the 4th argument, like the shipping helper (v1.13.1) that made these configs; real-macOS behaviour is settled in Phase 2.
        out=$("$NS" -setmanual "$svc" "${NA_IP[k]}" "${NA_MASK[k]}" "$router" 2>&1 </dev/null)
        rc=$?
    fi
    if [ "$rc" -ne 0 ] || ns_error_text "$out"; then
        step_fail NETWORK "$kind" "$svc" "$detail" "networksetup exit $rc: $(one_line "$out")"
        verify_network "$k" 1
        return 0
    fi
    step_ok NETWORK "$kind" "$svc" "$detail"
    verify_network "$k" "$NET_VERIFY_POLLS"
}

do_network() {
    local k n=${#NA_KIND[@]} list rc
    if [ "$n" -eq 0 ]; then
        step_ok NETWORK choice - "$NET_LABEL: no network settings were changed"
        return 0
    fi
    list=$("$NS" -listallnetworkservices 2>&1 </dev/null)
    rc=$?
    if [ "$rc" -ne 0 ] || ns_error_text "$list"; then
        list=""
    fi
    k=0
    while [ "$k" -lt "$n" ]; do
        case "${NA_KIND[k]}" in
            skip) step_fail NETWORK skip "${NA_SVC[k]}" - "${NA_DETAIL[k]}" ;;
            unchanged) step_ok NETWORK unchanged "${NA_SVC[k]}" "${NA_DETAIL[k]}" ;;
            setdhcp | setmanual) net_write "$k" "$list" ;;
        esac
        k=$((k + 1))
    done
}

wait_unloaded() {
    local k=0
    while label_loaded "$1"; do
        k=$((k + 1))
        if [ "$k" -ge 5 ]; then
            return 1
        fi
        sleep 1
    done
    return 0
}

do_stop() {
    local label plist out rc why
    for label in $LABELS; do
        if ! label_loaded "$label"; then
            case "$PLANNED_LABELS" in
                *" $label "*) step_ok STOP launchd "$label" "already stopped" ;;
            esac
            continue
        fi
        out=$("$LAUNCHCTL" bootout "system/$label" 2>&1 </dev/null)
        rc=$?
        if [ "$rc" -eq 0 ] && wait_unloaded "$label"; then
            step_ok STOP launchd "$label" "stopped (bootout)"
            continue
        fi
        why="bootout exit $rc: $(one_line "$out")"
        plist="$R/Library/LaunchDaemons/$label.plist"
        if [ -f "$plist" ]; then
            out=$("$LAUNCHCTL" unload "$plist" 2>&1 </dev/null)
            rc=$?
            if ! label_loaded "$label"; then
                step_ok STOP launchd "$label" "stopped (unload fallback; $why)"
                continue
            fi
            why="$why; unload exit $rc: $(one_line "$out")"
        fi
        step_fail STOP launchd "$label" loaded "still loaded ($why)"
    done
}

do_quit() {
    local k=0
    APP_STILL_RUNNING=0
    if ! app_running; then
        if [ "$APP_RUNNING" -eq 1 ]; then
            step_ok QUIT app "$APP_PROC" "already quit"
        fi
        return 0
    fi
    "$PKILL" -x "$APP_PROC" >/dev/null 2>&1 </dev/null
    while app_running; do
        k=$((k + 1))
        if [ "$k" -ge "$QUIT_POLLS" ]; then
            APP_STILL_RUNNING=1
            step_fail QUIT app "$APP_PROC" running "still running after 10 s"
            return 0
        fi
        sleep 0.5
    done
    step_ok QUIT app "$APP_PROC" quit
}

# safe_remove <display path> <anchor>: the ONLY place that deletes anything.
# Refuses anything empty, "/", relative or oddly formed, not on the inventory
# list, outside the effective root, or whose parent folder does not resolve to
# where it is expected. A symlink item loses only the link (no trailing slash).
safe_remove() {
    local p="$1" full err
    RM_REASON=""
    case "$p" in
        "")
            RM_REASON="refused: empty path"
            return 1
            ;;
        /)
            RM_REASON="refused: /"
            return 1
            ;;
        /*) ;;
        *)
            RM_REASON="refused: not an absolute path"
            return 1
            ;;
    esac
    case "$p" in
        */ | *//* | */./* | */../* | */. | */..)
            RM_REASON="refused: unexpected path form"
            return 1
            ;;
    esac
    case "$INV_LIST" in
        *"$NL$p$NL"*) ;;
        *)
            RM_REASON="refused: not on the inventory list"
            return 1
            ;;
    esac
    full="$R$p"
    if [ -n "$R" ]; then
        case "$full" in
            "$R"/?*) ;;
            *)
                RM_REASON="refused: outside the test root"
                return 1
                ;;
        esac
    fi
    if ! parent_ok "$p" "$2"; then
        RM_REASON="$PARENT_REASON"
        return 1
    fi
    err=$(rm -rf -- "$full" 2>&1)
    if exists "$full"; then
        RM_REASON="could not remove: $(one_line "${err:-still present}")"
        return 1
    fi
    return 0
}

# do_remove <all|A|B>
do_remove() {
    local i=0 n=${#ITEM_PATH[@]} p k d
    while [ "$i" -lt "$n" ]; do
        if [ "${ITEM_EXISTS[i]}" = 1 ] && stage_match "$i" "$1"; then
            p="${ITEM_PATH[i]}"
            k="${ITEM_KIND[i]}"
            d="-"
            if [ "${ITEM_LINK[i]}" = 1 ]; then
                d="symlink: only the link is removed"
            fi
            if [ -n "${ITEM_UNSAFE[i]}" ]; then
                step_fail REMOVE "$k" "$p" "$d" "${ITEM_UNSAFE[i]}"
            elif [ -n "${ITEM_SKIP[i]}" ]; then
                step_fail REMOVE "$k" "$p" "$d" "${ITEM_SKIP[i]}"
            elif [ "$p" = "$APP_BUNDLE" ] && [ "$APP_STILL_RUNNING" -eq 1 ]; then
                step_fail REMOVE "$k" "$p" "$d" "the app is still running"
            elif ! exists "$R$p"; then
                step_ok REMOVE "$k" "$p" "already gone"
            elif safe_remove "$p" "${ITEM_ANCHOR[i]}"; then
                step_ok REMOVE "$k" "$p" "$d"
            else
                step_fail REMOVE "$k" "$p" "$d" "$RM_REASON"
            fi
        fi
        i=$((i + 1))
    done
}

do_forget() {
    local id out rc
    read_receipts
    for id in $RECEIPTS; do
        if ! receipt_present "$id"; then
            case "$PLANNED_RECEIPTS" in
                *" $id "*) step_ok FORGET receipt "$id" "already forgotten" ;;
            esac
            continue
        fi
        out=$("$PKGUTIL" --forget "$id" 2>&1 </dev/null)
        rc=$?
        read_receipts
        if receipt_present "$id"; then
            step_fail FORGET receipt "$id" - "pkgutil exit $rc: $(one_line "$out")"
        else
            step_ok FORGET receipt "$id" -
        fi
    done
}

# do_verify <all|A>: inventory items of that stage, then every launchd label
# and receipt.
do_verify() {
    local i=0 n=${#ITEM_PATH[@]} label id
    while [ "$i" -lt "$n" ]; do
        if stage_match "$i" "$1"; then
            if exists "$R${ITEM_PATH[i]}"; then
                verify_fail "${ITEM_PATH[i]}"
            elif [ "${ITEM_EXISTS[i]}" = 1 ]; then
                verify_ok "${ITEM_PATH[i]}"
            fi
        fi
        i=$((i + 1))
    done
    for label in $LABELS; do
        if label_loaded "$label"; then
            verify_fail "launchd:$label"
        else
            verify_ok "launchd:$label"
        fi
    done
    read_receipts
    for id in $RECEIPTS; do
        if [ "$PKGS_RC" -ne 0 ]; then
            verify_fail "receipt:$id (pkgutil --pkgs failed)"
        elif receipt_present "$id"; then
            verify_fail "receipt:$id"
        else
            verify_ok "receipt:$id"
        fi
    done
}

# shown_log: the log path as the user sees it (the test root stripped).
shown_log() {
    local s="$LOG_FILE"
    if [ -n "$R" ]; then
        case "$LOG_FILE" in
            "$R"/*) s="${LOG_FILE#"$R"}" ;;
        esac
    fi
    printf '%s' "$s"
}

# final_report <title> <1: report leftovers | 0: not yet>
final_report() {
    local line n=0
    say "$1"
    if [ -n "$BACKUP_DIR" ]; then
        say "# backup folder: $BACKUP_DIR_DISPLAY"
    else
        say "# backup folder: none created (nothing to remove)"
    fi
    if [ -n "$LOG_FILE" ]; then
        say "# log: $(shown_log)"
    fi
    if [ "$FAIL_COUNT" -eq 0 ] && [ "$VFAIL_COUNT" -eq 0 ]; then
        say "# failed steps: 0"
    else
        say "# failed steps: $FAIL_COUNT; still present after verify: $VFAIL_COUNT"
        while IFS= read -r line; do
            [ -n "$line" ] && say "$line"
        done <<<"$FAIL_LIST"
    fi
    [ "$2" -eq 1 ] || return 0
    LEFTOVERS=$(find_leftovers)
    while IFS= read -r line; do
        [ -n "$line" ] && n=$((n + 1))
    done <<<"$LEFTOVERS"
    say "# leftovers (reported only, never removed): $n"
    while IFS= read -r line; do
        [ -n "$line" ] && say "LEFTOVER${TAB}path${TAB}$line${TAB}reported only, not removed"
    done <<<"$LEFTOVERS"
}

failed_any() {
    [ "$FAIL_COUNT" -ne 0 ] || [ "$VFAIL_COUNT" -ne 0 ]
}

# ---------------------------------------------------------------- --wait-pid

# stage_b_end: never returns (stage B is a background subshell).
stage_b_end() {
    local rc=0
    if failed_any; then rc=1; fi
    chmod -R a+rX "$BACKUP_DIR" 2>/dev/null
    say "# stage B exit: $rc"
    exit "$rc"
}

# stage_b <stage A start time>: runs detached (stdin and stdout /dev/null,
# stderr into the log; say still appends to the log). Waits for stage A to
# finish writing, then for WAIT_PID, never signalling it. Its result covers its
# own steps plus the final full verify.
stage_b() {
    local t0 i d
    trap '' HUP
    wait_gone "$$" "$1" 30 || true
    FAIL_COUNT=0
    VFAIL_COUNT=0
    FAIL_LIST=""
    say ""
    say "# stage B: started; waiting for process $WAIT_PID to exit (up to $WAIT_TIMEOUT s)"
    t0=$SECONDS
    if [ -z "$WAIT_START" ]; then
        say "# stage B: process $WAIT_PID was not running when stage A started: continuing now"
    elif wait_gone "$WAIT_PID" "$WAIT_START" "$WAIT_TIMEOUT"; then
        say "# stage B: process $WAIT_PID has exited (waited $((SECONDS - t0)) s)"
    else
        say "# stage B: process $WAIT_PID is still running after $WAIT_TIMEOUT s: the app bundle and per-user data are left in place"
        i=0
        while [ "$i" -lt ${#ITEM_PATH[@]} ]; do
            if [ "${ITEM_EXISTS[i]}" = 1 ] && [ "${ITEM_STAGE[i]}" = B ]; then
                d="-"
                if [ "${ITEM_LINK[i]}" = 1 ]; then d="symlink: only the link is removed"; fi
                step_fail REMOVE "${ITEM_KIND[i]}" "${ITEM_PATH[i]}" "$d" "not removed: process $WAIT_PID still running after $WAIT_TIMEOUT s"
            fi
            i=$((i + 1))
        done
        do_verify all
        final_report "# result: stage B" 1
        stage_b_end
    fi
    do_quit
    do_remove B
    do_verify all
    final_report "# result: stage B" 1
    stage_b_end
}

# run_stage_a: everything except the stage B items, then starts stage B in the
# background and exits with stage A's own result. Never returns.
run_stage_a() {
    local rc=0 me_start errlog
    do_stop
    do_remove A
    do_forget
    do_verify A
    final_report "# result: stage A" 0
    if failed_any; then rc=1; fi
    if [ -n "$WAIT_START" ]; then
        say "# stage B: waits for process $WAIT_PID to exit (up to $WAIT_TIMEOUT s), then removes the app bundle and per-user data; its result is appended to $(shown_log)"
    else
        say "# stage B: process $WAIT_PID is not running (treated as already exited), so stage B starts now and removes the app bundle and per-user data; its result is appended to $(shown_log)"
    fi
    if [ -n "$BACKUP_DIR" ]; then
        chmod -R a+rX "$BACKUP_DIR" 2>/dev/null
    fi
    me_start=$(pid_start "$$")
    errlog="${LOG_FILE:-/dev/null}"
    # Ignored before the fork, so stage B never sees a HUP (Terminal closing).
    trap '' HUP
    (stage_b "$me_start") </dev/null >/dev/null 2>>"$errlog" &
    say "# stage B process: $!"
    exit "$rc"
}

run_uninstall() {
    PLAN_TEXT=$(render_plan uninstall)
    say "$PLAN_TEXT"
    if [ "$YES" -ne 1 ]; then
        confirm UNINSTALL
    fi
    if [ "$HAS_WORK" -eq 1 ]; then
        if ! create_backup_dir; then
            step_fail BACKUP dir "$BACKUP_DIR_DISPLAY" - "$BACKUP_FAIL_REASON"
            say "# stopped before changing anything: no backup folder"
            exit 1
        fi
        if [ "$LOG_GIVEN" -ne 1 ]; then
            open_log "$BACKUP_DIR/rescue.log"
        fi
        if ! printf '%s\n' "$PLAN_TEXT" >"$BACKUP_DIR/plan.txt" 2>/dev/null; then
            step_fail BACKUP file plan.txt - "could not write plan.txt"
        fi
        do_snapshot
        do_backups
    fi
    do_network
    if [ "$FILES_WORK" -eq 1 ] && [ -n "$WAIT_PID" ]; then
        run_stage_a
    fi
    if [ "$FILES_WORK" -eq 1 ]; then
        do_stop
        do_quit
        do_remove all
        do_forget
    fi
    do_verify all
    final_report "# result" 1
    if [ -n "$BACKUP_DIR" ]; then
        chmod -R a+rX "$BACKUP_DIR" 2>/dev/null
    fi
    if failed_any; then
        exit 1
    fi
    exit 0
}

# ---------------------------------------------------------------- repair execution

WARN_COUNT=0
WARN_LIST=""

# verify_warn <subject> <detail>: the settings are right but something still
# needs the user's attention (no DHCP address yet). Not a failure.
verify_warn() {
    local line="VERIFY${TAB}warn${TAB}$1${TAB}$2"
    say "$line"
    WARN_COUNT=$((WARN_COUNT + 1))
    WARN_LIST="$WARN_LIST$line$NL"
}

# sys_run <tool> <args...>: the ONLY place a repair changes anything. argv
# only, never through a shell. Sets SYS_OUT and SYS_RC; returns 1 on a non-zero
# exit or networksetup error text.
sys_run() {
    local tool="$1"
    shift
    SYS_OUT=$("$tool" "$@" 2>&1 </dev/null)
    SYS_RC=$?
    if [ "$SYS_RC" -ne 0 ]; then
        return 1
    fi
    if [ "$tool" = "$NS" ] && ns_error_text "$SYS_OUT"; then
        return 1
    fi
    return 0
}

# tool_name <tool path>: the system tool's own name, also for a test fake.
tool_name() {
    case "$1" in
        "$NS") printf 'networksetup' ;;
        "$IPCONFIG") printf 'ipconfig' ;;
        "$DSCACHEUTIL") printf 'dscacheutil' ;;
        "$KILLALL") printf 'killall' ;;
        *) printf '%s' "${1##*/}" ;;
    esac
}

# sys_write <TYPE> <KIND> <SUBJECT> <DETAIL> <tool> <args...>: one write plus
# its OK or FAILED line.
sys_write() {
    local t="$1" k="$2" sj="$3" d="$4" tool="$5"
    shift 5
    if sys_run "$tool" "$@"; then
        step_ok "$t" "$k" "$sj" "$d"
        return 0
    fi
    step_fail "$t" "$k" "$sj" "$d" "$(tool_name "$tool") exit $SYS_RC: $(one_line "${SYS_OUT:-no output}")"
    return 1
}

# service_state <name> <-listallnetworkservices text>: enabled|disabled|missing.
service_state() {
    local line
    while IFS= read -r line; do
        if [ "$line" = "$1" ]; then
            printf 'enabled'
            return 0
        fi
        if [ "$line" = "*$1" ]; then
            printf 'disabled'
            return 0
        fi
    done <<<"$2"
    printf 'missing'
}

# repair_check <service> <0: soft | 1: service reset>: one read-back. Sets
# RCK_STATE ok|warn|bad and RCK_TEXT (the detail for the VERIFY line).
repair_check() {
    local svc="$1" out rc dns search en list ok=1 addr=1
    out=$("$NS" -getinfo "$svc" 2>&1 </dev/null)
    rc=$?
    if [ "$rc" -ne 0 ] || ns_error_text "$out"; then
        RCK_STATE=bad
        RCK_TEXT="now=unreadable ($(one_line "$out")), expected dhcp"
        return 0
    fi
    info_view "$out"
    RCK_TEXT="now=$(view_text)"
    if [ "$IV_MODE" != "dhcp" ]; then ok=0; fi
    if ! is_ipv4 "$IV_IP"; then
        addr=0
    else
        case "$IV_IP" in
            169.254.*) addr=0 ;;
        esac
    fi
    if [ "$2" -eq 1 ]; then
        out=$("$NS" -getdnsservers "$svc" 2>&1 </dev/null)
        rc=$?
        if [ "$rc" -ne 0 ] || ns_error_text "$out"; then dns="unreadable"; else dns=$(value_list "$out"); fi
        out=$("$NS" -getsearchdomains "$svc" 2>&1 </dev/null)
        rc=$?
        if [ "$rc" -ne 0 ] || ns_error_text "$out"; then search="unreadable"; else search=$(value_list "$out"); fi
        list=$("$NS" -listallnetworkservices 2>&1 </dev/null)
        case "$(service_state "$svc" "$list")" in
            enabled) en=yes ;;
            *) en=no ;;
        esac
        RCK_TEXT="$RCK_TEXT dns=$dns search=$search enabled=$en"
        if [ "$dns" != "-" ] || [ "$search" != "-" ] || [ "$en" != "yes" ]; then ok=0; fi
    fi
    if [ "$ok" -eq 0 ]; then
        RCK_STATE=bad
        if [ "$2" -eq 1 ]; then
            RCK_TEXT="$RCK_TEXT, expected dhcp dns=- search=- enabled=yes"
        else
            RCK_TEXT="$RCK_TEXT, expected dhcp"
        fi
    elif [ "$addr" -eq 1 ]; then
        RCK_STATE=ok
    else
        RCK_STATE=warn
        case "$IV_IP" in
            169.254.*) RCK_TEXT="$RCK_TEXT; link-local only (169.254.x.x): no DHCP server answered yet; check the cable, the switch or the DHCP server" ;;
            *) RCK_TEXT="$RCK_TEXT; no address yet: check the cable, the switch or the DHCP server" ;;
        esac
    fi
}

# repair_verify <0|1> <service>...: reads every service back once a second
# until all are ok or REPAIR_WAIT seconds have passed, then one VERIFY line each.
repair_verify() {
    local full="$1" t=0 i all
    local -a svcs st tx
    shift
    svcs=("$@")
    [ ${#svcs[@]} -gt 0 ] || return 0
    while :; do
        all=1
        i=0
        while [ "$i" -lt ${#svcs[@]} ]; do
            if [ "${st[i]:-}" != "ok" ]; then
                repair_check "${svcs[i]}" "$full"
                st[i]="$RCK_STATE"
                tx[i]="$RCK_TEXT"
                if [ "$RCK_STATE" != "ok" ]; then all=0; fi
            fi
            i=$((i + 1))
        done
        [ "$all" -eq 1 ] && break
        [ "$t" -lt "$REPAIR_WAIT" ] || break
        sleep 1
        t=$((t + 1))
    done
    i=0
    while [ "$i" -lt ${#svcs[@]} ]; do
        case "${st[i]}" in
            ok) verify_ok "repair:${svcs[i]}" "${tx[i]}" ;;
            warn) verify_warn "repair:${svcs[i]}" "${tx[i]}" ;;
            *) verify_fail "repair:${svcs[i]}" "${tx[i]}" ;;
        esac
        i=$((i + 1))
    done
}

do_soft() {
    local k dev
    local -a done_svcs=()
    k=0
    while [ "$k" -lt ${#RA_KIND[@]} ]; do
        case "${RA_KIND[k]}" in
            flushcache) sys_write REPAIR flushcache dns-cache "${RA_DETAIL[k]}" "$DSCACHEUTIL" -flushcache ;;
            hup) sys_write REPAIR hup mDNSResponder "${RA_DETAIL[k]}" "$KILLALL" -HUP mDNSResponder ;;
            renew)
                dev="${RA_DEV[k]}"
                if ! is_bsd_device "$dev"; then
                    step_fail REPAIR renew "${RA_SVC[k]}" "${RA_DETAIL[k]}" "not a plain device name: nothing was changed"
                elif sys_write REPAIR renew "${RA_SVC[k]}" "${RA_DETAIL[k]}" "$IPCONFIG" set "$dev" DHCP; then
                    done_svcs[${#done_svcs[@]}]="${RA_SVC[k]}"
                fi
                ;;
        esac
        k=$((k + 1))
    done
    if [ ${#done_svcs[@]} -gt 0 ]; then
        repair_verify 0 "${done_svcs[@]}"
    fi
}

# do_service_snapshot: service-before.txt, read again right before the first
# write. Returns 1 (nothing may change) when it cannot be written.
do_service_snapshot() {
    local f="$BACKUP_DIR/service-before.txt" svc="$REPAIR_SVC" info dns search ri rd rs
    info=$("$NS" -getinfo "$svc" 2>&1 </dev/null)
    ri=$?
    dns=$("$NS" -getdnsservers "$svc" 2>&1 </dev/null)
    rd=$?
    search=$("$NS" -getsearchdomains "$svc" 2>&1 </dev/null)
    rs=$?
    if ! {
        printf '# service-before.txt: read-only snapshot by %s version %s, %s\n' "$SCRIPT_NAME" "$RESCUE_VERSION" "$TS"
        printf '# service: %s\n' "$svc"
        printf '\n$ networksetup -getinfo "%s"\n%s\n(exit %s)\n' "$svc" "$info" "$ri"
        printf '\n$ networksetup -getdnsservers "%s"\n%s\n(exit %s)\n' "$svc" "$dns" "$rd"
        printf '\n$ networksetup -getsearchdomains "%s"\n%s\n(exit %s)\n' "$svc" "$search" "$rs"
    } >"$f" 2>/dev/null; then
        step_fail BACKUP snapshot "$svc" service-before.txt "could not write the snapshot"
        return 1
    fi
    if [ "$ri" -ne 0 ] || [ "$rd" -ne 0 ] || [ "$rs" -ne 0 ] || ns_error_text "$info$NL$dns$NL$search"; then
        step_fail BACKUP snapshot "$svc" service-before.txt "could not read the service's settings again"
        return 1
    fi
    step_ok BACKUP snapshot "$svc" service-before.txt
    return 0
}

# do_service: the reset steps in plan order. Each write runs even when an
# earlier one failed, and "on" is always tried (twice) once "off" was tried,
# so a service is never left turned off without saying how to fix it.
do_service() {
    local svc="$REPAIR_SVC" list k d
    list=$("$NS" -listallnetworkservices 2>&1 </dev/null)
    if [ "$(service_state "$svc" "$list")" != "enabled" ]; then
        step_fail REPAIR setdhcp "$svc" "was=$SR_NOW" "the service is no longer listed or no longer enabled: nothing was changed"
        return 0
    fi
    k=0
    while [ "$k" -lt ${#RA_KIND[@]} ]; do
        d="${RA_DETAIL[k]}"
        case "${RA_KIND[k]}" in
            setdhcp) sys_write REPAIR setdhcp "$svc" "$d" "$NS" -setdhcp "$svc" ;;
            cleardns) sys_write REPAIR cleardns "$svc" "$d" "$NS" -setdnsservers "$svc" Empty ;;
            clearsearch) sys_write REPAIR clearsearch "$svc" "$d" "$NS" -setsearchdomains "$svc" Empty ;;
            disable) sys_write REPAIR disable "$svc" "$d" "$NS" -setnetworkserviceenabled "$svc" off ;;
            enable)
                if sys_run "$NS" -setnetworkserviceenabled "$svc" on; then
                    step_ok REPAIR enable "$svc" "$d"
                else
                    sleep 1
                    if sys_run "$NS" -setnetworkserviceenabled "$svc" on; then
                        step_ok REPAIR enable "$svc" "$d"
                    else
                        step_fail REPAIR enable "$svc" "$d" "networksetup exit $SYS_RC: $(one_line "${SYS_OUT:-no output}") (tried twice). The service may be turned off: in Terminal run  sudo networksetup -setnetworkserviceenabled $(printf '%q' "$svc") on"
                    fi
                fi
                ;;
        esac
        k=$((k + 1))
    done
    repair_verify 1 "$svc"
}

repair_report() {
    local line
    say "# result"
    if [ -n "$BACKUP_DIR" ]; then
        say "# backup folder: $BACKUP_DIR_DISPLAY"
    fi
    if [ -n "$LOG_FILE" ]; then
        say "# log: $(shown_log)"
    fi
    if [ "$FAIL_COUNT" -eq 0 ] && [ "$VFAIL_COUNT" -eq 0 ]; then
        say "# failed steps: 0"
    else
        say "# failed steps: $FAIL_COUNT; failed checks: $VFAIL_COUNT"
        while IFS= read -r line; do
            [ -n "$line" ] && say "$line"
        done <<<"$FAIL_LIST"
    fi
    say "# warnings: $WARN_COUNT"
    while IFS= read -r line; do
        [ -n "$line" ] && say "$line"
    done <<<"$WARN_LIST"
}

# run_repair: plan -> REPAIR -> backup folder + snapshots -> writes -> verify.
# Never returns.
run_repair() {
    PLAN_TEXT=$(render_repair_plan repair)
    say "$PLAN_TEXT"
    if [ "$YES" -ne 1 ]; then
        confirm REPAIR
    fi
    if ! create_backup_dir; then
        step_fail BACKUP dir "$BACKUP_DIR_DISPLAY" - "$BACKUP_FAIL_REASON"
        say "# stopped before changing anything: no backup folder"
        exit 1
    fi
    if [ "$LOG_GIVEN" -ne 1 ]; then
        open_log "$BACKUP_DIR/rescue.log"
    fi
    if ! printf '%s\n' "$PLAN_TEXT" >"$BACKUP_DIR/plan.txt" 2>/dev/null; then
        step_fail BACKUP file plan.txt - "could not write plan.txt"
    fi
    do_snapshot
    case "$REPAIR_TIER" in
        soft) do_soft ;;
        service)
            if do_service_snapshot; then
                do_service
            else
                say "# stopped before changing anything: the service's settings were not backed up"
            fi
            ;;
    esac
    repair_report
    chmod -R a+rX "$BACKUP_DIR" 2>/dev/null
    if failed_any; then
        exit 1
    fi
    exit 0
}

# ---------------------------------------------------------------- menu

menu_quit() {
    printf '\n'
    refuse "cancelled: nothing has been changed"
}

# menu_read: one trimmed answer into MENU_ANS; end of input quits.
menu_read() {
    MENU_ANS=""
    IFS= read -r MENU_ANS || menu_quit
    MENU_ANS=$(trim "$MENU_ANS")
}

# menu_network: the network step. Builds NET_CHOICES exactly as the flags
# would; nothing is ticked to begin with.
menu_network() {
    local i n=0 k mark
    local -a idx tick
    local jt=0
    i=0
    while [ "$i" -lt ${#NET_SVC[@]} ]; do
        if [ "${NET_EN[i]}" -eq 1 ] && [ "${NET_RC[i]}" -eq 0 ] && ! ns_error_text "${NET_INFO[i]}"; then
            parse_info "${NET_INFO[i]}"
            if [ "$INFO_MANUAL" -eq 1 ]; then
                idx[n]=$i
                tick[n]=0
                n=$((n + 1))
            fi
        fi
        i=$((i + 1))
    done
    while :; do
        printf '\nNetwork settings (nothing is ticked to begin with). Anything left unticked stays exactly as it is.\n'
        if [ "$J_STATE" = "ok" ]; then
            mark=" "
            if [ "$jt" -eq 1 ]; then mark="x"; fi
            printf '  [%s] r) Restore the settings the app recorded before it first changed them (%s service(s); IPv4 only, not DNS)\n' "$mark" "${#J_SVC[@]}"
        elif [ "$J_STATE" = "invalid" ]; then
            printf '  (The journal of original settings cannot be used: %s)\n' "$J_REASON"
        fi
        if [ "$n" -eq 0 ]; then
            printf '  No enabled service is on Manual (static) settings.\n'
        else
            printf '  Services on Manual (static) settings. Tick one to switch it to DHCP:\n'
            k=0
            while [ "$k" -lt "$n" ]; do
                mark=" "
                if [ "${tick[k]}" -eq 1 ]; then mark="x"; fi
                parse_info "${NET_INFO[${idx[k]}]}"
                printf '  [%s] %s) %s   %s / %s\n' "$mark" "$((k + 1))" "${NET_SVC[${idx[k]}]}" "$INFO_IP" "$INFO_MASK"
                k=$((k + 1))
            done
        fi
        printf 'Type a number to tick or untick'
        if [ "$J_STATE" = "ok" ]; then printf ', r for the journal'; fi
        printf ', d when done, q to quit: '
        menu_read
        case "$MENU_ANS" in
            d | D) break ;;
            q | Q) menu_quit ;;
            r | R)
                if [ "$J_STATE" = "ok" ]; then
                    jt=$((1 - jt))
                else
                    printf 'There is no usable journal on this Mac.\n'
                fi
                ;;
            "" | *[!0-9]* | 0*) printf 'Not a choice: %s\n' "$MENU_ANS" ;;
            *)
                if [ ${#MENU_ANS} -le 3 ] && [ "$MENU_ANS" -ge 1 ] && [ "$MENU_ANS" -le "$n" ]; then
                    k=$((MENU_ANS - 1))
                    tick[k]=$((1 - tick[k]))
                else
                    printf 'Not a choice: %s\n' "$MENU_ANS"
                fi
                ;;
        esac
    done
    NET_CHOICES=""
    if [ "$jt" -eq 1 ]; then
        NET_CHOICES="restore-journal$NL"
    fi
    k=0
    while [ "$k" -lt "$n" ]; do
        if [ "${tick[k]}" -eq 1 ]; then
            NET_CHOICES="${NET_CHOICES}dhcp:${NET_SVC[${idx[k]}]}$NL"
        fi
        k=$((k + 1))
    done
    if [ -z "$NET_CHOICES" ]; then
        NET_CHOICES="leave$NL"
    fi
}

# menu_repair_service: a numbered list of the services a reset can take
# (enabled, DHCP or Manual); sets REPAIR_TIER/REPAIR_SVC like the flag would.
menu_repair_service() {
    local i n=0
    local -a idx
    i=0
    while [ "$i" -lt ${#NET_SVC[@]} ]; do
        if [ "${NET_EN[i]}" -eq 1 ] && [ "${NET_RC[i]}" -eq 0 ] && ! ns_error_text "${NET_INFO[i]}"; then
            info_view "${NET_INFO[i]}"
            if [ "$IV_MODE" != "unknown" ]; then
                idx[n]=$i
                n=$((n + 1))
            fi
        fi
        i=$((i + 1))
    done
    if [ "$n" -eq 0 ]; then
        printf 'No enabled service has DHCP or Manual settings, so there is nothing to reset.\n'
        menu_quit
    fi
    while :; do
        printf '\nReset one service: it is switched to DHCP, its DNS servers and search domains\n'
        printf 'are cleared, and it is turned off and on again. The next screen shows its\n'
        printf 'current values, which are backed up first.\n'
        i=0
        while [ "$i" -lt "$n" ]; do
            info_view "${NET_INFO[${idx[i]}]}"
            printf '  %s) %s   %s\n' "$((i + 1))" "${NET_SVC[${idx[i]}]}" "$(view_text)"
            i=$((i + 1))
        done
        printf 'Type a number, or q to quit: '
        menu_read
        case "$MENU_ANS" in
            q | Q) menu_quit ;;
            "" | *[!0-9]* | 0*) printf 'Not a choice: %s\n' "$MENU_ANS" ;;
            *)
                if [ ${#MENU_ANS} -le 3 ] && [ "$MENU_ANS" -ge 1 ] && [ "$MENU_ANS" -le "$n" ]; then
                    REPAIR_TIER=service
                    REPAIR_SVC="${NET_SVC[${idx[$((MENU_ANS - 1))]}]}"
                    REPAIR_ARG="service:$REPAIR_SVC"
                    return 0
                fi
                printf 'Not a choice: %s\n' "$MENU_ANS"
                ;;
        esac
    done
}

# menu_repair: the same path as the flags: resolve -> plan -> REPAIR -> run.
menu_repair() {
    resolve_repair
    run_repair
}

# menu_uninstall: the same path as the flags: choices -> plan -> UNINSTALL -> run.
menu_uninstall() {
    check_network_choices
    resolve_network
    run_uninstall
}

run_menu() {
    while :; do
        printf '\nIt Hurts When IP rescue script, version %s\n' "$RESCUE_VERSION"
        printf '%s\n' \
            "  1) Show the plan (changes nothing)" \
            "  2) Uninstall, and choose what happens to network settings" \
            "  3) Uninstall, and leave network settings as they are" \
            "  4) Repair network: soft refresh (flush DNS, renew DHCP leases; changes no settings)" \
            "  5) Repair network: reset one service (DHCP, clear its DNS, turn it off and on)" \
            "  6) Quit (changes nothing)"
        printf 'Choose 1-6: '
        menu_read
        case "$MENU_ANS" in
            1)
                NET_CHOICES=""
                check_network_choices
                resolve_network
                say "$(render_plan plan)"
                ;;
            2)
                mode_allowed uninstall "$EUID_NOW"
                menu_network
                menu_uninstall
                ;;
            3)
                mode_allowed uninstall "$EUID_NOW"
                NET_CHOICES="leave$NL"
                menu_uninstall
                ;;
            4)
                mode_allowed repair "$EUID_NOW"
                REPAIR_TIER=soft
                REPAIR_ARG=soft
                menu_repair
                ;;
            5)
                mode_allowed repair "$EUID_NOW"
                menu_repair_service
                menu_repair
                ;;
            6 | q | Q) menu_quit ;;
            *) printf 'Please type a number from 1 to 6.\n' ;;
        esac
    done
}

# ---------------------------------------------------------------- main

main() {
    local mode
    parse_args "$@"
    if [ "$SHOW_HELP" -eq 1 ]; then
        print_help
        exit 0
    fi
    if [ "$SHOW_VERSION" -eq 1 ]; then
        printf 'ithurtswhenip-rescue %s\n' "$RESCUE_VERSION"
        exit 0
    fi
    check_network_choices
    if [ -n "$REPAIR_TIER" ]; then
        if [ "$ACTION" = "uninstall" ]; then
            usage_error "--repair cannot be combined with --uninstall"
        fi
        if [ "$NET_GIVEN" -eq 1 ]; then
            usage_error "--repair cannot be combined with --network"
        fi
        if [ -n "$WAIT_PID" ]; then
            usage_error "--repair cannot be combined with --wait-pid"
        fi
    fi
    if [ -n "$WAIT_PID" ] && [ "$ACTION" != "uninstall" ] && [ "$ACTION" != "plan" ]; then
        usage_error "--wait-pid needs --uninstall (or --plan to preview the two stages)"
    fi

    EUID_NOW="${EUID:-$(id -u)}"
    apply_guards "$EUID_NOW"
    if [ "$TEST_MODE" -ne 1 ]; then
        PATH="/usr/bin:/bin:/usr/sbin:/sbin"
        export PATH
    fi
    LC_ALL=C
    export LC_ALL

    mode="${ACTION:-none}"
    if [ -n "$REPAIR_TIER" ]; then
        if [ "$mode" = "plan" ]; then mode="repair-plan"; else mode="repair"; fi
    fi
    if [ "$mode" = "none" ] && [ "$NET_GIVEN" -eq 0 ] && [ "$YES" -eq 0 ] && stdin_is_terminal; then
        mode="menu"
    fi
    mode_allowed "$mode" "$EUID_NOW"
    setup_log
    if [ "$mode" = "uninstall" ] && [ "$YES" -ne 1 ] && ! stdin_is_terminal; then
        refuse "--uninstall needs confirmation: run it in Terminal, or pass --yes"
    fi
    if [ "$mode" = "repair" ] && [ "$YES" -ne 1 ] && ! stdin_is_terminal; then
        refuse "--repair needs confirmation: run it in Terminal, or pass --yes"
    fi

    init_time
    case "$mode" in
        repair | repair-plan)
            # A repair reads only network settings: no inventory, launchd,
            # receipt or process lookups.
            read_network
            choose_backup_base
            resolve_repair
            if [ "$mode" = "repair-plan" ]; then
                say "$(render_repair_plan plan)"
                exit 0
            fi
            run_repair
            ;;
    esac
    build_inventory
    collect_state

    case "$mode" in
        menu)
            run_menu
            ;;
        plan)
            resolve_network
            say "$(render_plan plan)"
            exit 0
            ;;
        none)
            resolve_network
            say "$(render_plan plan)"
            if [ -t 1 ]; then
                say "# to remove everything listed: sudo bash $SCRIPT_NAME --uninstall"
                exit 0
            fi
            printf 'rescue: no action given: printed the plan only (use --plan or --uninstall)\n' >&2
            exit 2
            ;;
        uninstall)
            resolve_network
            run_uninstall
            ;;
    esac
}

# Only run when executed, not when sourced (the test suite sources this file to
# unit-test apply_guards, mode_allowed and the journal parser without running anything).
if [ "${BASH_SOURCE[0]}" = "$0" ]; then
    main "$@"
fi
