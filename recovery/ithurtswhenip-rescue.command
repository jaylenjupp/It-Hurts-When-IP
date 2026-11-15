#!/bin/bash
#
# ithurtswhenip-rescue.command - standalone uninstall and rescue for
# "It Hurts When IP" (and old "IP Switcher" installs).
#
# Runs on: any Mac with the app installed, in Terminal. It needs nothing but
# macOS built-ins and bash 3.2, and it never needs the app or its helper.
#
#     sudo bash ithurtswhenip-rescue.command --plan        # shows every step, changes nothing
#     sudo bash ithurtswhenip-rescue.command --uninstall   # removes everything (asks first)
#
# Every removal is planned, backed up to /Users/Shared/ItHurtsWhenIP-backups/<UTC time>/,
# executed, then verified. Unknown files that merely look related are reported,
# never deleted.
#
# Test mode (never as root): IHWIP_ROOT=<fake tree> plus IHWIP_NETWORKSETUP,
# IHWIP_LAUNCHCTL, IHWIP_PKGUTIL, IHWIP_PGREP and IHWIP_PKILL pointing at fakes,
# optional IHWIP_NOW=YYYYMMDDTHHMMSSZ. See recovery/tests/run-tests.sh.
# As root every IHWIP_* variable is ignored.
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
LABELS="com.ithurtswhenip.helper com.ipswitcher.helper"
RECEIPTS="com.ithurtswhenip.pkg com.ipswitcher.pkg com.ipswitcher.helper"
USER_IDS="com.ithurtswhenip.desktop com.ithurtswhenip.app com.ithurtswhenip.switcher com.ipswitcher.switcher com.ipswitcher.app it-hurts-when-ip"
BACKUP_PARENT="/Users/Shared"
BACKUP_NAME="ItHurtsWhenIP-backups"
QUIT_POLLS=20 # x 0.5 s = 10 s

REAL_NETWORKSETUP="/usr/sbin/networksetup"
REAL_LAUNCHCTL="/bin/launchctl"
REAL_PKGUTIL="/usr/sbin/pkgutil"
REAL_PGREP="/usr/bin/pgrep"
REAL_PKILL="/usr/bin/pkill"

OVERRIDE_VARS="IHWIP_ROOT IHWIP_NETWORKSETUP IHWIP_LAUNCHCTL IHWIP_PKGUTIL IHWIP_PGREP IHWIP_PKILL IHWIP_NOW"
TOOL_VARS="IHWIP_NETWORKSETUP IHWIP_LAUNCHCTL IHWIP_PKGUTIL IHWIP_PGREP IHWIP_PKILL"

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
       bash $SCRIPT_NAME --help
EOF
}

print_help() {
    cat <<EOF
It Hurts When IP rescue script, version $RESCUE_VERSION

Removes It Hurts When IP completely (app, privileged helper, settings and old
"IP Switcher" files), without needing the app or its helper to work.

Usage: sudo bash $SCRIPT_NAME [--plan | --uninstall] [options]

Actions:
  --plan, --dry-run     Print every step --uninstall would take. Changes nothing.
  --uninstall           Back up, then remove everything listed by --plan.
                        Needs sudo. Asks you to type UNINSTALL unless --yes.
  (no action)           In Terminal: print the plan. Otherwise: print the plan
                        and exit 2.

Options:
  --network leave       Leave network settings exactly as they are. This is the
                        default and the only choice in this version.
  --network restore-journal, --network dhcp:"<service>"
                        Accepted but not available in this build yet.
  --yes                 Do not ask for confirmation (the app passes this after
                        asking you itself).
  --log <path>          Also append all output to <path>; its folder must exist.
                        Default for --uninstall: rescue.log in the backup folder.
  --wait-pid <pid>      Not available in this build yet.
  --repair ...          Network repair arrives in a later version.
  --help                Show this help.
  --version             Print the script version.

Exit codes:
  0  everything worked
  1  some steps failed (each one is listed at the end)
  2  refused: bad arguments, not run with sudo, cancelled, or no action given
     outside Terminal

Example (changes nothing):
  sudo bash $SCRIPT_NAME --plan

--plan only reads: it never changes files, background services, receipts or
network settings.
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

parse_args() {
    ACTION=""
    YES=0
    LOG_ARG=""
    LOG_GIVEN=0
    WAIT_PID=""
    SHOW_HELP=0
    SHOW_VERSION=0
    NET_CHOICES=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --plan | --dry-run) set_action plan ;;
            --uninstall) set_action uninstall ;;
            --network)
                need_value "$@"
                check_network_value "$2"
                NET_CHOICES="$NET_CHOICES$2$NL"
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
            --repair | --repair=*) refuse "network repair arrives in a later version of this script" ;;
            -h | --help) SHOW_HELP=1 ;;
            --version) SHOW_VERSION=1 ;;
            *) usage_error "unknown option: $1" ;;
        esac
        shift
    done
}

