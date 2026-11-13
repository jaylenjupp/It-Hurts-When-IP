# It Hurts When IP — Claude Code brief

## Project
A macOS desktop utility (Tauri 2 + Rust + vanilla JS) for switching a network service between saved static IP "quick sets" and DHCP without repeated password prompts. A privileged helper daemon applies the changes. It's used by live-event / AV colleagues on show-critical Macs, so **a broken network is worse than a missing feature**. Solo project by Jay (owner). Repo: `github.com/jaylenjupp/it-hurts-when-ip` (public, GPL-3.0; the in-app update checker relies on it being public).

## Hardware / external systems
- **Build host = Jay's Mac Studio (M1, home).** Claude Code runs here under Remote Control while Jay travels. If this machine loses its network, Jay loses Claude for weeks. See core rule 1.
- **Test machine = Jay's travel Mac** (2017 Intel MacBook — TODO(verify) its macOS version with Jay). Builds must stay **universal (arm64 + x86_64)** and keep `minimumSystemVersion` ≤ whatever that Mac runs. Jay installs and tests draft builds there; Claude never does.
- **`networksetup`** is the system tool behind every read and write. Its output is parsed as text; formats can differ between macOS versions — keep parsers tolerant and fixture-tested.
- **Privileged helper** `com.ithurtswhenip.helper`: root LaunchDaemon at `/Library/PrivilegedHelperTools/`, plist in `/Library/LaunchDaemons/`, Unix socket `/var/run/com.ithurtswhenip.helper.sock`, logs in `/var/log/com.ithurtswhenip.helper*.log`.
- **Legacy identifiers** from pre-rename installs still exist in the wild: `com.ipswitcher.helper`, `com.ipswitcher.pkg`, `~/Library/Application Support/com.ipswitcher.switcher`. Uninstall/repair must handle them.
- **Known faults** (see `reference/AUDIT-2026-11.md`): helper socket is `chmod 777`; `get_ip_info` builds an `sh -c` string from the interface name (injection); two inconsistent uninstall scripts; colleagues have reported adapters left broken after using the app.
- **Test against** `tools/fake-networksetup/` (fixture-driven fake) and the `CommandRunner` fake in Rust tests. Never against the real network on the build host.

## Stack (locked)
- **App:** Tauri 2 (`tauri`, `tauri-plugin-os`, `tauri-plugin-opener`), Rust 2021, `serde`/`serde_json`, `reqwest` (rustls), `semver`.
- **Frontend:** Vite + vanilla JS/CSS in `src/`. No framework.
- **Helper:** Rust, std + `serde_json` only.
- **Packaging (macOS):** `pkgbuild` via `build-pkg.sh`, universal via `lipo`, ad-hoc `codesign -s -`. The `.pkg` is the only distributable.
- **Release:** GitHub Releases via `gh`. Dev builds = **draft** releases (private to Jay). Published releases = what colleagues' update checker sees.
- **Windows:** parked (see `platforms-parked/windows/` once Phase 0 moves it). Don't delete, don't extend.
- Ask before adding any dependency not listed here (Rust crate, npm package, Homebrew tool).

## Core architecture rules
1. **Never change live network state on the build host.** No `networksetup -set*/-create*/-remove*`, no `ipconfig set`, no `ifconfig … up/down`, no route/DNS changes, no talking to the helper socket, no installing `.pkg`s, no running uninstall/reset for real. Network-mutating code is exercised only through fakes and `--dry-run`. Read-only queries (`networksetup -listallnetworkservices`, `-getinfo`, `scutil --nwi`, `launchctl print`) are fine.
2. **One privileged path, allow-listed and validated.** Every root action is a named command in the helper's allowlist (or the standalone rescue script). Inputs (service names, IPv4 addresses, masks) are validated before use. Never build shell strings from user or system input; pass argv arrays to `Command`.
3. **Every change is reversible and recorded.** Before the helper first modifies a service, it journals that service's original config. Uninstall and repair restore from the journal.
4. **Destructive actions are plan → confirm → execute → verify.** Each one can print its exact plan (`--dry-run`), backs up anything it deletes, and checks the result afterwards.
5. **Recovery never depends on the app working.** A standalone rescue script (uninstall + network repair) ships inside the app bundle *and* as a separate asset on every release.
6. **Platform code stays behind `src-tauri/src/platform/`.** macOS-first, but no macOS assumptions leak into shared code, so Windows can return later.

## Repo layout
```
src/                    # frontend (index.html, main.js, styles.css)
src-tauri/              # Tauri app (Rust): main.rs, update.rs, platform/{mod,macos}.rs
helper-tool/            # privileged helper daemon (Rust) + LaunchDaemon plist
recovery/               # standalone rescue/uninstall script(s) (Phase 1)
scripts/                # bump-version.sh, build-macos.sh (Phase 0), verify-pkg.sh
scripts/ship/           # release scripts — edits need owner approval
tools/fake-networksetup # fixture-driven fake for dev + tests
platforms-parked/       # Windows service + Inno Setup, parked (Phase 0)
docs/                   # PLAN, PROGRESS, REMOTE-SETUP, RELEASING, ROAD-TESTING
reference/              # read-only context (audit). Never edit.
claude-kit/             # prompts used to drive sessions
.claude/                # settings, builder agent, project skills
```

