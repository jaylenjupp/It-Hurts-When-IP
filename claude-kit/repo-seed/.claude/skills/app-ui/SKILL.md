---
name: app-ui
description: Use when changing anything in src/ (index.html, main.js, styles.css), adding screens/panels/modals such as Settings, Maintenance or Updates, or adding the Tauri commands the UI calls in src-tauri/src/main.rs. Guarantees the UI stays calm, fast and honest about network state, and never hides a destructive action behind one click.
---
# App UI (Tauri frontend + commands)

## Contract / core rule
**The UI must always tell the truth about the network and never surprise the user.** What's shown as current IP/mode comes from a fresh read, every destructive action shows its plan and needs an explicit confirm, and a failed action says exactly what failed and how to recover.

## Details

### Feel
A small, quiet utility used under pressure backstage: one window, big obvious targets, no animation for its own sake, no nagging. It should feel like a well-labelled rack panel — everything has a place, status is readable at a glance, dangerous controls are red and set apart. Jay often fine-tunes CSS himself: keep styles in `src/styles.css`, use clear class names, and report the visual values you chose so he can adjust them.

### Design plugins
None installed. If one is added later, this skill wins on conflicts about layout and behaviour; the plugin may advise on polish.

### Tokens as code
`styles.css` currently has no variables. When adding new UI (Phase 4+), introduce CSS custom properties at `:root` for the colours/spacing the new UI uses (`--ihw-bg`, `--ihw-fg`, `--ihw-muted`, `--ihw-accent`, `--ihw-danger`, `--ihw-ok`, `--ihw-radius`, `--ihw-gap`) and use them in new rules. Don't restyle the existing main screen unless the brief says so.

### Light / dark
The app currently follows one look. New UI should read correctly in both macOS appearances via `@media (prefers-color-scheme: dark)` overrides of the tokens. TODO(verify with Jay) whether the main screen should also follow dark mode — ask in the Phase 4 approach check.

### Layout
Window is 480–800 px wide, min height 580 (`tauri.conf.json`). Design for 480 first; at ≥640 px allow two-column rows in Settings. No horizontal scrolling at 480.

### Settings (Phase 4) — proposal, confirm in approach check
- Opened from a gear button in the main window and the app menu (`It Hurts When IP → Settings…`, ⌘,).
- Recommended: an in-window slide-over panel (no new window/capabilities). Alternative: a second Tauri window (needs a capability entry and window config — ask first).
- Sections: **General** (check for updates on launch ✓, remind-later period), **Updates** (current version + build stamp, latest version, last checked, *Check now*, *Update* button — Phase 5), **Maintenance** (*Repair network…*, *Uninstall…*, *Open rescue script location*, *Copy diagnostics*), **Advanced** (locked by default).
- **Advanced mode unlock:** a toggle behind a confirm ("Advanced options can change more of your network. Turn on?"), persisted in `AppConfig`. Candidate advanced features (owner picks in Phase 4): show hidden/VPN services in the interface list, per-quick-set DNS servers, verbose helper logging, full network reset (tier 3) visible, dev build channel info. Every new config field goes in the struct **and** the hand-written `Default`.

### Destructive flows (uninstall, repair)
1. Explain in one sentence what it does. 2. Show the plan list from `rescue --plan` (exact services and files). 3. Explicit confirm button naming the action ("Uninstall and restore 2 services"). 4. Password prompt from macOS. 5. Result list with ✓/✗ per item and a "what next" line (e.g. "Restart your Mac to finish").

### Screens / states checklist
For every new screen or panel: empty state, loading state (spinner or "Checking…" text, disabled buttons), error state (message + next step), success state. Specifically:
- Interface list: no services found · helper not running · service disabled.
- Updates: up to date · update available · check failed (offline / GitHub error) · downloading · installer opened.
- Maintenance: plan loading · nothing to do · plan shown · running · finished with failures.

### Tauri commands
- Add commands in `main.rs` delegating to `platform::` or a focused module; register in `generate_handler!`.
- Commands return `Result<T, String>` with human-readable errors.
- Long-running work is `async` so the UI stays responsive.

## Known quirks
- `withGlobalTauri: true` — code mixes `invoke` imports and `window.__TAURI__`; prefer the import.
- The update check runs on every launch and suppresses the modal for 24 h after dismissal (`update_prompt_dismissed_at`). The Settings "Update" button must work regardless of that suppression.
- Gateway field tells users "No gateway? Use 0.0.0.0" — keep behaviour consistent with the helper's validation rules.
- Window position persistence lives in `main.rs` setup; don't regress the off-screen guard.

## Tests
- Rust: unit-test command logic (config defaults/migration, update comparison, plan parsing) with fakes.
- Frontend: `npm run build:web` must pass. Keep DOM logic in small functions so it can be checked by reading; no test framework is installed (ask before adding one).
- Visual checks are Jay's: list "what to look at" (480 px and 800 px widths, light and dark) in the phase report so he can check the draft build on his travel Mac.
