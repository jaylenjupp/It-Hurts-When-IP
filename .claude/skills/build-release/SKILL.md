---
name: build-release
description: Use when touching build scripts (build-pkg.sh, scripts/build-macos.sh, scripts/verify-pkg.sh, scripts/bump-version.sh, scripts/ship/), tauri.conf.json bundle settings, versioning, signing, or making draft/published GitHub releases. Guarantees every build is universal, signed, verifiable without installing, traceable to a commit, and that dev builds stay private.
---
# Build & release (macOS)

## Contract / core rule
**Every `.pkg` is universal (arm64 + x86_64), ad-hoc signed, verified without being installed, and traceable to one commit. Dev builds are only ever draft releases; publishing to colleagues is an owner decision.**

## Details

### Where things run
Everything here runs **on the Mac Studio, repo root**, except installing a `.pkg`, which only Jay does on his travel Mac. Never `sudo`, never `installer`, never `open *.pkg` on the build host.

### Build (Phase 0 creates `scripts/build-macos.sh`)
Wraps today's manual steps from the top of `build-pkg.sh` into one command:
1. `rustup target list --installed` must show `aarch64-apple-darwin` and `x86_64-apple-darwin` (if missing: ask the owner to approve `rustup target add …`).
2. Helper: `cargo build --release --target aarch64-apple-darwin` and `--target x86_64-apple-darwin` in `helper-tool/`, `lipo -create … -output helper-tool/com.ithurtswhenip.helper`, `codesign -s - --force helper-tool/com.ithurtswhenip.helper`.
3. App: `npm ci` (only if `node_modules` missing or lockfile changed), `npm run tauri build -- --target universal-apple-darwin --bundles app` (skip the dmg; it isn't distributed — TODO(verify) the `--bundles` flag form with the installed Tauri CLI via `npm run tauri build -- --help`).
4. `./build-pkg.sh` → `ItHurtsWhenIP-<version>.pkg`. Move output to `release-out/` (gitignored).
5. Run `scripts/verify-pkg.sh` on the result. Fail the build if it fails.
Print the pkg path and SHA-256 at the end.

### Verify without installing (`scripts/verify-pkg.sh`, Phase 0)
- `pkgutil --payload-files` lists exactly: the app bundle, helper binary, LaunchDaemon plist (and nothing from `node_modules`, no `._*` files, no `.DS_Store`).
- `pkgutil --expand` to a temp dir; check `preinstall`/`postinstall` exist and pass `bash -n`.
- Extract payload (`cpio`/`tar` from the expanded `Payload`) to a temp dir; `lipo -archs` on the app binary and helper shows `x86_64 arm64`; `codesign --verify` passes on both; `plutil -lint` on Info.plist and the LaunchDaemon plist.
- `Info.plist` `CFBundleShortVersionString` equals `tauri.conf.json` version; `LSApplicationCategoryType` present once added (Phase 6).
- The rescue script is present in `Contents/Resources/` once Phase 1 lands.

### Build stamp
Phase 0 adds a build stamp (short git SHA, dirty flag, UTC build time) baked in at compile time (e.g. via `build.rs` → `env!`), shown in Settings → Updates and in diagnostics, so Jay can tell drafts apart.

### Versions
- `scripts/bump-version.sh X.Y.Z` (dry run) then `--apply`; needs a clean tree. Phase 0 removes the Inno Setup and Windows service entries from it (Windows parked) and keeps the README screenshot exclusion.
- Dev builds don't bump the version; they're distinguished by build stamp and draft tag (`dev-v<version>-<UTC stamp>-<sha7>`). Bump before a release the owner wants to publish.

### Releases
- **Draft (pre-approved):** commit on `dev` → `git push origin dev` → `./scripts/ship/release-dev.sh release-out/ItHurtsWhenIP-<v>.pkg notes.md`. The script refuses unless on `dev`, clean, and pushed; always passes `--draft`; checks `isDraft` afterwards. Drafts are only visible to accounts with write access, and `/releases/latest` ignores them, so colleagues' update checkers never see them.
- **Published (owner approves each step):** merge `dev` → `main`, bump version, build, `gh release create vX.Y.Z <pkg> recovery/ithurtswhenip-rescue.command --target main --title … --notes-file …` (not draft). This is what colleagues' update checker finds. Tag must be `vX.Y.Z` (the checker strips `v` and parses semver).
- Release notes for colleagues: plain language, what changed, how to get back to normal if anything goes wrong (link to the rescue script asset).

## Known quirks
- Ad-hoc signing only (`codesign -s -`); Gatekeeper still shows an allow prompt. Full Developer ID + notarization is a "later idea", not this round.
- Tauri's JSON merge patch replaces arrays wholesale (`tauri.*.conf.json`) — write complete arrays.
- `pkgbuild` relocatable bundles silently no-op on first install; `build-pkg.sh` already disables `BundleIsRelocatable`. Keep that.
- AppleDouble `._*` files: `COPYFILE_DISABLE=1` + `dot_clean` already in `build-pkg.sh`.
- Cargo package names can't contain spaces; only `version` changes during bumps.
- `.gitignore` already ignores `*.pkg`, `installer-build/`; add `release-out/`.

## Tests
- `bash -n` + `shellcheck` on all scripts.
- `release-dev.sh --dry-run` prints tag/target/assets without uploading.
- `verify-pkg.sh` is itself the build's acceptance test; run it on every pkg.
- A phase isn't done until a draft release exists and `gh release view <tag> --json isDraft,assets` shows `isDraft: true` and the expected assets.
