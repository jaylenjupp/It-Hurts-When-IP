# Plan — It Hurts When IP (macOS refinement round, from v1.13.1)

Goal of this round: make the app safe on show-critical Macs (full uninstall, rescue and network repair), add a Settings area with update controls and an advanced mode, and refine the macOS app — while Jay is travelling and testing draft builds remotely. Windows is parked, not deleted.

Every phase ends with a **draft release** (see `docs/RELEASING.md`) and a "what to test" list for Jay's travel Mac.

---

## Phase 0 — Foundation: macOS focus, one-command build, private dev releases
**Skills:** build-release, macos-helper · plugin: /feature-dev

**Deliverables**
- `dev` branch created from `main` and pushed (`git push -u origin dev`).
- Windows parked: `service-tool/`, `installer-iss/` and `src-tauri/src/platform/windows.rs` moved (git mv, owner-approved) under `platforms-parked/windows/` with a README explaining how to restore; `platform/mod.rs` still compiles on macOS; Windows conf/scripts no longer referenced by macOS builds; `bump-version.sh` updated (no `.iss`/service-tool entries).
- Stale root `Uninstall Script.command` (old `com.ipswitcher` ids) flagged in PROGRESS for removal in Phase 1 (don't delete yet).
- `package.json` script `build:web` (`vite build`).
- `scripts/build-macos.sh` (one command → verified universal `.pkg` in `release-out/`) and `scripts/verify-pkg.sh`.
- Build stamp (git SHA, dirty flag, UTC time) compiled into the app and exposed through a Tauri command.
- `CommandRunner` abstraction in `src-tauri` (macOS platform module) with a test fake; `IHWIP_NETWORKSETUP` override honoured in debug builds only.
- First unit tests: `get_ip_info` parsing against `tools/fake-networksetup/fixtures/default`, service-list parsing incl. `*` disabled prefix.
- `.gitignore`: `release-out/`, `.claude/settings.local.json`.
- First draft release of current behaviour built by the new pipeline.

**Acceptance**
- [ ] `cargo test --manifest-path src-tauri/Cargo.toml` passes with ≥ 4 parsing tests
- [ ] `cargo test --manifest-path helper-tool/Cargo.toml` passes (even if 0 tests yet, the crate builds)
- [ ] `cargo clippy --manifest-path src-tauri/Cargo.toml -- -D warnings` passes
- [ ] `./scripts/build-macos.sh` exits 0 and prints a `.pkg` path + SHA-256
- [ ] `./scripts/verify-pkg.sh release-out/*.pkg` exits 0 and reports `x86_64 arm64` for app and helper
- [ ] `git grep -n "service-tool\|installer-iss" -- scripts build-pkg.sh src-tauri/tauri.conf.json` returns nothing
- [ ] `gh release view <draft tag> --json isDraft --jq .isDraft` prints `true`

---

## Phase 1 — Full uninstaller + rescue script (ship ASAP)
**Skills:** network-recovery, build-release, app-ui · plugin: /feature-dev

**Deliverables**
- `recovery/ithurtswhenip-rescue.command`: interactive menu + flags (`--plan`, `--uninstall`, `--network …`, `--yes`, `--wait-pid`, `--log`), full uninstall inventory incl. legacy `com.ipswitcher.*`, per-user data, pkg receipts, leftover report; network revert options (journal if present / pick services → DHCP / leave), backups to `/Users/Shared/ItHurtsWhenIP-backups/<ts>/`, verify step. All system paths overridable for tests.
- Script bundled into the app (`Contents/Resources/`) by `build-pkg.sh` (replacing the heredoc `uninstall.command`), and attached to every release by `release-dev.sh` (already supported).
- In-app **Uninstall…** (app menu, and a temporary button until Settings exists): shows the plan, confirms, runs the script with a macOS admin prompt, app quits, script removes the bundle after the app exits.
- Root `Uninstall Script.command` removed (owner-approved).
- `docs/ROAD-TESTING.md` uninstall section updated with real steps.
- Test harness: fake filesystem root + fake `launchctl`/`pkgutil`/`networksetup`, golden `--plan` outputs.

**Acceptance**
- [ ] `shellcheck recovery/ithurtswhenip-rescue.command` and `bash -n` pass
- [ ] Script test suite passes (fake root, no sudo): full install tree → after `--uninstall --yes` nothing from the inventory remains, backups exist, leftovers reported
- [ ] Legacy-only tree (old `com.ipswitcher` files) is cleaned by the same run
- [ ] With no journal, `--plan` lists Manual services and selects none by default
- [ ] `--plan` changes nothing (tree checksum identical before/after)
- [ ] `verify-pkg.sh` confirms the rescue script is inside the app bundle
- [ ] Draft release has `.pkg`, `.sha256` and `ithurtswhenip-rescue.command` assets
- [ ] Owner confirms on travel Mac: install → use → uninstall leaves no files and a working network (owner-run, recorded in PROGRESS)
- [ ] Owner decision recorded: publish as v1.14.0 to colleagues now, or after Phase 2

---

## Phase 2 — Helper hardening + change journal
**Skills:** macos-helper, network-recovery

**Deliverables**
- Socket mode `0660 root:admin` (or peer-credential check, owner-approved), never 0777.
- `CommandRunner` in the helper; strict validation of service names (must exist), IPv4, contiguous masks, optional gateway; argv only; stdout+stderr error capture.
- App-side: `get_ip_info`/`get_interfaces` without `sh -c`; disabled `*` services handled.
- Protocol v2 with `ping` (helper + protocol version) and backward-compatible v1 commands.
- Change journal (`/Library/Application Support/com.ithurtswhenip.helper/journal.json`, schema 1): original config recorded before first change per service; `journal_status`, `restore_service` commands. Rescue script reads it.
- App shows a clear banner if the helper is missing, not running, or a different version.

**Acceptance**
- [ ] Helper tests: invalid service / IP / mask / injection strings (`'; rm -rf /'`, newline, quotes) are rejected and run no command (fake log empty)
- [ ] Helper tests: first `set_static_ip` on a service writes its original to the journal; second change doesn't overwrite it; `restore_service` issues the right `networksetup` argv
- [ ] `git grep -n 'sh", *"-c\|Command::new("sh")' -- src-tauri helper-tool` returns nothing
- [ ] `git grep -n '777' -- helper-tool` returns nothing
- [ ] v1 requests still succeed in tests; unknown version gets `unsupported_version`
- [ ] Rescue script test: journal fixture → plan offers "restore original" for exactly the journaled services
- [ ] Draft release; owner confirms quick sets still work on travel Mac and `--plan` shows journal entries after a change

---

## Phase 3 — Network repair ("get me back to normal")
**Skills:** network-recovery, macos-helper, app-ui

**Deliverables**
- Rescue script `--repair soft|service:<name>|full` with plan/confirm/backup/verify; `--restore-backup <dir>` for tier 3.
- In-app **Repair network…** flow (tier picker with plain-language consequences, plan, confirm, result list, "restart needed" handling).
- `docs/ROAD-TESTING.md` repair section + manual Terminal fallback steps (no app needed).

**Acceptance**
- [ ] Script tests with fakes: soft repair logs DNS flush + DHCP renew only for DHCP services; service reset logs the expected argv sequence; full reset moves exactly the listed plists into the backup dir and nothing else
- [ ] `--restore-backup` puts the files back (checksums match)
- [ ] Full reset refuses without `RESET` confirmation (or `--yes` from the app after its own typed confirm)
- [ ] `shellcheck` clean
- [ ] Draft release; owner runs soft + service reset on travel Mac and records result (full reset only if owner chooses to)

---

## Phase 4 — Settings + advanced mode
**Skills:** app-ui · plugin: /feature-dev

**Deliverables**
- Settings panel (gear + ⌘, + app menu) with General, Updates (placeholder until Phase 5), Maintenance (Repair, Uninstall, Copy diagnostics), Advanced.
- Advanced mode toggle (confirm to enable, persisted) and the advanced features the owner picks at the start of this phase.
- CSS tokens + dark-mode support for new UI.
- Config migration: new `AppConfig` fields with defaults; old config files load unchanged.

**Acceptance**
- [ ] `cargo test` covers: old config JSON (v1.13.1 shape) loads with new defaults; advanced flag round-trips
- [ ] `npm run build:web` passes
- [ ] Uninstall/Repair entries moved into Settings → Maintenance; temporary button removed
- [ ] Draft release + owner visual check list (480/800 px, light/dark) recorded

---

## Phase 5 — Updates in Settings
**Skills:** app-ui, build-release

**Deliverables**
- Settings → Updates: current version + build stamp, latest version, last checked time, **Check now**, **Update** (downloads the `.pkg` to `~/Downloads` with progress, verifies size/checksum if published, opens it in Installer), **View changelog**.
- Works regardless of the 24 h popup suppression; launch popup unchanged ("Remind me later" behaviour kept).
- Graceful states: offline, GitHub rate limit (403/429), no `.pkg` asset.

**Acceptance**
- [ ] Rust tests for version comparison incl. pre-release/odd tags (`dev-…` tags never count as newer), asset selection, error mapping
- [ ] Update check uses only published releases (unauthenticated `/releases/latest`), never drafts
- [ ] Draft release; owner tests "Check now" + "Update" against a published test scenario chosen with the owner

---

## Phase 6 — Backlog polish (from the project notes)
**Skills:** app-ui, macos-helper, build-release

**Deliverables**
- Refresh button next to the interface dropdown.
- Ignore a double-press of a quick set (don't overwrite "previous IP" with the quick set itself).
- Background refresh to catch IP changes made outside the app (low-frequency poll + on window focus).
- Manual IP section highlights blue when its values are the current config.
- `bundle.category` set so macOS gets `LSApplicationCategoryType` (`public.app-category.utilities`; TODO(verify) whether `developer-tools` fits better — owner's call).

**Acceptance**
- [ ] Rust/JS logic tests for double-press guard and "is current" comparison
- [ ] `verify-pkg.sh` shows `LSApplicationCategoryType` in Info.plist
- [ ] Background refresh interval ≥ 5 s and paused while a change is in flight (code review + test)
- [ ] Draft release + owner check list

---

## Phase 7 — Release candidate and publish
**Skills:** build-release, network-recovery

**Deliverables**
- Version bump, CHANGELOG entry, README update (uninstall/rescue instructions), release notes for colleagues.
- Merge `dev` → `main`, published release `vX.Y.Z` with `.pkg` + rescue script (each step owner-approved).

**Acceptance**
- [ ] All tests, clippy, `verify-pkg.sh` pass on the release commit
- [ ] Owner has run install → quick sets → repair soft → uninstall on the travel Mac with the RC build
- [ ] Published release visible at `https://api.github.com/repos/jaylenjupp/it-hurts-when-ip/releases/latest` with the new tag
- [ ] A v1.13.1 install sees the update prompt (owner-checked)

---

## Later ideas (not scheduled)
- Bring Windows back from `platforms-parked/` with the same uninstall/rescue/journal model (Winsock/TCP-IP reset tiers).
- Developer ID signing + notarization (removes the Gatekeeper prompt).
- Migrate the LaunchDaemon to `SMAppService` (macOS 13+) if the minimum macOS is raised.
- Menu-bar mode, per-service quick sets, import/export of quick sets.
- GitHub Actions macOS runner for CI builds (so builds don't depend on the Mac Studio being awake).
