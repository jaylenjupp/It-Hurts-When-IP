---
name: macos-helper
description: Use when touching helper-tool/, the helper's Unix-socket protocol, src-tauri/src/platform/macos.rs, the LaunchDaemon plist, or anything that runs as root. Guarantees every privileged action is allow-listed, validated, journaled and testable without root or a real network.
---
# macOS privileged helper

## Contract / core rule
The helper is the **only** thing in the app that runs as root, and it only does what's on its allowlist, with validated arguments passed as argv (never through a shell). Nothing in this area is ever exercised against the real network on the build host.

## Details
**Today (v1.13.1)** — see `reference/AUDIT-2026-11.md` for line-level findings:
- Binary `/Library/PrivilegedHelperTools/com.ithurtswhenip.helper`, LaunchDaemon `/Library/LaunchDaemons/com.ithurtswhenip.helper.plist` (`RunAtLoad`, `KeepAlive`), logs `/var/log/com.ithurtswhenip.helper{,.error}.log`.
- Socket `/var/run/com.ithurtswhenip.helper.sock`, one JSON object per line in, one per line out: `{"cmd":"set_static_ip","interface","ip","subnet","gateway"}`, `{"cmd":"set_dhcp","interface"}` → `{"ok": "..."}` or `{"error": "..."}`.
- Socket is `chmod 777` → any local process can make root change network config. **Fix in Phase 2.**
- No input validation; `networksetup` stderr only (errors can arrive on stdout).
- The app's `get_ip_info` runs `sh -c "networksetup -getinfo '<iface>'"` → shell injection via service name. **Fix in Phase 2** (argv, no shell).
- `get_interfaces` filters with a shell pipeline and doesn't strip the `*` disabled-service prefix.

**Target design (Phase 2):**
- `CommandRunner` trait (`run(program, args) -> Output`) in both crates; real impl uses `std::process::Command`, test impl records calls and returns canned output. The `networksetup` path comes from `IHWIP_NETWORKSETUP` in debug builds only; release builds hard-code `/usr/sbin/networksetup`.
- Protocol: add `"v": 2` to every request; helper answers `{"error":"unsupported_version"}` to unknown versions. New commands: `ping` (returns helper version + protocol version), `journal_status`, `restore_service`, `repair_soft` (Phase 3). Keep v1 `set_static_ip`/`set_dhcp` working until the app that sends them is gone.
- Validation: service name must exactly match a line from `networksetup -listallnetworkservices` (minus `*`); IPv4/mask via `std::net::Ipv4Addr` parse; mask must be contiguous; gateway optional but must be in-subnet if given (TODO: confirm with Jay whether out-of-subnet gateways are ever used on show networks before enforcing).
- Socket permissions: owned `root:admin`, mode `0660` (TODO(verify) the console user is in `admin` on colleagues' Macs; if not, fall back to checking peer credentials with `getpeereid` against the console user — would need `libc` → ask before adding). Never `0777`.
- Journal: before the first change to a service, write its original `-getinfo`, `-getdnsservers`, `-getsearchdomains` output and a timestamp to `/Library/Application Support/com.ithurtswhenip.helper/journal.json` (root-owned 0644, atomic write via temp+rename). Never overwrite an existing original for a service. Schema is versioned (`"schema": 1`).
- Capture both stdout and stderr from `networksetup`; treat a line starting `** Error` on either as failure.

## Known quirks
- `networksetup` takes the **service name** ("USB 10/100/1000 LAN"), not the BSD device (`en7`). Names can contain `/`, spaces and non-ASCII.
- `-setmanual` with an empty router: TODO(verify) what macOS does with `""` as the 4th arg vs omitting it. Test on Jay's travel Mac, not here.
- Writes can return before the system state settles; read back with a short poll (like `waitForExpectedState` on Windows) before reporting success.
- `launchctl load/unload` are legacy; `bootstrap`/`bootout system/<label>` are current. Pkg scripts may keep `load` for 10.15 compatibility — TODO(verify) on the oldest target macOS.
- `KeepAlive=true` means a crashing helper restarts in a loop; a bad helper build can't be "stopped" without root. Another reason not to install dev builds on the build host.

## Tests
- Unit-test parsing, validation and request handling with the `CommandRunner` fake — assert the exact argv the helper would run.
- Integration: run the helper binary against a temp socket path (make the path configurable in debug builds) and `tools/fake-networksetup` via `IHWIP_NETWORKSETUP`; send JSON lines from the test; assert responses and `fake-log` contents.
- Journal tests write to a temp dir (path injectable in debug builds).
- Never run the real helper, real socket or `sudo` on the build host.
