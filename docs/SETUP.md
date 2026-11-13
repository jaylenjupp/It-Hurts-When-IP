# Setup: Mac Studio build host

What the setup session did on **2026-11-13**, how the pre-commit hook works, and how to redo setup on a new machine. Every command block says where it runs. Nothing here changes live network state (CLAUDE.md core rule 1).

**tmux:** not installed, and the owner decided (2026-11-13) it isn't needed: the Terminal window stays open on the Mac Studio while he travels. Skip the `tmux` lines in `docs/REMOTE-SETUP.md` step 4 and run `claude --remote-control "It Hurts When IP"` directly. Closing that Terminal window ends the session.

## 1. What was done (2026-11-13)
```bash
# Mac Studio, repo root
git switch -c dev                      # from main @ 20b1ec9; main untouched
git config core.hooksPath .githooks    # local .git/config only, not versioned
```
- `.gitignore`: added `release-out/` and `.claude/settings.local.json`.
- `.githooks/pre-commit` added (see §3).
- Plugins installed at **project scope**. The `claude-plugins-official` marketplace was already registered. The orchestrator ran the commands and the owner approved each prompt:
  ```bash
  # Mac Studio, repo root
  claude plugin install commit-commands@claude-plugins-official --scope project
  claude plugin install feature-dev@claude-plugins-official --scope project
  ```
  Recorded in `.claude/settings.json` → `enabledPlugins` (both v1.0.1). Check with `claude plugin list` (Mac Studio, repo root). In a running session, `/reload-plugins` loads them.
- Read-only `networksetup` fixtures captured to `tools/fake-networksetup/fixtures/mac-studio/`. Its `README.md` has the commands and redaction rules.
- **Nothing installed** via Homebrew, rustup or npm. The newest Homebrew keg is from 10 Nov, the x86_64 Rust target from 7 Nov, and `node_modules/` from May.
- After this doc: the kit is committed on `dev`, `git push -u origin dev` runs, and a `release-dev.sh --dry-run` proves the release path. Results are in `docs/PROGRESS.md`.

## 2. Toolchain (verified 2026-11-13, Mac Studio)
| Tool | Version / state | Status |
|---|---|---|
| macOS | 26.7.1 (25G241), arm64 | ok |
| Dev tools | Command Line Tools, `/Library/Developer/CommandLineTools` (selected) | ok |
| Xcode.app | present, **not selected**, so `tauri info` prints "Xcode: not installed" | ok for now |
| lipo · pkgbuild · codesign | lipo from CLT; pkgbuild + codesign in `/usr/bin` | ok |
| rustc / cargo | 1.99.1 (rustup 1.29.3, Homebrew) | ok |
| Rust targets | `aarch64-apple-darwin` + `x86_64-apple-darwin` | ok, universal builds possible |
| node / npm | v26.13.0 / 11.20.0 (Homebrew) | ok |
| Tauri (`tauri info`) | tauri 2.11.2, tauri-build 2.6.2, @tauri-apps/cli 2.11.2, api 2.11.0, plugin-os 2.3.2, plugin-opener 2.5.4 | behind latest, no action |
| gh | 2.103.3, logged in as jaylenjupp (keychain) | ok |
| claude | 2.1.323 | ok |
| tmux | not installed | not needed (owner decision: Terminal stays open) |
| shellcheck | not installed | optional; hook skips lint |

- Latest per `tauri info`: tauri 2.12.5, tauri-build 2.7.4, cli/api 2.12.4, plugin-os 2.4.0, plugin-opener 2.7.3. Updating them is a dependency change, so it's the owner's call.
- Only switch to full Xcode if a full build needs it. It needs sudo, so the **owner does it at the Mac**:
  `sudo xcode-select -s /Applications/Xcode.app/Contents/Developer` (Mac Studio, Terminal).

Re-check any time (read-only):
```bash
# Mac Studio, repo root
sw_vers; xcode-select -p; rustc --version; rustup target list --installed
node -v; npm -v; gh --version; gh auth status; claude --version
command -v tmux shellcheck
npm run tauri info
```

## 3. Pre-commit hook (`.githooks/pre-commit`)
Runs on every `git commit` (Mac Studio, repo root). It only reads files and never changes anything. Checks, in order:
1. **Branch guard** (runs first and stops at once): refuses commits on `main`.
2. **Secret-looking filenames**: refuses staged files named `.env`, `*.pem` or `*.p12`, or with `token` anywhere in the name (any case). **No override.**
3. **`cargo fmt --check`** for each crate (`src-tauri`, `helper-tool`, `service-tool`) with staged `.rs` changes. Skipped with a warning if cargo isn't on PATH.
4. **`shellcheck`** on staged `*.sh` / `*.command` files, if it's installed. Otherwise it prints a one-line note.

