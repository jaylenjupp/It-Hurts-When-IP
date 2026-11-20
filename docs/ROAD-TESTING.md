# Testing draft builds on the travel Mac

Everything here runs **on your travel MacBook**, by you. Claude never installs or runs network changes; it builds, you test. (Exception: Phase 3, 4 and 5 testing happen on the Mac Studio, by you, see *Phase 3 — network repair*, *Phase 4 — Settings* and *Phase 5 — Updates in Settings*.)

**Download this file and the rescue script before you start testing**, so you have them if the network goes away.

## Before installing a new build
1. Note what "normal" looks like (Terminal):
   ```bash
   networksetup -listallnetworkservices
   networksetup -getinfo "Wi-Fi"            # and any other service you use
   ```
   Screenshot it.
2. Know which network you're on and whether you can rejoin it (hotel Wi-Fi password, phone hotspot as a fallback).

## Installing
Double-click the `.pkg` → it's blocked as an unknown developer → Cancel (don't delete it) → System Settings → Privacy & Security → **Open Anyway** → password once.

## Reporting back
In the Claude app session: what you did, what you expected, what happened, plus screenshots. From Phase 1 on, also paste the output of:
```bash
sudo bash ~/Downloads/ithurtswhenip-rescue.command --plan
```
(`--plan` changes nothing.)

## Uninstalling (rescue script)
Every command in this section runs on **Jay's travel Mac, Terminal**. The same script is in two places:
- inside the app: `"/Applications/It Hurts When IP.app/Contents/Resources/ithurtswhenip-rescue.command"` (keep the quotes)
- the release asset: `~/Downloads/ithurtswhenip-rescue.command` (works when the app is gone or broken)

The examples use the downloaded copy; swap in the in-app path to test that one too. Each uninstall removes the app, so reinstall the pkg before trying the next way.

1. **Plan first, and paste the output into the session** (changes nothing):
   ```bash
   sudo bash ~/Downloads/ithurtswhenip-rescue.command --plan
   ```
   One tab-separated line per step: `BACKUP …` (copied first), `NETWORK manual <service> ip=… selected=no` (services on Manual settings; none is picked unless you pick it), `NETWORK choice - leave`, `STOP launchd com.ithurtswhenip.helper`, `REMOVE …`, `FORGET receipt com.ithurtswhenip.pkg`, `LEFTOVER …` (found, only reported), then a `# summary:` line.
2. **Uninstall from the menu:**
   ```bash
   sudo bash ~/Downloads/ithurtswhenip-rescue.command
   ```
   Pick `3` to leave the network alone, or `2` to tick services to switch to DHCP (nothing is ticked to begin with; type a number to tick, `d` when done). It prints the plan again, then asks you to type `UNINSTALL`.
