# fake-networksetup

A stand-in for `/usr/sbin/networksetup` so the app, the helper and tests can run **without touching the real network** — which matters because the build host (Mac Studio) is Jay's Remote Control lifeline.

Runs on: the Mac Studio (or any machine with bash). Changes nothing outside its own state folder.

## Use it
```bash
# Mac Studio, repo root — run the dev app against the fake (env var wired in Phase 0)
IHWIP_NETWORKSETUP="$PWD/tools/fake-networksetup/fake-networksetup" npm run tauri dev

# Mac Studio, repo root — poke it directly
./tools/fake-networksetup/fake-networksetup -listallnetworkservices
./tools/fake-networksetup/fake-networksetup -setmanual "Dante Primary" 192.168.10.50 255.255.255.0
./tools/fake-networksetup/fake-networksetup -getinfo "Dante Primary"
./tools/fake-networksetup/fake-networksetup fake-log          # every call made so far
IHWIP_FAKE_RESET=1 ./tools/fake-networksetup/fake-networksetup -version   # wipe state
```

| Variable | Meaning |
|---|---|
| `IHWIP_FAKE_FIXTURES` | fixture set (default `fixtures/default`) |
| `IHWIP_FAKE_STATE` | private state dir (default `$TMPDIR/ihwip-fake-networksetup`); give each test its own |
| `IHWIP_FAKE_FAIL` | comma list of subcommands to fail, e.g. `setmanual` |
| `IHWIP_FAKE_RESET=1` | start fresh from fixtures |

## Fixtures
`fixtures/default/` has five services: a DHCP USB LAN, a static "Dante Primary" with no router, Wi-Fi, a link-local Thunderbolt Bridge, and a disabled `*Old Adapter`. Info files are named by service with non `[A-Za-z0-9-]` runs replaced by `_`.

**TODO(verify):** capture real output from Jay's Macs (read-only commands are fine on the Mac Studio: `networksetup -listallnetworkservices`, `-getinfo "<service>"`, `-listallhardwareports`) and add it as `fixtures/mac-studio/` and `fixtures/travel-macbook/`, so parsers are tested against what macOS really prints, including error text and exit codes.

## In Rust tests
Prefer the in-process `CommandRunner` fake (Phase 0) for unit tests. Use this script for integration tests that spawn a real process: point the code at it via `IHWIP_NETWORKSETUP` and give each test its own `IHWIP_FAKE_STATE`.