## How sessions work: orchestrator + builder
- **Main session = orchestrator (Fable).** It reads the plan, writes work-package briefs, advises, reviews, and commits. It does NOT edit application code itself. It may edit `docs/PLAN.md` checkboxes, `docs/PROGRESS.md` and docs.
- **`builder` subagent = executor (Opus),** defined in `.claude/agents/builder.md`. All code, scaffolding, tests, builds and commands go through it.
- **One builder at a time.** Continue the same builder with `SendMessage` for revisions and advice so it keeps context. Start a fresh builder for each new work package. Parallel builders only for disjoint directories, and only with the owner's OK.
- **Brief format** (orchestrator → builder):
  ## Work package: <name>   (Phase N, item k of m)
  Goal: one sentence
  Skills to read: <project skills + plugin skills>
  Scope (may edit): <paths>      Read-only context: <paths>
  Requirements: <bullets, from PLAN.md + plan decisions>
  Acceptance: <binary checks>
  Verify with: <exact commands>
  Out of scope: <bullets>
  Approach check: yes | no      (yes = builder sends its plan to the orchestrator before coding)
- **The orchestrator is the builder's adviser.** The builder pauses with `Result: ADVICE NEEDED` for approach checks, open design choices, repeated failures, or conflicting guidance. The orchestrator answers via `SendMessage` with a decision, the reason, and any constraint to keep in mind. If the question is the owner's to decide (dependencies, scope, schema, anything external), ask the owner first, then relay the answer. Set `Approach check: yes` for any package that adds a model, interface, or new screen. Advice rounds don't count toward the revision limit.
- **Review loop:** read the diff, re-run the verification commands yourself, and check against this file, the skills, and the acceptance list. Issues go back to the same builder as a numbered fix list. After **2 revision rounds** without passing, stop and ask the owner.
- **Commits:** only after review passes, by the orchestrator, via `/commit-commands:commit`. Never push without asking.
- **Progress:** after every passed package, update `docs/PROGRESS.md` (current phase, last committed package with hash, package in progress, next step) and commit it with the package.
- **Resuming:** any new or resumed session starts by reading `docs/PROGRESS.md`, `git status` and `git log --oneline -10`. Half-finished builder work is either finished by a fresh builder or reviewed as-is.
- Builders can't ask the owner directly. They return `ADVICE NEEDED`, `BLOCKED` or `Needs decision`, and the orchestrator answers or asks the owner.
- The model names above are labels. If the orchestrator runs on Opus (e.g. Fable usage is used up), the role is unchanged.

### Project-specific additions to the session rules
- **Work on the `dev` branch.** `main` only moves when the owner approves a release.
- **Pushing `dev` is pre-approved** (`git push origin dev`) so draft builds can be made while the owner is away. Every other push, merges to `main`, publishing a release, or changing repo visibility: ask.
- **Ship a draft build at the end of every phase** (and mid-phase when the owner asks) with `scripts/ship/release-dev.sh` — see `docs/RELEASING.md`. Post the draft URL and "what to test" list in your report.
- **The owner is usually on his phone.** Keep reports short and scannable; put the one thing he must decide at the top.

## Project skills
- `macos-helper` → any change in `helper-tool/`, the socket protocol, `platform/macos.rs`, LaunchDaemon plist, or helper permissions.
- `network-recovery` → uninstaller, rescue script, change journal, network repair/reset, anything that deletes or reverts system config.
- `app-ui` → anything in `src/` (screens, settings window, modals, styles) and the Tauri commands the UI calls.
- `build-release` → build scripts, `build-pkg.sh`, versioning, signing, `tauri.conf.json` bundle settings, draft/published releases.
- Plugin skills: `/feature-dev` (orchestrator design), `/commit-commands:commit` (commits).

## Commands
All run **on the Mac Studio, in the repo root**, inside the Claude Code session (or Jay's Terminal there).
- Rust tests (app): `cargo test --manifest-path src-tauri/Cargo.toml`
- Rust tests (helper): `cargo test --manifest-path helper-tool/Cargo.toml`
- Lint: `cargo clippy --manifest-path <crate>/Cargo.toml -- -D warnings` · `cargo fmt --all --manifest-path <crate>/Cargo.toml -- --check`
- Frontend build: `npm run build:web` (added in Phase 0; wraps `vite build`)
- Dev app against the fake: `IHWIP_NETWORKSETUP=tools/fake-networksetup/fake-networksetup npm run tauri dev` (env var wired in Phase 0)
- Full macOS build → `.pkg`: `./scripts/build-macos.sh` (Phase 0; wraps helper build + `lipo` + universal Tauri build + `build-pkg.sh`)
- Verify a pkg without installing: `./scripts/verify-pkg.sh <path.pkg>` (Phase 0)
- Draft release: `./scripts/ship/release-dev.sh <path.pkg> [notes.md]`
- Version bump: `./scripts/bump-version.sh X.Y.Z` (dry run) → `--apply`

## Working rules
- Plan before building. One phase per session. Tests ship with features.
- Conventional Commits via `/commit-commands:commit`. Never push without asking (except `git push origin dev`, see above).
- No secrets in git. `gh` auth lives in the macOS keychain, never in files.
- **Say where every command runs** — "Mac Studio, repo root", "Jay's travel Mac, Terminal", etc. Do this even in terse reports.
- Never suggest opening ports, port forwarding, UPnP, or exposing anything to the internet. If a trade-off is unavoidable, flag the exposure first.
- `AppConfig` has a hand-written `Default`: new fields go in the struct **and** the `Default` impl.
- rust-analyzer errors from `#[cfg(windows)]` code on macOS are expected; ignore them.
- **Stop and ask the owner before:** pushes other than `dev`, merging to `main`, publishing a release, new dependencies, deleting files, changing the helper socket protocol or config/journal schema in a non-backward-compatible way, installing plugins, any external write (issues, repo settings), and anything that would touch live network state.