3. **Uninstall and put one service back on DHCP, without the menu** (name exactly as in the plan's `NETWORK` lines):
   ```bash
   sudo bash ~/Downloads/ithurtswhenip-rescue.command --plan --network dhcp:"<service>"
   sudo bash ~/Downloads/ithurtswhenip-rescue.command --uninstall --network dhcp:"<service>"
   ```
   The plan shows `NETWORK setdhcp <service> source=pick`. The uninstall still asks you to type `UNINSTALL`.

**What a good run prints** after you confirm (fields are tab-separated, lined up here; there are more `OK`/`VERIFY` lines of each kind):
```
OK      BACKUP   snapshot  network  network-before.txt
OK      NETWORK  setdhcp   <service>  source=pick
VERIFY  ok       network:<service>  now=dhcp/<ip>/<mask>/<router>
OK      STOP     launchd   com.ithurtswhenip.helper  stopped (bootout)
OK      REMOVE   file      /Library/PrivilegedHelperTools/com.ithurtswhenip.helper  -
OK      FORGET   receipt   com.ithurtswhenip.pkg  -
VERIFY  ok       /Library/PrivilegedHelperTools/com.ithurtswhenip.helper
VERIFY  ok       launchd:com.ithurtswhenip.helper
VERIFY  ok       receipt:com.ithurtswhenip.pkg
# result
# backup folder: /Users/Shared/ItHurtsWhenIP-backups/<UTC time>
# log: /Users/Shared/ItHurtsWhenIP-backups/<UTC time>/rescue.log
# failed steps: 0
# leftovers (reported only, never removed): 0
```
- The `network:` lines appear only if you picked a service. They check the service is set to DHCP, not that it already has an address; `(no DHCP address yet: …)` after it means check the cable or DHCP server. TODO(verify) on a real Mac.
- Problems show as `FAILED …` lines (a step that failed) or `VERIFY FAILED …` (still there afterwards), repeated under `# failed steps:`.
- `echo $?` straight afterwards: `0` all good, `1` some steps failed (listed), `2` refused or cancelled (nothing changed).

**Afterwards, check and report:**
```bash
ls "/Users/Shared/ItHurtsWhenIP-backups/"                # one folder per run (UTC time)
ls "/Users/Shared/ItHurtsWhenIP-backups/<UTC time>/"     # plan.txt, rescue.log, network-before.txt, system/, users/
networksetup -getinfo "<service>"                        # if you picked one: "DHCP Configuration"
```
Read-only leftover check: both commands should print nothing (TODO(verify) on a real install):
```bash
sudo find /Applications /Library /Users/*/Library /private/var/run /private/var/log /private/var/db/receipts \( -iname '*ithurtswhenip*' -o -iname '*ipswitcher*' -o -iname '*it-hurts-when-ip*' \) -print 2>/dev/null
pkgutil --pkgs | grep -i -e ithurtswhenip -e ipswitcher
```

TODO(verify): `sudo bash` runs a downloaded (quarantined) `.command` without a Gatekeeper prompt; the in-app copy runs the same way.

## Per-phase test lists
Claude adds a "what to test" list to each phase report and to the draft release notes. Tick through it and reply with results; Claude records them in `docs/PROGRESS.md`.

### Phase 2 — helper hardening + change journal
Every command runs on **Jay's travel Mac, Terminal**. Paste each output into the session.

1. **Install over the existing version** (see *Installing*). Open the app: there should be no banner above "Network Interface". If a banner shows, note its exact text.
2. **Changes still work:** apply each quick set, then **Set DHCP**, then one quick set that has **no gateway**. After the no-gateway one:
   ```bash
   networksetup -getinfo "<service>"
   ```
   shows the quick set's IP and an empty `Router:` line.
3. **Socket is admin-only:**
   ```bash
   ls -l /var/run/com.ithurtswhenip.helper.sock
   ```
   Expect `srw-rw----  1 root  admin …`.
4. **Journal exists** (after at least one change in step 2):
   ```bash
   ls -ld "/Library/Application Support/com.ithurtswhenip.helper" "/Library/Application Support/com.ithurtswhenip.helper/journal.json"
   ```
   Expect `drwxr-xr-x … root …` for the folder and `-rw-r--r-- … root …` for `journal.json`.
5. **The rescue plan sees the journal** (changes nothing):
   ```bash
   sudo bash ~/Downloads/ithurtswhenip-rescue.command --plan
   ```
   Each service you changed shows as a `NETWORK journal <service> original=… current=…` line.
6. **Optional — Standard (non-admin) account**, if the Mac has one: log in as it and open the app. The banner says the account isn't an Administrator, and applying a quick set fails with a clear error.
7. **Phase 1 carry-over (not tested yet):** in the app, **Uninstall…** → follow it through → no files left and the network still works. Check with the steps under *Uninstalling (rescue script)* → "Afterwards, check and report" above. Reinstall the pkg afterwards if you want to keep testing.
8. **macOS version** (sets the minimum macOS the builds must support):
   ```bash
   sw_vers
   ```


### Phase 3 — network repair
**Where:** Jay's **Mac Studio** (macOS 26.7.1), in the installed app and in Jay's own Terminal. Jay runs every step himself. The Claude session never installs, never runs a repair and never changes the network (CLAUDE.md core rule 1); it only reads what you paste back.

The Mac Studio is also the machine the Claude session runs on. If a step takes its internet uplink down, the session is cut off until the network is back, so keep the *Manual Terminal fallback* below on your phone.

**Before you start** (Mac Studio, Terminal; all read-only):
1. Download this draft's `ithurtswhenip-rescue.command` to `~/Downloads`.
2. Find the internet uplink, so you can avoid it:
   ```bash
   route -n get default | grep interface    # the uplink's device, e.g. en0
   networksetup -listallhardwareports       # which service name uses that device
   networksetup -listallnetworkservices
   ```
3. Pick a **test adapter that is NOT the uplink** (for example a USB or Thunderbolt Ethernet adapter you don't need right now). Save its current settings; the service reset switches it to DHCP and clears its DNS servers and search domains:
   ```bash
   networksetup -getinfo "<test service>"
   networksetup -getdnsservers "<test service>"
   networksetup -getsearchdomains "<test service>"
   ```
4. Install the draft pkg (see *Installing*).

Every repair saves its backup to `/Users/Shared/ItHurtsWhenIP-backups/<UTC time>/` (one folder per run, kept): `plan.txt`, `rescue.log` (the full result), `network-before.txt`, plus `service-before.txt` (service reset) or `system-configuration/` with `SHA256SUMS` (full reset).

#### In the app (Mac Studio, the app)
Open **It Hurts When IP → Repair Network…** in the menu bar, or **Settings → Maintenance → Repair network…** (from Phase 4 on; the Phase 3 draft had the button at the bottom of the window). Every tier shows its plan first; nothing changes until you confirm and enter your password.

1. **Soft refresh.** Choose **Soft refresh**. The plan lists: clear the DNS cache, restart the DNS responder, and "ask for a fresh address" for each enabled adapter on DHCP; static, disabled and unplugged adapters are listed as left alone. Click **Refresh network** → password → **Repair finished** with a *Done* list, the backup folder and the full-result path.
   - Expected: adapters on DHCP show "working, automatic (DHCP), <ip> / <mask>". An adapter with no cable or no DHCP server shows in amber under **Needs a look**: "No address yet — check cable/switch/DHCP server" (or "only a self-assigned 169.254 address"). That is a warning, not a failure.
   - Note: if the uplink is on DHCP it is renewed too; expect at most a short blip.
   - What can go wrong / undo: no settings are changed, so there is nothing to undo. If something looks off afterwards, a second soft refresh or turning the adapter off and on (fallback step 5) is enough.
2. **Cancel the password prompt.** Start a soft refresh again, then click **Cancel** in the macOS password window.
   - Expected: back on the plan with "Cancelled, nothing changed." No new folder in `ls /Users/Shared/ItHurtsWhenIP-backups/` (Mac Studio, Terminal).
3. **Service reset on the test adapter (NOT the uplink).** Choose **Reset one service**. The picker lists every service; ones that can't be reset are greyed with the reason. Pick the test adapter. The plan shows "What gets cleared on <service>" (current IP, DNS servers, search domains). Click **Reset "<service>"** → password → result.
   - Expected *Done* lines: switch to automatic (DHCP), clear DNS servers, clear search domains, turn off, turn on again; then "working, …" or the amber **Needs a look** "No address yet" if nothing hands out an address on that adapter.
   - Check (Mac Studio, Terminal): `networksetup -getinfo "<test service>"` says DHCP Configuration; `networksetup -getdnsservers "<test service>"` and `networksetup -getsearchdomains "<test service>"` say none are set.
   - What can go wrong / undo: the adapter loses its static IP and its DNS / search domains, and drops off the network for a few seconds. The old values are in `/Users/Shared/ItHurtsWhenIP-backups/<UTC time>/service-before.txt`; put them back with the app's quick set (or Manual entry), or in Terminal (Mac Studio, Terminal):
     ```bash
     sudo networksetup -setmanual "<test service>" <ip> <mask> <router>
     sudo networksetup -setdnsservers "<test service>" <dns1> <dns2>
     sudo networksetup -setsearchdomains "<test service>" <domain>
     ```
4. **Full network reset — only if you choose to, and last** (after every other test is reported and the session has pushed its work). From Phase 4 on, turn on **Settings → Advanced** first; without it the tier isn't offered (the Terminal script's `--repair full` below doesn't need it). Choose **Full network reset**, read the consequences, type `RESET`, click **Reset network settings** → password.
   - Expected: *Done* lists the backup of each file, "Write the backup checksums", "Move <file> to the backup" and "moved, backup copy checked" per file. A **Restart needed** panel says to restart from the Apple menu; there is no restart button, and the network keeps working until you restart. On macOS 26 `com.apple.network.identification.plist` doesn't exist; it shows as not present (normal).
   - Check (Mac Studio, Terminal): `ls "/Users/Shared/ItHurtsWhenIP-backups/<UTC time>/system-configuration/"` lists the moved files and `SHA256SUMS`.
   - Then restart (Apple menu → Restart…). Afterwards every service, static IP, Location and the service order are rebuilt from scratch; rejoin Wi-Fi if needed.
   - What can go wrong / undo: the Mac comes back with default network settings (and the uplink may need its static settings again). To go back to exactly what you had, run `--restore-backup` with that folder (script test 5 below), then restart.

#### With the rescue script (Mac Studio, Terminal)
The examples use `~/Downloads/ithurtswhenip-rescue.command`; the in-app copy is `"/Applications/It Hurts When IP.app/Contents/Resources/ithurtswhenip-rescue.command"`. Lines are tab-separated. `echo $?` afterwards: `0` ok (also when there are only warnings), `1` some steps failed, `2` refused or cancelled (nothing changed).

1. **Plan a soft refresh** (changes nothing):
   ```bash
   sudo bash ~/Downloads/ithurtswhenip-rescue.command --plan --repair soft
   ```
   Expected: `# repair: soft`, `# plan only: nothing has been changed`, `REPAIR flushcache …`, `REPAIR hup …`, one `REPAIR renew <service> device=<dev> now=…` per enabled DHCP service, `REPAIR leave <service> <reason>` for the others, `# summary: …`. Run it without `--plan` to do it (type `REPAIR`).
2. **Service reset on the test adapter** (NOT the uplink):
   ```bash
   sudo bash ~/Downloads/ithurtswhenip-rescue.command --plan --repair service:"<test service>"
   sudo bash ~/Downloads/ithurtswhenip-rescue.command --repair service:"<test service>"
   ```
   The plan shows `BACKUP snapshot <service> service-before.txt`, `REPAIR current … now=… dns=… search=…`, then `setdhcp`, `cleardns`, `clearsearch`, `disable`, `enable`. The real run asks you to type `REPAIR`, then prints `OK REPAIR …` lines and `VERIFY ok repair:<service> now=dhcp/<ip>/…`, or `VERIFY warn repair:<service> …; no address yet: check the cable, the switch or the DHCP server` (or `link-local only (169.254.x.x)`), counted under `# warnings:`; exit code still `0`. Undo: as in-app test 3 (`service-before.txt`).
3. **The menu:**
   ```bash
   sudo bash ~/Downloads/ithurtswhenip-rescue.command
   ```
   Expected: items 1–8 (4 soft refresh, 5 reset one service, 6 full network reset, 7 restore a backup, 8 quit). Try `4` and `5` (5 lists numbered services; pick the test adapter). Each prints the plan and asks you to type `REPAIR`; anything else cancels with "cancelled: nothing has been changed" (exit `2`). `8` quits without changing anything.
4. **Plan a full reset** (changes nothing; safe any time):
   ```bash
   sudo bash ~/Downloads/ithurtswhenip-rescue.command --plan --repair full
   ```
   Expected: `BACKUP file …` and `RESET move …` for each file that exists, `RESET absent …` for one that doesn't (`com.apple.network.identification.plist` on macOS 26), three `NOTE info reset …` lines, `# summary: …`.
5. **Restore after a full reset** (only if you ran one):
   ```bash
   ls /Users/Shared/ItHurtsWhenIP-backups/
   sudo bash ~/Downloads/ithurtswhenip-rescue.command --plan --restore-backup /Users/Shared/ItHurtsWhenIP-backups/<UTC time of the reset>
   sudo bash ~/Downloads/ithurtswhenip-rescue.command --restore-backup /Users/Shared/ItHurtsWhenIP-backups/<UTC time of the reset>
   ```
   (Menu item `7` does the same and lists the backups newest first.) Type `RESTORE`. Expected: the current files are backed up first to a **new** backup folder, then `OK RESTORE file …`, `VERIFY ok restore:<file> matches SHA256SUMS, mode 644`, and last `RESTART needed … RESTART NEEDED: …`. Restart the Mac yourself; your old services, static IPs and Locations are back. If anything doesn't match its checksum the script refuses and changes nothing.

Paste every output into the session.

### Phase 4 — Settings
**Where:** Jay's **Mac Studio** (P3-D6), in the installed app. Nothing in this list changes the network: don't confirm any repair or uninstall here (those are covered by Phase 3 and the uninstall section).

**Before installing** (Mac Studio, the app you have now, v1.13.1): note your four quick sets (names, IPs). Then install the draft (see *Installing*).

1. **Config kept:** open the app. The quick sets are exactly the ones you noted.
2. **Open Settings** three ways: the gear next to "Network Interface"; **It Hurts When IP → Settings…** in the menu bar; ⌘, (comma). ✕ and Esc close it.
3. **Widths:** drag the window to its narrowest (480 px) and widest (800 px). Settings has no sideways scrolling; at 800 px labels sit left and controls right.
4. **Footer:** the bottom of the main window has no Repair or Uninstall buttons any more.
5. **Modals from Settings:** Settings → **Repair network…** opens Repair on top of Settings; Esc closes it and you're back in Settings. Same for **Uninstall…** (Esc on the plan; don't confirm) and **Copy diagnostics…**.
6. **Advanced confirm:** tick **Advanced mode** → "Advanced options can change more of your network. Turn on?" → **Cancel** keeps it off. Tick again → **Turn on** → stays on. Quit the app (⌘Q), reopen, open Settings: still on. Untick it (no confirm) before step 7.
7. **Advanced off → no full reset:** with Advanced off, open **Repair Network…** from the menu bar. Only *Soft refresh* and *Reset one service* are offered, then a grey line: "Full network reset: turn on Advanced mode in Settings." Cancel.
8. **Advanced on → full reset offered:** turn Advanced on, open **Repair Network…** again: **Full network reset** (red) is back as the third choice. Open it to see the plan and the `RESET` box if you like, then **Cancel** (don't type RESET).
9. **Advanced details:** with Advanced on, Settings → Advanced shows "Loading details…", then: App build (version, git SHA, built time, release), Helper (status, version), Change journal (state and each recorded service with its time), Config (folder path, config.json exists). You can select and copy a value. Turn Advanced off: the details disappear.
10. **Copy diagnostics:** Settings → **Copy diagnostics…** → **Copy** → paste into TextEdit. Check it has the headings App, System, Helper, Journal, Network services, Selected service, Settings, and that Journal lists the services the app has changed (the release build asks the helper).
11. **Update check off:** untick **Check for updates when the app opens**, quit (⌘Q), reopen: no update popup. Tick it again afterwards. (The popup only ever shows when a newer published release exists, so this check only proves something while one does.)

Paste anything unexpected (with a screenshot) into the session.

### Phase 5 — Updates in Settings
**Where:** Jay's **Mac Studio** (P3-D6), in the installed draft. Nothing here changes the network: Check now is one read-only request to GitHub. The draft is version 1.13.1, the same as the latest published release, so the expected answer is "up to date".

1. **Opening is quiet:** open Settings. Updates shows Current version 1.13.1, the build line (git SHA + built time), Latest version **—** (or the version from an earlier Check now in this run of the app), and Last checked (**Never**, or the time of the last successful check, including the one the app makes when it opens). Opening Settings never contacts GitHub by itself.
2. **Check now:** click **Check now**. The button reads "Checking…" and is greyed out for a moment, then: Latest version **v1.13.1**, status "You're up to date." in green, Last checked "Today HH:MM" (your local time, now). No update popup appears.
3. **View changelog:** appears after the check; it opens the v1.13.1 release page in your browser.
4. **Update check at app start off:** in General, untick **Check for updates when the app opens**. Check now still works (step 2 again). Tick it back afterwards.
5. **Close and reopen:** close Settings (Esc) and open it again: the result from step 2 is still shown, and nothing is fetched again.
6. **Widths:** at 480 px the buttons wrap onto a second line if they don't fit, with no sideways scrolling; at 800 px the labels sit left and the values right.
7. **Offline (optional, skip unless it's easy):** only if the Mac Studio has another uplink the remote session doesn't need — **never** disconnect the connection the remote session uses. With no internet, Check now shows "Can't reach GitHub. Check your internet connection." in red, and Last checked stays as it was.

**Not tested yet: Download / Open in Installer.** Those buttons only show up when a newer published release has a `.pkg`, so there is nothing to download from the 1.13.1 draft. How to test this is still open in `docs/PROGRESS.md`: wait for v1.14.0 (Phase 7), or a one-off local test build stamped 1.13.0 that updates to the published v1.13.1. When it's tested: **Download vX.Y.Z** → progress bar with "x.x of y.y MB" and **Cancel** (Cancel → "Download cancelled.", and the Download button is back) → "Checking the download…" → "Downloaded to ~/Downloads/ItHurtsWhenIP-X.Y.Z.pkg (checksum verified)" or "(checksum not published — size checked)" → **Open in Installer** → "Installer opened. Follow its steps; the app will be replaced." Installing it is the normal install (see *Installing*).

## Manual Terminal fallback (no app, no script)
Use this when the app and the rescue script can't help (or you don't have the script). Every step runs on **any Mac, Terminal** and uses only macOS built-ins. Work down the list and stop when the network works. Use the service name exactly as listed in step 1, in quotes.

1. **List the services** (any Mac, Terminal; read-only):
   ```bash
   networksetup -listallnetworkservices
   ```
2. **Read a service's current state** (any Mac, Terminal; read-only). Repeat after each step below to see what changed:
   ```bash
   networksetup -getinfo "<service>"
   networksetup -getdnsservers "<service>"
   networksetup -getsearchdomains "<service>"
   ```
3. **Flush DNS** (any Mac, Terminal):
   ```bash
   sudo dscacheutil -flushcache; sudo killall -HUP mDNSResponder
   ```
4. **Put the service back on DHCP and clear its DNS servers and search domains** (any Mac, Terminal). This removes a static IP on that service; write it down first (step 2):
   ```bash
   sudo networksetup -setdhcp "<service>"
   sudo networksetup -setdnsservers "<service>" Empty
   sudo networksetup -setsearchdomains "<service>" Empty
   ```
5. **Turn the service off and on** (any Mac, Terminal):
   ```bash
   sudo networksetup -setnetworkserviceenabled "<service>" off
   sudo networksetup -setnetworkserviceenabled "<service>" on
   ```
6. **Stop the app's helper so nothing else changes** (any Mac, Terminal; only if the app is installed):
   ```bash
   sudo launchctl bootout system/com.ithurtswhenip.helper
   ```
   (On older macOS: `sudo launchctl unload /Library/LaunchDaemons/com.ithurtswhenip.helper.plist`.)

Note (from Phase 2 on): the helper records each service's original settings before it first changes it, in `/Library/Application Support/com.ithurtswhenip.helper/journal.json` (read it with `sudo cat "<that path>"`). The rescue script can put them back, see below.

### Then: the rescue script
If you can get online at all (phone hotspot, another Mac), get the rescue script and let it do the rest, with backups and a plan first:
1. In a browser, open the project's **Releases** page (`github.com/jaylenjupp/it-hurts-when-ip/releases`), open the newest release and download `ithurtswhenip-rescue.command` (it lands in `~/Downloads`). Copy it to a USB stick if the broken Mac can't get online.
2. Run it (any Mac, Terminal):
   ```bash
   sudo bash ~/Downloads/ithurtswhenip-rescue.command
   ```
   The menu offers soft refresh (4), reset one service (5), full network reset (6), restore a full-reset backup (7) and uninstall (2, 3). `--plan` with any action shows what it would do without changing anything; `--help` lists every option. If the app is still installed, the same script is at `"/Applications/It Hurts When IP.app/Contents/Resources/ithurtswhenip-rescue.command"`.
3. To put back the settings the app recorded and remove the app (any Mac, Terminal): `sudo bash ~/Downloads/ithurtswhenip-rescue.command --plan --network restore-journal` first (changes nothing), then the same with `--uninstall` instead of `--plan`.

Last resort without the script: a full network configuration reset by hand (you'll rejoin Wi-Fi and lose all custom network settings and Locations). The script's `--repair full` does the same with checksummed backups and can undo it, so prefer it. By hand (any Mac, Terminal):
```bash
sudo mkdir -p /Users/Shared/network-backup
cd /Library/Preferences/SystemConfiguration
sudo cp -p preferences.plist NetworkInterfaces.plist /Users/Shared/network-backup/
sudo cp -p com.apple.network.identification.plist com.apple.airport.preferences.plist /Users/Shared/network-backup/ 2>/dev/null
sudo rm preferences.plist NetworkInterfaces.plist
sudo rm -f com.apple.network.identification.plist com.apple.airport.preferences.plist
```
Then restart. macOS rebuilds these files. To undo, copy the files from `/Users/Shared/network-backup/` back into `/Library/Preferences/SystemConfiguration/` with `sudo cp -p` and restart.
