# fixtures/mac-studio

Real `networksetup` output from Jay's Mac Studio (the build host), captured read-only and redacted for this public repo. These are parser fixtures, so apart from the redactions below they keep exactly what macOS printed: line order, blank lines, wording, trailing spaces (`Client ID: `) and the final newline. Because the redactions substitute addresses and two service names, the files are no longer byte-for-byte copies of the capture; only the line structure and wording are unchanged.

- Captured: 2026-11-13
- Machine: Mac Studio (M1), `arm64`
- macOS: 26.7.1 (build 25G241)

## Commands (Mac Studio, Terminal, no sudo)
Only these three read-only commands were run. stdout and stderr were captured separately to a temp dir outside the repo, then the redacted copies were written here.
```bash
/usr/sbin/networksetup -listallnetworkservices            > services.txt
/usr/sbin/networksetup -listallhardwareports              > hardwareports.txt
/usr/sbin/networksetup -getinfo "<service>"               > info/<Service>.txt   # once per line of services.txt (header skipped, leading * stripped)
/usr/sbin/networksetup -getinfo "No Such Service IHWIP"   > errors/getinfo-nonexistent.stdout.txt 2> errors/getinfo-nonexistent.stderr.txt; echo $? > errors/getinfo-nonexistent.exit
```
Every `-getinfo` for a listed service exited 0 with empty stderr. Info filenames follow the fake's rule: the service name (without any `*`) with each run of characters outside `[A-Za-z0-9-]` replaced by `_` (`USB Audio Device (R)` → `USB_Audio_Device_R_.txt`).

## Redaction
- Every MAC address (`Ethernet Address:`, `Wi-Fi ID:` and anywhere else) → `xx:xx:xx:xx:xx:xx`.
- `Client ID` values → would be `REDACTED`. All were empty in this capture, so nothing changed.
- Private LAN IPv4 addresses → replaced with `192.0.2.x` (documentation range), keeping the last octet. Subnet masks are unchanged.
- Public IPv4 and global or MAC-derived IPv6 → would be documentation values (`192.0.2.x`, `2001:db8::x`). None were present, so nothing changed.
- Two service names that identified specific products → replaced with generic names: a VPN service is now `VPN Service`, and a USB audio device is now `USB Audio Device (R)` (keeps the trailing `(R)` and parentheses as a parser edge case). Their info files were renamed to match the fake's filename rule. All other service and hardware-port names are kept as printed.

## What this set shows (useful for parsers)
- No disabled (`*`) services on this Mac.
- `services.txt` and `hardwareports.txt` don't match one-to-one: services for USB adapters that aren't plugged in (`LAN USB Port`, `USB 10/100/1000 LAN`, `iPhone USB`…) have no hardware port, and the ports list generic names like `Ethernet Adapter (en6)` and `Thunderbolt 1`.
- Unplugged adapters print `Ethernet Address: (null)`.
- `Ethernet` (en0, no link) has a `DHCP Configuration` header but no `IP address` / `Subnet mask` / `Router` lines.
- `VPN Service` has no configuration header and no hardware address line at all: just the three IPv6 lines.
- `Thunderbolt Bridge` has no hardware address line in `-getinfo`, but does have an address in `-listallhardwareports`.
- `USB Audio Device (R)` and `LAN USB Port` have no `IPv6: Automatic` line.
- An unknown service name exits **4**, prints nothing on stderr, and prints two lines on **stdout**: `<name> is not a recognized network service.` then `** Error: The parameters were not valid.`

## Use with the fake
`IHWIP_FAKE_FIXTURES` takes a directory path. The fake reads only `services.txt`, `hardwareports.txt` and `info/*.txt`; it ignores this README and `errors/`.
```bash
# Mac Studio, repo root
IHWIP_FAKE_FIXTURES="$PWD/tools/fake-networksetup/fixtures/mac-studio" \
IHWIP_FAKE_STATE="$(mktemp -d)/state" \
  ./tools/fake-networksetup/fake-networksetup -getinfo "Wi-Fi"
```
