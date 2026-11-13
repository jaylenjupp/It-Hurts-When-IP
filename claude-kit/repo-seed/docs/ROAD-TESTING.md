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

## Per-phase test lists
Claude adds a "what to test" list to each phase report and to the draft release notes. Tick through it and reply with results; Claude records them in `docs/PROGRESS.md`.

## If the network is broken — without the app
Work down this list; stop when it works. These are standard macOS commands, nothing app-specific.

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
6. **Remove the app completely** (from Phase 1 on): `sudo bash ~/Downloads/ithurtswhenip-rescue.command` and pick *Uninstall*.
