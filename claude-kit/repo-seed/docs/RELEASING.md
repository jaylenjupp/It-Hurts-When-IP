# Releasing

Two kinds of release live on the same public repo's Releases page:

| | Draft (dev build) | Published (colleagues) |
|---|---|---|
| Who sees it | Only accounts with write access (you) | Everyone |
| Update checker sees it | No — `/releases/latest` ignores drafts | Yes |
| Tag | `dev-v<version>-<UTC stamp>-<sha7>` (created only if ever published) | `vX.Y.Z` |
| Branch | `dev` | `main` |
| Who can start it | Claude, pre-approved | Claude asks you at every step |

Sources: GitHub docs — only users with write access can view draft releases; published releases are public (https://docs.github.com/en/rest/releases/releases, https://docs.github.com/en/repositories/releasing-projects-on-github/managing-releases-in-a-repository).

## Draft build (Mac Studio, repo root — Claude does this)
```bash
git switch dev
git push origin dev                                   # pre-approved
./scripts/build-macos.sh                              # → release-out/ItHurtsWhenIP-<v>.pkg (verified)
./scripts/ship/release-dev.sh release-out/ItHurtsWhenIP-<v>.pkg notes.md
```
`release-dev.sh` refuses unless you're on `dev`, the tree is clean and the commit is on `origin/dev`; it always creates a draft and double-checks `isDraft` afterwards. Assets: the `.pkg`, its `.sha256`, and `ithurtswhenip-rescue.command` (from Phase 1).

## Getting a draft onto your travel Mac (travel Mac, browser + Terminal)
1. Browser, signed in to GitHub as jaylenjupp: https://github.com/jaylenjupp/it-hurts-when-ip/releases — drafts are at the top, marked **Draft**. (TODO(verify) whether the GitHub mobile app lists drafts; the website does.)
2. Download the `.pkg`, the `.sha256` and the rescue script. Keep the rescue script — it works offline.
3. Terminal, in Downloads: `shasum -a 256 -c ItHurtsWhenIP-<v>.pkg.sha256` → `OK`.
4. Install as usual (Privacy & Security → Open Anyway, then your password). Follow `docs/ROAD-TESTING.md`.

## Publishing to colleagues (Claude asks before each step)
1. Bump version on `dev`: `./scripts/bump-version.sh X.Y.Z` → `--apply`, commit.
2. Merge `dev` into `main` and push `main` (ask).
3. Build from `main`, verify.
4. `gh release create vX.Y.Z release-out/ItHurtsWhenIP-X.Y.Z.pkg recovery/ithurtswhenip-rescue.command --target main --title "It Hurts When IP X.Y.Z" --notes-file notes.md` (ask). **Not** `--draft`.
5. Check: `gh release view vX.Y.Z --json isDraft,isPrerelease,assets` and that https://api.github.com/repos/jaylenjupp/it-hurts-when-ip/releases/latest shows the new tag.

## Housekeeping
Drafts pile up. Claude may list them (`gh release list`) but deleting needs your OK (`gh release delete` is an "ask" rule).
