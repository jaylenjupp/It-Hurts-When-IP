## Your role in this session
You are the **orchestrator**, running on Fable. You plan, advise, brief the **`builder` subagent** (Opus, `.claude/agents/builder.md`), review, and commit. You don't edit application code yourself. Follow "How sessions work" in `CLAUDE.md`.

## Session Strategy
New session (`/clear` first). Start in plan mode. No delegation until I approve the plan. I'm on my phone: keep the plan scannable — one line per work package, details underneath.

## Objective
Deliver **Phase 0** (foundation: macOS focus, one-command verified build, private draft releases) and **Phase 1** (full uninstaller + rescue script) from `docs/PLAN.md`. At the end I can download a draft build on my travel Mac that installs, works as before, and uninstalls itself completely — plus a standalone rescue script that works without the app.

## Context
- Read `CLAUDE.md`, `docs/PROGRESS.md`, `docs/PLAN.md` (Phases 0–1), `reference/AUDIT-2026-11.md`, `docs/RELEASING.md`, `docs/ROAD-TESTING.md`. Run `git status` and `git log --oneline -15`. Confirm you're on `dev`.
- Skills for these phases: `build-release`, `macos-helper`, `network-recovery`, `app-ui`.
- Test against `tools/fake-networksetup/` (fixtures `default` and `mac-studio`) and a fake filesystem root for the rescue script. **Never** run network-changing commands, `sudo`, `installer`, or the real uninstall/rescue on this Mac.
- Use `/feature-dev` for your own exploration and design, then turn the design into builder briefs.
- Uninstall is urgent: colleagues have reported broken adapters. Prioritise getting a correct, well-tested rescue script into a draft build over polish.

## Plan (what I approve)
1. **Structure:** where new code lives (`recovery/`, `scripts/`, `platform/macos.rs` changes, test layout).
2. **Key interfaces:** `CommandRunner` trait; rescue script CLI (flags, exit codes, env overrides for tests); how the app invokes the script with an admin prompt and quits; build-stamp command.
3. **Decisions you need from me** (list them, with your recommendation), e.g. the exact `platforms-parked/` move, deleting the old root `Uninstall Script.command`, rescue script flag names, backup location.
4. **Ordered work packages**, each with: goal, skills, scope paths, acceptance checks, verify commands, Approach check yes/no. Small enough to review in one diff. Include the Phase 0 draft release as its own package and the Phase 1 draft release as the last.

## Target State
Every Phase 0 and Phase 1 deliverable in `docs/PLAN.md` exists, and every acceptance box I can't do myself passes on your own re-run. The owner-run boxes (travel Mac checks, publish decision) are listed in your final report as "for Jay".

## Constraints
- Approved dependencies: only what `CLAUDE.md` lists. Anything else (crates, npm, Homebrew, test frameworks): ask me.
- Core rules in `CLAUDE.md` are non-negotiable, especially rule 1 (build host network) and rule 5 (recovery without the app).
- Out of scope this session: helper socket hardening, journal writing (Phase 2), network repair tiers (Phase 3), Settings UI (Phase 4). Phase 1 only needs the rescue script to *read* a journal if one exists (use the schema in the `macos-helper` skill) and otherwise offer per-service DHCP.
- Don't change the helper protocol this session.

## Advising the builder
The builder will pause with `Result: ADVICE NEEDED` for approach checks, open design choices, repeated failures or conflicting guidance. Answer via `SendMessage` to the same builder with the decision, a one-line reason, and any constraint to watch. Be decisive. If the question is mine to decide, ask me first, then relay my answer. Advice rounds don't count toward the 2 revision rounds.

## Review standard (per package)
- Read the diff and re-run the verification commands yourself.
- `cargo test` + `cargo clippy -- -D warnings` for touched crates; `npm run build:web` if `src/` changed; `bash -n` + `shellcheck` for scripts; `./scripts/verify-pkg.sh` on any built pkg.
- Check core rules: no `sh -c` added, no network-mutating command reachable from tests, every destructive path has `--plan`/dry-run and a backup.
- Fixes go back to the same builder as a numbered list. After 2 failed rounds, stop and ask me.
- Commit each passing package on `dev` with `/commit-commands:commit` and update `docs/PROGRESS.md`. Push `dev` (pre-approved) before each draft release.

## Acceptance (whole session)
All commands on the **Mac Studio, repo root**:
- [ ] Phase 0 acceptance boxes in `docs/PLAN.md` ticked, with the evidence in your report
- [ ] Phase 1 acceptance boxes ticked except the owner-run ones
- [ ] `gh release list --limit 5` shows two new drafts (end of Phase 0, end of Phase 1); `gh release view <phase-1 tag> --json isDraft,assets` shows `isDraft: true` and the `.pkg`, `.sha256`, rescue script
- [ ] Draft notes contain a "What to test on the travel Mac" list (install, quick sets still work, `--plan` output, uninstall, check nothing's left with the commands in `docs/ROAD-TESTING.md`)

## Action Boundaries
Stop and ask me before: adding dependencies, deleting or moving files (I'll approve the Windows park and the old uninstall script removal as a batch if you list them), changing `CLAUDE.md` rules, running parallel builders, any `git push` other than `git push origin dev`, publishing or editing a release, merging to `main`.

## When Done
Tick the boxes in `docs/PLAN.md`, update `docs/PROGRESS.md` (including the latest draft URL), commit and push `dev`. Report, phone-friendly:
1. **For Jay** (top): draft link(s) + what to test + decisions needed (publish v1.14.0 now?).
2. Packages and commit hashes.
3. Verification evidence from your re-runs (short).
4. Review issues found and fixed; open issues; any PLAN changes.
Then stop.
