# Testing draft builds on the travel Mac

Everything here runs **on your travel MacBook**, by you. Claude never installs or runs network changes; it builds, you test.

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

## If the network is broken — without the app
Work down this list; stop when it works. These are standard macOS commands, nothing app-specific.

Note (from Phase 2 on): the helper records each service's original settings before it first changes it, in `/Library/Application Support/com.ithurtswhenip.helper/journal.json` (read it with `sudo cat "<that path>"`). `sudo bash ~/Downloads/ithurtswhenip-rescue.command --uninstall --network restore-journal` puts them back (and removes the app; try `--plan --network restore-journal` first, it changes nothing).

1. **Stop the helper so nothing else changes:**
   ```bash
   sudo launchctl bootout system/com.ithurtswhenip.helper
   ```
   (On older macOS: `sudo launchctl unload /Library/LaunchDaemons/com.ithurtswhenip.helper.plist`.)
2. **Put the affected service back on DHCP** (name exactly as listed):
   ```bash
   networksetup -listallnetworkservices
   sudo networksetup -setdhcp "Wi-Fi"
   ```
3. **Flush DNS:**
   ```bash
   sudo dscacheutil -flushcache; sudo killall -HUP mDNSResponder
   ```
4. **Turn the service off and on:**
   ```bash
   sudo networksetup -setnetworkserviceenabled "Wi-Fi" off
   sudo networksetup -setnetworkserviceenabled "Wi-Fi" on
   ```
5. **Full network configuration reset** (last resort — you'll rejoin Wi-Fi and lose all custom network settings and Locations):
   ```bash
   sudo mkdir -p /Users/Shared/network-backup
   cd /Library/Preferences/SystemConfiguration
   sudo cp -p preferences.plist NetworkInterfaces.plist /Users/Shared/network-backup/
   sudo cp -p com.apple.network.identification.plist com.apple.airport.preferences.plist /Users/Shared/network-backup/ 2>/dev/null
   sudo rm preferences.plist NetworkInterfaces.plist
   sudo rm -f com.apple.network.identification.plist com.apple.airport.preferences.plist
   ```
   Then restart. macOS rebuilds these files. TODO(verify) the file list against your macOS version before you're relying on it.
6. **Remove the app completely** (from Phase 1 on): `sudo bash ~/Downloads/ithurtswhenip-rescue.command` and pick `2` or `3` — see *Uninstalling (rescue script)* above.
