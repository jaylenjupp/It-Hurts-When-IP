# It Hurts When IP — Claude Code kit

Drives remote development of It Hurts When IP from your phone while the Mac Studio at home does the building. Claude works on a `dev` branch, commits, builds a verified universal `.pkg`, and uploads it as a **draft** GitHub release that only you can see. You download it on the travel Mac, test, and report back in the same session.

Do steps 1–5 **at home, before you leave**.

## 1. Before you start (Mac Studio)
Follow `repo-seed/docs/REMOTE-SETUP.md` sections 1–3: Rust targets (both arm64 and x86_64), node, `gh auth login`, `tmux`, Energy settings, the Claude phone app. Claude Code needs to be signed in with your claude.ai account (Remote Control doesn't work with an API key).

## 2. Put the kit in the repo (Mac Studio, Terminal)
```bash
cd ~/Developer/it-hurts-when-ip          # your clone of jaylenjupp/it-hurts-when-ip
git switch main && git pull --ff-only
# unzip the kit here so you have ./claude-kit/
cp -Rn claude-kit/repo-seed/. .
ls -a .claude CLAUDE.md docs/PLAN.md     # check the hidden .claude folder came across
```
`.claude/` is a hidden folder: Finder drag-and-drop of the *contents* of `repo-seed` skips it. Use the `cp` line. `-n` means it never overwrites a file already in the repo (nothing in the seed collides today).

## 3. How the sessions are split
- **Orchestrator** = the main session, on **Fable** (`/model` → Fable before pasting a prompt). It plans, writes briefs, answers the builder's questions, reviews diffs, re-runs tests, commits, and makes draft releases. It doesn't write app code.
- **Builder** = `.claude/agents/builder.md`, on **Opus**. Does all hands-on work and reports with real command output.
- Revisions go back to the same builder; after 2 failed rounds the orchestrator stops and asks you.
- Approvals still come to you: on your phone as push notifications. Pre-approved: building, testing, committing on `dev`, `git push origin dev`, and creating **draft** releases via `scripts/ship/release-dev.sh`. Everything else that's risky asks (other pushes, publishing, new dependencies, deletes). Network-changing commands, `sudo` and installing packages are **denied outright** on the Mac Studio.

## 4. Start Claude with Remote Control (Mac Studio, Terminal)
```bash
tmux new -s ihwip
cd ~/Developer/it-hurts-when-ip
claude --remote-control "It Hurts When IP"
```
Then in Claude: `/model` → Fable, `/config` → **Push when actions required** on. Details and why it's the interactive mode, not server mode: `docs/REMOTE-SETUP.md` §4.

## 5. Session 1 — setup (paste in Claude)
> Read `claude-kit/prompts/01-setup.md` and follow it exactly.

Approve the plugin installs when asked. When it finishes, run `/reload-plugins` (or restart Claude in the same tmux window). Detach tmux with **Ctrl-b, d** before you leave — don't close the session.

## 6. Session 2 — kickoff (from anywhere)
`/clear`, then:
> Read `claude-kit/prompts/02-kickoff.md` and run it.

Approve the plan from your phone. It delivers Phase 0 (build/release pipeline) and Phase 1 (uninstaller + rescue script) as draft builds.

## 7. Every phase after
`/clear`, then:
> Read `claude-kit/prompts/03-phase.md` and run it for Phase 2.

(Change the number.) Each phase ends with a draft build + "what to test" list.

## 8. Getting builds and testing (travel Mac)
`docs/RELEASING.md` (download + checksum) and `docs/ROAD-TESTING.md` (what to note before installing, how to report, and how to fix the network **without** the app). Download `ROAD-TESTING.md` and the rescue script to the travel Mac before testing anything.

## 9. Usage limits
- Limit hit: after the reset, in the same session (or `claude --continue` in the tmux window at home): "Check `git status`, `git log --oneline -10` and `docs/PROGRESS.md`, then continue."
- Fable used up: `/model` → Opus and carry on.
- To stretch usage: ask Claude to set `model: sonnet` in `.claude/agents/builder.md`.

## 10. Seed files
| File | What it's for |
|---|---|
| `CLAUDE.md` | Persistent brief: project, stack, core rules (incl. "never touch the build host's network"), orchestration, commands |
| `docs/PLAN.md` | Phases 0–7 with deliverables and binary acceptance checks |
| `docs/PROGRESS.md` | Where things stand; updated after every package |
| `docs/REMOTE-SETUP.md` | Mac Studio + phone setup for Remote Control |
| `docs/RELEASING.md` | Draft vs published releases, download + checksum on the road |
| `docs/ROAD-TESTING.md` | Testing on the travel Mac + manual network recovery |
| `.claude/settings.json` | Allow / ask / deny permissions (network changes, sudo, installs denied) |
| `.claude/agents/builder.md` | The Opus builder subagent |
| `.claude/skills/macos-helper/` | Privileged helper, socket protocol, validation, journal |
| `.claude/skills/network-recovery/` | Uninstaller, rescue script, repair tiers |
| `.claude/skills/app-ui/` | Frontend, Settings, destructive-flow UX |
| `.claude/skills/build-release/` | Universal build, verify-without-install, draft/published releases |
| `scripts/ship/release-dev.sh` | Uploads a build as a draft release (refuses anything else) |
| `tools/fake-networksetup/` | Fixture-driven fake `networksetup` so nothing touches the real network |
| `reference/AUDIT-2026-11.md` | Findings from a read of v1.13.1 (socket 777, `sh -c` injection, uninstall gaps) |

## 11. Choices made for you (change them if you disagree)
- **Repo stays public; dev builds are drafts.** Making the repo private would hide the releases from colleagues' update checkers, so nobody on v1.13.1 would ever be offered the uninstaller fix. Drafts are only visible to you and the update checker ignores them.
- **Claude doesn't install builds on the Mac Studio.** Installing needs root (`sudo installer`), and a bad helper build or a network test on that Mac could cut it off the network — which would end your Remote Control session until you're home. Builds are verified without installing (`verify-pkg.sh`: architectures, signatures, payload, scripts); you install on the travel Mac.
  - *If you still want it:* a sudoers rule like `jaylenjupp ALL=(root) NOPASSWD: /usr/sbin/installer -pkg /Users/jaylenjupp/Developer/it-hurts-when-ip/release-out/* -target /` would let Claude install unattended. **Exposure:** anything that runs as your user (Claude, any script, any compromised tool) can then drop a package in that folder and get root on the Mac Studio. I'd leave it off.
- **Windows is parked, not deleted.** Note: Windows colleagues won't get the uninstall/repair work until Windows comes back.
- **Advanced-mode features** are proposed in the `app-ui` skill; you pick the list at the start of Phase 4.

## 12. Third-party plugins
Plugins can run code on your Mac. Setup only installs Anthropic's `commit-commands` and `feature-dev`, and asks before each install. Anything else: Claude must show you the repo and what it adds, then wait for your yes.

Sources: Claude Code Remote Control (https://code.claude.com/docs/en/remote-control), plugin install (https://code.claude.com/docs/en/plugins/install), Anthropic marketplaces (https://code.claude.com/docs/en/plugins/anthropic-marketplaces), permissions (https://code.claude.com/docs/en/permissions), subagents (https://code.claude.com/docs/en/sub-agents), GitHub releases API — drafts visible only to users with push access (https://docs.github.com/en/rest/releases/releases).
