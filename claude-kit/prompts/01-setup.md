## Your role in this session
You are the **orchestrator**, running on Fable. You plan, advise and brief the **`builder` subagent** (Opus, `.claude/agents/builder.md`), review, and commit. You don't edit application code yourself. Follow "How sessions work" in `CLAUDE.md`.

## Objective
Get the repo ready for remote development: git branch and hooks, Claude Code plugins, verified toolchain and release path, and a short `docs/SETUP.md`. **No application code changes in this session.**

## Context
- I'm Jay, the owner. I'm about to travel for a few weeks and will drive this session from my phone via Remote Control. Keep messages short; put anything I must decide at the top.
- This repo is `jaylenjupp/it-hurts-when-ip` (public on purpose: the in-app update checker reads its published releases). I've copied the kit seed from `claude-kit/repo-seed/` into the repo root. Read `CLAUDE.md` first, then `docs/PLAN.md`, `docs/PROGRESS.md`, `docs/REMOTE-SETUP.md`, `docs/RELEASING.md` and `reference/AUDIT-2026-11.md`.
- Everything runs on the **Mac Studio** (this machine). Say where each command runs anyway.
- `gh` should already be authenticated (`gh auth status`).
- **Core rule 1 applies from now on:** nothing that changes live network state on this Mac, ever.

## Target State
1. Seed files present at the root and identical to `claude-kit/repo-seed/` (`diff -r`), scripts executable.
2. `.gitignore` also ignores `release-out/` and `.claude/settings.local.json`.
3. Branch `dev` created from `main`, kit committed on `dev`, `dev` pushed to origin. `main` untouched.
4. Git hooks in `.githooks/` with `git config core.hooksPath .githooks`:
   - `pre-commit`: refuse commits on `main` (unless `ALLOW_MAIN=1`); run `cargo fmt --check` for crates with staged `.rs` changes; run `shellcheck` on staged `*.sh`/`*.command` if shellcheck is installed; refuse staged files matching `.env`, `*.pem`, `*.p12`, `*token*`.
5. Plugins installed at **project scope**, each confirmed with `claude plugin list`:
   - `commit-commands@claude-plugins-official` (provides `/commit-commands:commit`)
   - `feature-dev@claude-plugins-official` (provides `/feature-dev`)
   Install command (Mac Studio, repo root): `claude plugin install <name>@claude-plugins-official --scope project`. If the official marketplace isn't registered: `claude plugin marketplace add anthropics/claude-plugins-official`. Source: https://code.claude.com/docs/en/plugins/install
6. Toolchain verified and recorded: Rust targets `aarch64-apple-darwin` + `x86_64-apple-darwin`, node/npm versions, `npm run tauri info` output summary, `tmux`, `gh`, `shellcheck` (optional).
7. Real read-only `networksetup` output from this Mac captured into `tools/fake-networksetup/fixtures/mac-studio/` (same layout as `fixtures/default/`). Only `-listallnetworkservices`, `-listallhardwareports`, `-getinfo "<service>"`. Redact MAC addresses to `xx:xx:…`.
8. Release path proven without a real build: `./scripts/ship/release-dev.sh --help` and a `--dry-run` with a placeholder `.pkg` in a temp dir succeed on `dev` after the push.
9. `docs/SETUP.md`: what was installed (with the exact commands and where they ran), hook behaviour, how to re-run setup on a new machine.
10. `docs/PROGRESS.md` updated: setup done, next step = `claude-kit/prompts/02-kickoff.md`.

## Work packages
1. **Audit the seed** — builder: check files, `diff -r claude-kit/repo-seed .` limited to the seed's paths, `chmod +x` on `scripts/ship/release-dev.sh` and `tools/fake-networksetup/fake-networksetup`, `bash -n` on both, smoke-test the fake (`-listallnetworkservices`, `-setmanual` then `-getinfo`, `fake-log`) with a temp `IHWIP_FAKE_STATE`. Report `git status`, `git log --oneline -3`, `gh repo view --json visibility,defaultBranchRef`.
2. **Git local** — builder: create `dev`, `.gitignore` additions, `.githooks/pre-commit` (make the branch check overridable for testing, e.g. `IHWIP_HOOK_BRANCH=main .githooks/pre-commit` must exit non-zero; never create a real commit on `main` to test it), `git config core.hooksPath .githooks`.
3. **Official plugins** — you (orchestrator) run the two `claude plugin install … --scope project` commands; I'll approve each prompt. Then tell me to run `/reload-plugins`, or do it if you can.
4. **Toolchain + fixtures** — builder: item 6 and 7 above (read-only commands only).
5. **Verify + document** — builder writes `docs/SETUP.md`; you review.
6. **Commit** — `/commit-commands:commit` on `dev` (once plugins load; if not loaded yet, a normal Conventional Commit `chore: add Claude Code kit and dev tooling`).
7. **Push `dev`** — `git push -u origin dev` (pre-approved). Then run the release-dev dry run (item 8).
8. **Seed cleanup** — ask me before removing `claude-kit/repo-seed/` (it's duplicated at the root once committed). `claude-kit/README.md` and `claude-kit/prompts/` stay.

## Advising the builder
The builder will pause with `Result: ADVICE NEEDED` for approach checks, open design choices, repeated failures or conflicting guidance. Answer via `SendMessage` to the same builder with the decision, a one-line reason, and any constraint to watch. Be decisive. If the question is mine to decide, ask me first, then relay my answer. Advice rounds don't count toward the 2 revision rounds.

## Review each package
Read the builder's report, re-run its verification commands yourself, and check against `CLAUDE.md`. Fixes go back to the same builder as a numbered list. After 2 failed rounds, stop and ask me.

## Constraints
- No application code (`src/`, `src-tauri/`, `helper-tool/`, `service-tool/`, `build-pkg.sh` stay untouched).
- Never change repo visibility. Never commit secrets. Don't touch `reference/`.
- No third-party plugins in this session.

## Action Boundaries
Stop and ask me before: any push other than `git push -u origin dev`, installing plugins (I'll approve each), deleting files, new dependencies or Homebrew installs, anything touching `main`, anything that changes network state.

## Done When
- [ ] `git branch --show-current` → `dev`; `git log origin/dev --oneline -1` shows the kit commit
- [ ] `git config core.hooksPath` → `.githooks`; hook refuses `main` when run directly with a simulated branch
- [ ] `claude plugin list` shows both plugins at project scope; `.claude/settings.json` has them under `enabledPlugins`
- [ ] `rustup target list --installed` shows both macOS targets
- [ ] `fixtures/mac-studio/` exists with redacted real output
- [ ] `release-dev.sh --dry-run` prints a `dev-v1.13.1-…` tag and exits 0
- [ ] `docs/SETUP.md` and `docs/PROGRESS.md` committed and pushed on `dev`

Finish with: a 5-line summary, anything I need to do at the Mac before I leave (e.g. `/reload-plugins`, `/config` push notifications, energy settings from `docs/REMOTE-SETUP.md`), and the next step ("`/clear`, then: Read `claude-kit/prompts/02-kickoff.md` and run it"). Then stop.