Checks 2–4 always all run, and every failure is listed before the commit is refused. fmt and shellcheck read the working-tree copy of each file, not the staged copy.

Env vars:
- `ALLOW_MAIN=1`: **owner-only** override for release merges on `main`. Never set it in automation, and Claude never sets it.
- `IHWIP_HOOK_BRANCH=<name>`: makes the hook act as if you're on `<name>`. For testing.

Test the guard safely (makes no commit):
```bash
# Mac Studio, repo root
IHWIP_HOOK_BRANCH=main .githooks/pre-commit; echo $?   # refusal message, then 1
```
Gotchas:
- Any filename containing `token` is blocked with no override, e.g. a future `tokens.css`. Rename the file or ask the owner.
- Only the exact name `.env` is caught. `.env.local` and similar get through.
- Hooks are **per clone**. `core.hooksPath` lives in `.git/config`, which isn't versioned, so every new clone needs `git config core.hooksPath .githooks`.

## 4. Redo setup on a new machine
1. Clone, switch to `dev`, enable hooks:
   ```bash
   # new machine, Terminal
   git clone https://github.com/jaylenjupp/it-hurts-when-ip.git && cd it-hurts-when-ip
   git switch dev
   git config core.hooksPath .githooks
   ```
2. Install the tools from `docs/REMOTE-SETUP.md` §1. The Mac Studio gets node, rustup and gh from Homebrew. Homebrew's rustup is keg-only, so `/opt/homebrew/opt/rustup/bin` must be on PATH.
   ```bash
   # new machine, Terminal
   xcode-select --install                    # Command Line Tools
   brew install node rustup gh               # + tmux, shellcheck (both optional)
   rustup default stable                     # TODO(verify) needed with Homebrew rustup
   rustup target add aarch64-apple-darwin x86_64-apple-darwin
   gh auth login                             # GitHub.com → HTTPS → browser; saved in keychain
   ```
3. Install JS deps: `npm ci` (new machine, repo root).
4. Plugins: the project-scope plugins are listed in `.claude/settings.json`. Claude Code should offer to install them when you trust the folder (TODO(verify) on a fresh machine). If it doesn't, run the two `claude plugin install …` commands from §1 (new machine, repo root), then `/reload-plugins`. If the marketplace is missing: `claude plugin marketplace add anthropics/claude-plugins-official`.
5. Verify:
   ```bash
   # new machine, repo root
   rustup target list --installed                         # both macOS targets
   npm run tauri info                                     # env + package versions
   IHWIP_HOOK_BRANCH=main .githooks/pre-commit; echo $?   # 1
   ./scripts/ship/release-dev.sh --help                   # usage text, exit 0
   claude plugin list                                     # both plugins, project scope
   ```
6. For remote use, follow `docs/REMOTE-SETUP.md` §2–4.
7. **Core rule 1 applies on every build host:** never change live network state there. Use `tools/fake-networksetup` and `--dry-run`.

## 5. Known gaps / follow-ups
- **tmux not installed:** by owner decision, see the note at the top.
- **shellcheck** not installed. It's optional; the hook skips lint without it.
- **`.DS_Store` files are tracked** even though `.gitignore` lists them: `.DS_Store`, `icon/.DS_Store`, `icon/ip-switcher.iconset/.DS_Store`, `src-tauri/.DS_Store`. Untracking them (`git rm --cached`) needs the owner's OK. `.gitignore` also lists `.DS_Store` three times (cosmetic).
- **Fake vs real for an unknown service:** real `-getinfo` exits **4** and prints two lines on stdout (`<name> is not a recognized network service.` then `** Error: The parameters were not valid.`). The fake exits **1** and prints only the second line. Fix in Phase 0/2.
- **The fake doesn't seed an existing empty state dir:** `IHWIP_FAKE_STATE=$(mktemp -d)` gives a `cat` error and still exits 0. Pass a path that doesn't exist yet (`"$(mktemp -d)/state"`) or set `IHWIP_FAKE_RESET=1`.
- **`release-dev.sh` paths:** run it from the repo root, with paths that are valid from there (or absolute). It checks the pkg/notes paths *before* it changes to the root, so relative paths from a subfolder break, even though its help says "from anywhere inside the repo". Changes under `scripts/ship/` need owner approval.
- Tauri crates and packages are one or two minor versions behind latest. Updating them is the owner's call.
