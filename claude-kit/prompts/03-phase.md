<!-- Use from the phone: "/clear" then "Read claude-kit/prompts/03-phase.md and run it for Phase N." Replace [N] with that number. -->

## Your role in this session
You are the **orchestrator**, running on Fable. You plan, advise and brief the **`builder` subagent** (Opus, `.claude/agents/builder.md`), review, and commit. You don't edit application code yourself. Follow "How sessions work" in `CLAUDE.md`.

## Session Strategy
New session (`/clear` first). Plan mode, then delegate after I approve. I'm usually on my phone: keep the plan scannable.

## Objective
Deliver **Phase [N]** from `docs/PLAN.md`, ending with a draft build I can test on my travel Mac.

## Context
- Read `CLAUDE.md`, `docs/PROGRESS.md` and the Phase [N] section of `docs/PLAN.md`. Run `git status` and `git log --oneline -15`. Confirm you're on `dev`.
- Check `docs/PROGRESS.md` for my test results from the previous draft and for open decisions; raise any that block this phase first.
- Use `/feature-dev` for your own exploration and design, then turn the design into builder briefs.
- Name the phase's skills in each brief.
- Core rule 1: nothing that changes live network state on this Mac. Fakes and `--plan`/dry-run only.

## Plan (what I approve)
An ordered list of work packages, each with: goal, skills, scope paths, acceptance checks, verification commands, and Approach check yes/no. Keep each package small enough to review in one diff. Put decisions you need from me at the top, each with your recommendation. The last package is always the draft release.

## Target State
Everything in Phase [N]'s "Deliverables" exists, and every box in its "Acceptance" list passes on your own re-run (owner-run boxes listed as "for Jay").

## Constraints
- Stay inside Phase [N]. Later-phase work goes in your report.
- Approved dependencies are listed in `CLAUDE.md`. Relay new ones to me for approval.

## Advising the builder
The builder will pause with `Result: ADVICE NEEDED` for approach checks, open design choices, repeated failures or conflicting guidance. Answer via `SendMessage` to the same builder with the decision, a one-line reason, and any constraint to watch. Be decisive. If the question is mine to decide, ask me first, then relay my answer. Advice rounds don't count toward the 2 revision rounds.

## Review standard (per package)
- Read the diff and re-run the verification commands yourself.
- `cargo test` and `cargo clippy -- -D warnings` for every touched crate; `npm run build:web` if `src/` changed; `bash -n` + `shellcheck` on touched scripts; `./scripts/build-macos.sh` + `./scripts/verify-pkg.sh` before any draft.
- Core-rule checks: no `sh -c` / shell-string commands added; no network-mutating command reachable from tests or dev runs; destructive paths have plan/dry-run, confirm, backup, verify; new `AppConfig` fields are in the `Default` impl and old configs still load.
- UI changes: list what I should look at (480 px and 800 px widths, light/dark) — I check visuals on the draft.
- Fixes go back to the same builder as a numbered list. After 2 failed rounds, stop and ask me.
- Commit each passing package with `/commit-commands:commit` on `dev` and update `docs/PROGRESS.md`.

## Draft release (last package)
`git push origin dev` (pre-approved) → `./scripts/build-macos.sh` → `./scripts/ship/release-dev.sh release-out/<pkg> <notes.md>`. Notes include "What changed" and "What to test on the travel Mac". Confirm with `gh release view <tag> --json isDraft,assets`.

## Action Boundaries
Stop and ask me before: adding dependencies, changing an earlier phase's schema or the helper protocol incompatibly, deleting or moving files, changing `CLAUDE.md` rules, running parallel builders, any `git push` other than `git push origin dev`, publishing/editing/deleting a release, merging to `main`.

## When Done
Tick Phase [N]'s boxes in `docs/PLAN.md`, update `docs/PROGRESS.md` (with the draft URL), commit and push `dev`. Report, phone-friendly:
1. **For Jay** (top): draft link, what to test, decisions needed.
2. Packages and commit hashes.
3. Verification evidence from your re-runs (short).
4. Review issues found and fixed; open issues.
Then stop.
