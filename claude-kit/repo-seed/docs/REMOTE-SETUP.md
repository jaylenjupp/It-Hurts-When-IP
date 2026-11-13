# Remote setup — running Claude Code on the Mac Studio while travelling

Everything in this file runs **on the Mac Studio** unless it says otherwise. Do it before you leave; you can't fix most of it from the road.

Sources: Claude Code Remote Control docs — https://code.claude.com/docs/en/remote-control

## 1. Tools (Mac Studio, Terminal)
Check each; install what's missing.
```bash
xcode-select -p                      # Command Line Tools present?
rustup target list --installed       # need aarch64-apple-darwin AND x86_64-apple-darwin
node -v && npm -v
gh --version && gh auth status       # GitHub CLI signed in as jaylenjupp
tmux -V                              # keeps Claude running after you close Terminal
claude --version
```
- Missing x86_64 target: `rustup target add x86_64-apple-darwin` (needed for the universal build your Intel travel MacBook runs).
- Missing tmux / gh: `brew install tmux gh`.
- `gh auth login` → GitHub.com → HTTPS → log in with a web browser. The token is kept in the macOS keychain, not in the repo.

## 2. Keep the Mac Studio reachable
Remote Control only makes **outbound** HTTPS connections and never opens inbound ports on your Mac, so nothing on your router needs changing. It does need the Mac awake and the `claude` process running.
- **System Settings → Energy** (TODO(verify) exact labels on your macOS): turn on *Prevent automatic sleeping when the display is off* and *Start up automatically after a power failure*.
- If the Mac restarts (power cut, update), Claude Code will **not** come back by itself — and FileVault will hold the Mac at the login screen. Don't turn FileVault off to work around this; just accept that a restart ends remote work until someone at home logs in and runs step 4 again.
- **macOS updates:** an automatic macOS install restarts the Mac. Option: System Settings → General → Software Update → Automatic updates → turn off *Install macOS updates* while away (keep *Download* on). **Exposure:** that delays OS security fixes until you're back. Leave *Security Responses and system files* on unless you're happy to delay those too — they can occasionally restart the Mac. Your call; the conservative choice is to leave security responses on.
- Don't use the app's network-changing features on this Mac while away (CLAUDE.md core rule 1 enforces this for Claude).

## 3. Phone
- Install the **Claude** app (iOS/Android), sign in with the same account you use for Claude Code. Remote Control needs a Pro, Max, Team or Enterprise plan and a claude.ai login (not an API key).
- Optional, recommended: claude.ai → Settings → turn on **Require trusted devices**, so only devices you've verified can steer the session.

## 4. Start the session (Mac Studio, Terminal)
```bash
tmux new -s ihwip
cd ~/Developer/it-hurts-when-ip          # wherever you cloned the repo
claude --remote-control "It Hurts When IP"
```
- First run asks you to trust the folder and to enable Remote Control — answer yes.
- Inside Claude: `/config` → turn on **Push when actions required** (permission prompts and questions reach your phone) and, if you like, **Push when Claude decides**.
- `/model` → **Fable** for the orchestrator.
- Detach from tmux with **Ctrl-b then d**; Claude keeps running. Reattach later at the Mac: `tmux attach -t ihwip`.
- Use `claude --remote-control` (interactive), **not** `claude remote-control` (server mode): server mode exits after about 10 minutes without network; the interactive session keeps retrying until the network returns.

## 5. From the road
- Open the Claude app (or claude.ai/code) → the "It Hurts When IP" session.
- Permission prompts (pushes, publishing, new dependencies, deletes) wait until you answer them — nothing risky happens while you're asleep, but work also pauses there.
- You can attach screenshots/files from your phone; they're downloaded to the Mac Studio for Claude.
- If the session shows **disconnected** but the Mac is fine, it reconnects by itself once it's back online. If Claude Code itself stopped, you need someone at the Mac (step 4) — there's no way to restart it remotely without opening up remote access, which this setup deliberately doesn't do.

## 6. Usage limits / resuming
- Hit a limit: wait for the reset, then in the same session send: "Check `git status`, `git log --oneline -10` and `docs/PROGRESS.md`, then continue."
- Fable usage used up: `/model` → Opus and carry on; the orchestrator role doesn't change.
- To stretch usage, set `model: sonnet` in `.claude/agents/builder.md` (ask Claude to make that change; it needs your approval).
- New phase: `/clear`, then "Read `claude-kit/prompts/03-phase.md` and run it for Phase N."
