#!/usr/bin/env bash
# release-dev.sh — upload a dev build as a DRAFT GitHub release.
#
# Runs on: the Mac Studio (build host), from anywhere inside the repo.
#
# Draft releases are visible only to people with write access to the repo
# (i.e. Jay), so colleagues never see them, and GitHub's /releases/latest
# endpoint (used by the in-app update checker) ignores drafts. This script
# ALWAYS creates a draft. Publishing a release is a separate, owner-approved
# step (see docs/RELEASING.md).
#
# Usage:
#   ./scripts/ship/release-dev.sh <path/to/App.pkg> [notes.md] [--dry-run]
#
# Preconditions (checked):
#   - gh is installed and authenticated
#   - current branch is `dev`, working tree is clean
#   - HEAD has been pushed to origin/dev (git push origin dev)
#
# Edits to this file need owner approval (.claude/settings.json "ask" rule).

set -euo pipefail

die() { echo "release-dev: ERROR: $*" >&2; exit 1; }

PKG=""
NOTES=""
DRY_RUN=false
for arg in "$@"; do
    case "$arg" in
        --dry-run) DRY_RUN=true ;;
        -h|--help) sed -n '2,20p' "$0"; exit 0 ;;
        *)
            if [[ -z "$PKG" ]]; then PKG="$arg"
            elif [[ -z "$NOTES" ]]; then NOTES="$arg"
            else die "unexpected argument: $arg"; fi
            ;;
    esac
done

[[ -n "$PKG" ]] || die "usage: $0 <path/to/App.pkg> [notes.md] [--dry-run]"
[[ -f "$PKG" ]] || die "pkg not found: $PKG"
[[ "$PKG" == *.pkg ]] || die "expected a .pkg file, got: $PKG"
if [[ -n "$NOTES" ]]; then [[ -f "$NOTES" ]] || die "notes file not found: $NOTES"; fi

command -v gh >/dev/null 2>&1 || die "gh (GitHub CLI) not installed"
command -v git >/dev/null 2>&1 || die "git not installed"

REPO_ROOT="$(git rev-parse --show-toplevel)" || die "not inside a git repo"
cd "$REPO_ROOT"

BRANCH="$(git rev-parse --abbrev-ref HEAD)"
[[ "$BRANCH" == "dev" ]] || die "dev builds are released from the 'dev' branch (current: $BRANCH)"

if ! git diff --quiet || ! git diff --cached --quiet; then
    die "working tree has uncommitted changes — commit first so the build matches a commit"
fi

SHA="$(git rev-parse HEAD)"
SHORT="$(git rev-parse --short=7 HEAD)"

git fetch --quiet origin dev || die "could not fetch origin/dev"
if ! git merge-base --is-ancestor "$SHA" origin/dev; then
    die "HEAD ($SHORT) is not on origin/dev yet — run: git push origin dev"
fi

VERSION="$(grep -Eo '"version"[[:space:]]*:[[:space:]]*"[^"]+"' src-tauri/tauri.conf.json | head -1 | sed -E 's/.*"([^"]+)"$/\1/')"
[[ -n "$VERSION" ]] || die "could not read version from src-tauri/tauri.conf.json"

STAMP="$(date -u +%Y%m%d-%H%M)"
TAG="dev-v${VERSION}-${STAMP}-${SHORT}"
TITLE="Dev build ${VERSION} (${SHORT}, ${STAMP} UTC)"

# Checksum alongside the pkg so Jay can confirm the download on the road.
SUMFILE="$(mktemp -d)/$(basename "$PKG").sha256"
( cd "$(dirname "$PKG")" && shasum -a 256 "$(basename "$PKG")" ) > "$SUMFILE"

ASSETS=("$PKG" "$SUMFILE")
RESCUE="recovery/ithurtswhenip-rescue.command"
if [[ -f "$RESCUE" ]]; then ASSETS+=("$RESCUE"); fi

NOTES_FILE="$(mktemp)"
{
    echo "**Draft dev build — not visible to colleagues.**"
    echo
    echo "- Version: \`${VERSION}\`"
    echo "- Commit: \`${SHA}\`"
    echo "- Built on: $(hostname -s), $(sw_vers -productVersion 2>/dev/null || echo unknown) — $(date -u '+%Y-%m-%d %H:%M UTC')"
    echo "- SHA-256: \`$(awk '{print $1}' "$SUMFILE")\`"
    echo
    if [[ -n "$NOTES" ]]; then cat "$NOTES"; else echo "_No notes supplied._"; fi
} > "$NOTES_FILE"

echo "Tag:    $TAG"
echo "Target: $SHA"
echo "Assets: ${ASSETS[*]}"
if $DRY_RUN; then
    echo "--- notes ---"; cat "$NOTES_FILE"
    echo "Dry run: nothing uploaded."
    exit 0
fi

gh release create "$TAG" "${ASSETS[@]}" \
    --draft \
    --target "$SHA" \
    --title "$TITLE" \
    --notes-file "$NOTES_FILE"

IS_DRAFT="$(gh release view "$TAG" --json isDraft --jq .isDraft)"
URL="$(gh release view "$TAG" --json url --jq .url)"
if [[ "$IS_DRAFT" != "true" ]]; then
    die "release $TAG is NOT a draft (isDraft=$IS_DRAFT) — check it on GitHub immediately: $URL"
fi

echo
echo "Draft release created: $URL"
echo "Open it on GitHub while signed in as the repo owner to download the .pkg."