# ------------------------------------------- NETWORK CHOICES (package 1.2 replaces this block)
# This build implements only "leave". restore-journal and dhcp:<service> are
# parsed and shape-checked in parse_args, then refused here.
check_network_choices() {
    local c
    while IFS= read -r c; do
        [ -n "$c" ] || continue
        case "$c" in
            leave) ;;
            *) refuse "--network $c is not available in this build yet (only --network leave)" ;;
        esac
    done <<<"$NET_CHOICES"
}

do_network() {
    step_ok "NETWORK" "choice" "-" "leave: no network settings were changed"
}
# ------------------------------------------- end of NETWORK CHOICES block

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
    R=""
    TEST_MODE=0

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
    R="$res"
    TEST_MODE=1
    NS="$IHWIP_NETWORKSETUP"
    LAUNCHCTL="$IHWIP_LAUNCHCTL"
    PKGUTIL="$IHWIP_PKGUTIL"
    PGREP="$IHWIP_PGREP"
    PKILL="$IHWIP_PKILL"
    return 0
}

# mode_allowed <action> <euid>: --uninstall needs root, or test mode.
mode_allowed() {
    if [ "$1" = "uninstall" ] && [ "$2" != "0" ] && [ "$TEST_MODE" -ne 1 ]; then
        refuse "--uninstall needs administrator rights. Run:  sudo bash $SCRIPT_NAME --uninstall"
    fi
    return 0
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

add_item() {
    local n=${#ITEM_PATH[@]}
    ITEM_PATH[n]="$1"
    ITEM_BACKUP[n]="$2"
    ITEM_HINT[n]="$3"
    ITEM_ANCHOR[n]="$4"
    ITEM_BREL[n]="$5"
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
    local p b k u id t rel i
    ITEM_PATH=()
    ITEM_BACKUP=()
    ITEM_HINT=()
    ITEM_ANCHOR=()
    ITEM_BREL=()
    ITEM_EXISTS=()
    ITEM_KIND=()
    ITEM_LINK=()
    ITEM_UNSAFE=()
    ITEM_SKIP=()
    INV_LIST="$NL"
    while IFS='|' read -r p b k; do
        [ -n "$p" ] || continue
        add_item "$p" "$b" "$k" "$(anchor_of "$p")" "system$p"
    done <<<"$SYSTEM_ITEMS"
    collect_users
    i=0
    while [ "$i" -lt ${#USERS[@]} ]; do
        u="${USERS[i]}"
        for id in $USER_IDS; do
            while IFS='|' read -r t b; do
                [ -n "$t" ] || continue
                rel="${t//@ID@/$id}"
                add_item "/Users/$u/$rel" "$b" "" "/Users/$u" "users/$u/$rel"
            done <<<"$USER_ITEMS"
        done
        i=$((i + 1))
    done
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

# read_network: read-only. Fills NET_SVC / NET_INFO / NET_RC; NET_ERR on failure.
read_network() {
    local out rc line n
    NET_SVC=()
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
        line="${line#\*}"
        [ -n "$line" ] || continue
        n=${#NET_SVC[@]}
        NET_SVC[n]="$line"
        NET_INFO[n]=$("$NS" -getinfo "$line" 2>&1 </dev/null)
        NET_RC[n]=$?
    done <<<"$out"
    return 0
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

collect_state() {
    local i n label id
    HAS_WORK=0
    n=${#ITEM_PATH[@]}
    i=0
    while [ "$i" -lt "$n" ]; do
        if exists "$R${ITEM_PATH[i]}"; then
            ITEM_EXISTS[i]=1
            HAS_WORK=1
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
            HAS_WORK=1
        fi
    done
    read_receipts
    PLANNED_RECEIPTS=""
    for id in $RECEIPTS; do
        if receipt_present "$id"; then
            PLANNED_RECEIPTS="$PLANNED_RECEIPTS $id "
            HAS_WORK=1
        fi
    done
    APP_RUNNING=0
    if app_running; then
        APP_RUNNING=1
        HAS_WORK=1
    fi
    read_network
    LEFTOVERS=$(find_leftovers)
    choose_backup_base
}

# ---------------------------------------------------------------- plan

rec() {
    printf '%s\t%s\t%s\t%s\n' "$1" "$2" "$3" "$4"
}

# render_plan <plan|uninstall>: pure; prints the header and one record per line.
render_plan() {
    local i n p d label id cb=0 cn=0 cs=0 cq=0 cr=0 cf=0 cl=0 cx=0
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

    if [ "$HAS_WORK" -eq 0 ]; then
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
                if [ "$INFO_MANUAL" -eq 1 ]; then
                    rec NETWORK manual "${NET_SVC[i]}" "ip=$INFO_IP mask=$INFO_MASK router=$INFO_ROUTER selected=no"
                    cn=$((cn + 1))
                fi
            fi
            i=$((i + 1))
        done
    fi
    rec NETWORK choice - leave
    if exists "$R$JOURNAL_FILE"; then
        rec NOTE info journal "found; restore arrives in the next build"
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

# ---------------------------------------------------------------- execution

FAIL_COUNT=0
VFAIL_COUNT=0
FAIL_LIST=""
BACKUP_DIR=""

step_ok() {
    say "OK${TAB}$1${TAB}$2${TAB}$3${TAB}$4"
}

step_fail() {
    local line="FAILED${TAB}$1${TAB}$2${TAB}$3${TAB}$4${TAB}$5"
    say "$line"
    FAIL_COUNT=$((FAIL_COUNT + 1))
    FAIL_LIST="$FAIL_LIST$line$NL"
}

verify_ok() {
    say "VERIFY${TAB}ok${TAB}$1"
}

verify_fail() {
    local line="VERIFY${TAB}FAILED${TAB}$1"
    say "$line"
    VFAIL_COUNT=$((VFAIL_COUNT + 1))
    FAIL_LIST="$FAIL_LIST$line$NL"
}

confirm() {
    local answer=""
    printf 'Type UNINSTALL to continue: '
    IFS= read -r answer || answer=""
    if [ "$answer" != "UNINSTALL" ]; then
        refuse "cancelled: nothing has been changed"
    fi
    log_line "# confirmed by typing UNINSTALL"
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

do_remove() {
    local i=0 n=${#ITEM_PATH[@]} p k d
    while [ "$i" -lt "$n" ]; do
        if [ "${ITEM_EXISTS[i]}" = 1 ]; then
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

do_verify() {
    local i=0 n=${#ITEM_PATH[@]} label id
    while [ "$i" -lt "$n" ]; do
        if exists "$R${ITEM_PATH[i]}"; then
            verify_fail "${ITEM_PATH[i]}"
        elif [ "${ITEM_EXISTS[i]}" = 1 ]; then
            verify_ok "${ITEM_PATH[i]}"
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

final_report() {
    local line n=0 shown_log
    say "# result"
    if [ -n "$BACKUP_DIR" ]; then
        say "# backup folder: $BACKUP_DIR_DISPLAY"
    else
        say "# backup folder: none created (nothing to remove)"
    fi
    if [ -n "$LOG_FILE" ]; then
        shown_log="$LOG_FILE"
        if [ -n "$R" ]; then
            case "$LOG_FILE" in
                "$R"/*) shown_log="${LOG_FILE#"$R"}" ;;
            esac
        fi
        say "# log: $shown_log"
    fi
    if [ "$FAIL_COUNT" -eq 0 ] && [ "$VFAIL_COUNT" -eq 0 ]; then
        say "# failed steps: 0"
    else
        say "# failed steps: $FAIL_COUNT; still present after verify: $VFAIL_COUNT"
        while IFS= read -r line; do
            [ -n "$line" ] && say "$line"
        done <<<"$FAIL_LIST"
    fi
    LEFTOVERS=$(find_leftovers)
    while IFS= read -r line; do
        [ -n "$line" ] && n=$((n + 1))
    done <<<"$LEFTOVERS"
    say "# leftovers (reported only, never removed): $n"
    while IFS= read -r line; do
        [ -n "$line" ] && say "LEFTOVER${TAB}path${TAB}$line${TAB}reported only, not removed"
    done <<<"$LEFTOVERS"
}

run_uninstall() {
    PLAN_TEXT=$(render_plan uninstall)
    say "$PLAN_TEXT"
    if [ "$YES" -ne 1 ]; then
        confirm
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
    if [ "$HAS_WORK" -eq 1 ]; then
        do_stop
        do_quit
        do_remove
        do_forget
    fi
    do_verify
    final_report
    if [ -n "$BACKUP_DIR" ]; then
        chmod -R a+rX "$BACKUP_DIR" 2>/dev/null
    fi
    if [ "$FAIL_COUNT" -ne 0 ] || [ "$VFAIL_COUNT" -ne 0 ]; then
        exit 1
    fi
    exit 0
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
    if [ -n "$WAIT_PID" ]; then
        refuse "--wait-pid is not available in this build yet"
    fi
    check_network_choices

    EUID_NOW="${EUID:-$(id -u)}"
    apply_guards "$EUID_NOW"
    if [ "$TEST_MODE" -ne 1 ]; then
        PATH="/usr/bin:/bin:/usr/sbin:/sbin"
        export PATH
    fi
    LC_ALL=C
    export LC_ALL

    mode="${ACTION:-none}"
    mode_allowed "$mode" "$EUID_NOW"
    setup_log
    if [ "$mode" = "uninstall" ] && [ "$YES" -ne 1 ] && [ ! -t 0 ]; then
        refuse "--uninstall needs confirmation: run it in Terminal, or pass --yes"
    fi

    init_time
    build_inventory
    collect_state

    case "$mode" in
        plan)
            say "$(render_plan plan)"
            exit 0
            ;;
        none)
            say "$(render_plan plan)"
            if [ -t 1 ]; then
                say "# to remove everything listed: sudo bash $SCRIPT_NAME --uninstall"
                exit 0
            fi
            printf 'rescue: no action given: printed the plan only (use --plan or --uninstall)\n' >&2
            exit 2
            ;;
        uninstall)
            run_uninstall
            ;;
    esac
}

# Only run when executed, not when sourced (the test suite sources this file to
# unit-test apply_guards and mode_allowed without running anything).
if [ "${BASH_SOURCE[0]}" = "$0" ]; then
    main "$@"
fi
