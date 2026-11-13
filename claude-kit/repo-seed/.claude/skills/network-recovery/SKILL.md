---
name: network-recovery
description: Use when working on the uninstaller, the standalone rescue script (recovery/), the change journal, network repair/reset, or anything that deletes files or reverts system network config. Guarantees every destructive action is planned, confirmed, backed up, verified, and never run for real on the build host.
---
# Uninstall, rescue and network repair

## Contract / core rule
**A show-critical Mac must be able to get back to a working network even if the app is broken.** Every destructive action runs as plan → confirm → execute → verify, backs up what it removes, and only reverts what the app actually changed unless the user explicitly picks a wider reset. None of it is ever executed for real on the build host — only `--dry-run` and fakes there.

## Details

### One script, two front doors
- `recovery/ithurtswhenip-rescue.command` — bash, shellcheck-clean, no dependencies beyond macOS built-ins. It is the single source of truth for uninstall and repair.
- **Standalone:** attached to every GitHub release (draft and published) and documented in `docs/ROAD-TESTING.md`. Run with `sudo bash ithurtswhenip-rescue.command` from Terminal (downloaded `.command` files get quarantined; running through `bash` sidesteps Gatekeeper's double-click block — TODO(verify) on Jay's travel Mac). With no flags it shows a numbered menu; with flags it runs non-interactively.
- **In-app:** bundled in `It Hurts When IP.app/Contents/Resources/`. The app shows the plan in its own UI, then runs the script with an admin password prompt (`osascript -e 'do shell script "…" with administrator privileges'`, built from argv-escaped parts — TODO(verify) behaviour with the ad-hoc-signed app). This path must **not** need the helper, because the helper may be the broken part.
- Flags (proposal — confirm in the Phase 1 approach check): `--plan` (print every action, change nothing; default when stdout isn't a TTY and no action flag given), `--uninstall`, `--network restore-journal|dhcp:<service>[,…]|leave`, `--repair soft|service:<name>|full`, `--yes` (skip interactive confirm; only the app passes it, after its own confirm), `--wait-pid <pid>`, `--log <path>`.
- Exit codes: 0 ok, 1 partial (some steps failed, listed), 2 refused (bad args / not root / plan only).

### Uninstall inventory (remove all that exist; report each)
Root-owned, current identifiers:
- `launchctl bootout system/com.ithurtswhenip.helper` (fall back to `launchctl unload` on old macOS), then remove `/Library/LaunchDaemons/com.ithurtswhenip.helper.plist`, `/Library/PrivilegedHelperTools/com.ithurtswhenip.helper`, `/var/run/com.ithurtswhenip.helper.sock`, `/var/log/com.ithurtswhenip.helper.log`, `/var/log/com.ithurtswhenip.helper.error.log`
- `/Library/Application Support/com.ithurtswhenip.helper/` (journal — use it for network revert **before** deleting; copy to the backup folder)
- `/Applications/It Hurts When IP.app` (after `--wait-pid` exits / the app quits)
- `pkgutil --forget com.ithurtswhenip.pkg`
Legacy (pre-rename):
- `com.ipswitcher.helper` daemon, plist, binary, socket (TODO(verify) old socket path), `pkgutil --forget com.ipswitcher.pkg` and `com.ipswitcher.helper`
- TODO(verify with Jay): any older app bundle names (e.g. "IP Switcher.app")
Per user (every user with a home in `/Users`, not just the console user):
- `~/Library/Application Support/com.ithurtswhenip.desktop`, `~/Library/Caches/com.ithurtswhenip.desktop`, `~/Library/WebKit/com.ithurtswhenip.desktop`, `~/Library/HTTPStorages/com.ithurtswhenip.desktop`, `~/Library/Preferences/com.ithurtswhenip.desktop.plist`, `~/Library/Saved Application State/com.ithurtswhenip.desktop.savedState`, `~/Library/Logs/com.ithurtswhenip.desktop`, legacy `~/Library/Application Support/com.ipswitcher.switcher`
- TODO(verify): confirm the real list with a read-only search on the Mac Studio, e.g. `find ~/Library /Library -iname '*ithurtswhenip*' -o -iname '*ipswitcher*' 2>/dev/null` (reading is fine; deleting is not).
After the known list, the script searches those roots for `*ithurtswhenip*` / `*ipswitcher*` and **reports** leftovers rather than deleting unknown paths.

### Network revert (part of uninstall)
1. **Journal exists** → offer "restore original settings" per journaled service (show original vs current), default on.
2. **No journal** (installs from before the journal existed) → list services currently on Manual config with their IPs; user ticks which to switch to DHCP. **Default: none ticked.** Never blanket-reset services the user didn't pick — a colleague's Dante or management NIC may be intentionally static.
3. "Leave network as it is" is always an option.

### Network repair tiers (Phase 3)
1. **Soft refresh** — flush DNS (`dscacheutil -flushcache` + `killall -HUP mDNSResponder`), renew DHCP on DHCP services (`ipconfig set <bsd-device> DHCP` — TODO(verify) still the right renew on current macOS). No config is deleted.
2. **Service reset** — for one chosen service: record current config to backup, set DHCP, clear app-set DNS/search domains, toggle the service off/on (`-setnetworkserviceenabled`). Verify it gets an address or report link-local.
3. **Full network configuration reset** — backup, then remove from `/Library/Preferences/SystemConfiguration/`: `preferences.plist`, `NetworkInterfaces.plist`, `com.apple.network.identification.plist`, `com.apple.airport.preferences.plist` (if present), then **restart required**. macOS regenerates them. Consequences shown before confirming: all network services, static configs, Locations and service order are rebuilt from scratch; Wi-Fi networks must be rejoined. Requires typing `RESET` (or the app's equivalent). TODO(verify) the file list on the oldest and newest macOS the team runs.
- Backups go to `/Users/Shared/ItHurtsWhenIP-backups/<UTC-timestamp>/` (survives uninstall; readable by the user). The script can `--restore-backup <dir>` for tier 3.

### Verify step
After any action: re-read the state (`networksetup -getinfo`, `scutil --nwi`, file existence) and print a pass/fail line per item. The UI shows this list.

## Known quirks
- `networksetup` takes service names; `ipconfig` and `ifconfig` take BSD devices (`en7`). Map via `-listallhardwareports`.
- An app can't delete its own bundle while running: launch the script detached with `--wait-pid`, then quit.
- Removing the SystemConfiguration plists while the network is up leaves the running config in memory until restart — say so in the UI.
- `pkgutil --forget` only removes the receipt, not files.

## Tests
- `shellcheck` and `bash -n` on every script.
- Make every system path in the script overridable by env vars (`IHWIP_ROOT=/tmp/fake-root`, `IHWIP_NETWORKSETUP`, `IHWIP_LAUNCHCTL`, `IHWIP_PKGUTIL` …) so tests build a fake filesystem tree under a temp dir, run the script **without root**, and assert what was removed, what was backed up, and the exact commands logged by fakes.
- Plan output is golden-tested: `--plan` against fixture trees with/without journal, with legacy files, with leftovers.
- Real runs happen only on Jay's travel Mac, by Jay, following `docs/ROAD-TESTING.md`.
