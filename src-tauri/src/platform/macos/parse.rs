// Pure parsers for `networksetup` output. No I/O: each function takes text
// that was already captured and returns plain data, so all of them can be
// tested against the real fixtures in tools/fake-networksetup/fixtures/.
// Keep them tolerant: output differs a little between macOS versions.

/// Shown in place of any field that isn't in the output.
pub const MISSING: &str = "—";

const SERVICE_LIST_HEADER: &str = "An asterisk";
const UNKNOWN_SERVICE_TEXT: &str = "is not a recognized network service";
const ERROR_PREFIX: &str = "** Error";

/// One line of `networksetup -listallnetworkservices`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Service {
    pub name: String,
    pub enabled: bool,
}

/// What the UI shows for a service, read from `networksetup -getinfo`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpInfo {
    pub ip: String,
    pub subnet: String,
    pub gateway: String,
    pub is_dhcp: bool,
}

impl IpInfo {
    /// The "couldn't read it" value: every field missing, not DHCP.
    pub fn fallback() -> Self {
        IpInfo {
            ip: MISSING.to_string(),
            subnet: MISSING.to_string(),
            gateway: MISSING.to_string(),
            is_dhcp: false,
        }
    }
}

/// Split on `\n` and drop one trailing `\r` per line; nothing else is touched.
fn lines(text: &str) -> impl Iterator<Item = &str> {
    text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l))
}

/// True if `networksetup` reported a failure in this text. It prints errors on
/// stdout (an unknown service gives "<name> is not a recognized network
/// service." then "** Error: ..."), so callers check stdout as well as stderr.
pub fn is_error_output(text: &str) -> bool {
    lines(text)
        .any(|l| l.trim_start().starts_with(ERROR_PREFIX) || l.contains(UNKNOWN_SERVICE_TEXT))
}

/// Parse `-listallnetworkservices`. Skips the "An asterisk (*) denotes…"
/// header and blank lines; a leading `*` marks a disabled service and is
/// stripped from its name. Names are otherwise kept exactly as printed
/// (spaces, `/`, parentheses, non-ASCII).
pub fn parse_service_list(stdout: &str) -> Vec<Service> {
    lines(stdout)
        .filter(|l| !l.trim().is_empty())
        .filter(|l| !l.starts_with(SERVICE_LIST_HEADER))
        .map(|l| match l.strip_prefix('*') {
            Some(name) => Service {
                name: name.to_string(),
                enabled: false,
            },
            None => Service {
                name: l.to_string(),
                enabled: true,
            },
        })
        .collect()
}

/// A configuration header is a line like "DHCP Configuration" or "Manual
/// Configuration": no colon, ends in "Configuration".
fn is_config_header(line: &str) -> bool {
    let t = line.trim();
    !t.contains(':') && t.ends_with("Configuration")
}

/// Parse `-getinfo <service>`. Missing fields stay "—". An empty `Router: `
/// gives `""` (what the app has always shown). `is_dhcp` comes from the
/// configuration header line ("DHCP Configuration", also "Manually Using DHCP
/// Router Configuration" as before); no header means not DHCP. Error output
/// gives the fallback.
pub fn parse_ip_info(stdout: &str) -> IpInfo {
    if is_error_output(stdout) {
        return IpInfo::fallback();
    }

    let mut info = IpInfo::fallback();
    let mut header_seen = false;

    for line in lines(stdout) {
        if let Some(v) = line.strip_prefix("IP address:") {
            info.ip = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("Subnet mask:") {
            info.subnet = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("Router:") {
            info.gateway = v.trim().to_string();
        } else if !header_seen && is_config_header(line) {
            header_seen = true;
            info.is_dhcp = line.contains("DHCP");
        }
    }

    info
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIX_DEFAULT_SERVICES: &str =
        include_str!("../../../../tools/fake-networksetup/fixtures/default/services.txt");
    const FIX_STUDIO_SERVICES: &str =
        include_str!("../../../../tools/fake-networksetup/fixtures/mac-studio/services.txt");
    const FIX_DEFAULT_WIFI: &str =
        include_str!("../../../../tools/fake-networksetup/fixtures/default/info/Wi-Fi.txt");
    const FIX_DEFAULT_DANTE: &str =
        include_str!("../../../../tools/fake-networksetup/fixtures/default/info/Dante_Primary.txt");
    const FIX_DEFAULT_OLD: &str =
        include_str!("../../../../tools/fake-networksetup/fixtures/default/info/Old_Adapter.txt");
    const FIX_DEFAULT_TB: &str = include_str!(
        "../../../../tools/fake-networksetup/fixtures/default/info/Thunderbolt_Bridge.txt"
    );
    const FIX_STUDIO_VPN: &str = include_str!(
        "../../../../tools/fake-networksetup/fixtures/mac-studio/info/VPN_Service.txt"
    );
    const FIX_STUDIO_IPHONE: &str =
        include_str!("../../../../tools/fake-networksetup/fixtures/mac-studio/info/iPhone_USB.txt");
    const FIX_STUDIO_UNKNOWN: &str = include_str!(
        "../../../../tools/fake-networksetup/fixtures/mac-studio/errors/getinfo-nonexistent.stdout.txt"
    );

    fn svc(name: &str, enabled: bool) -> Service {
        Service {
            name: name.to_string(),
            enabled,
        }
    }

    fn info(ip: &str, subnet: &str, gateway: &str, is_dhcp: bool) -> IpInfo {
        IpInfo {
            ip: ip.to_string(),
            subnet: subnet.to_string(),
            gateway: gateway.to_string(),
            is_dhcp,
        }
    }

    #[test]
    fn service_list_default_fixture() {
        assert_eq!(
            parse_service_list(FIX_DEFAULT_SERVICES),
            vec![
                svc("USB 10/100/1000 LAN", true),
                svc("Dante Primary", true),
                svc("Wi-Fi", true),
                svc("Thunderbolt Bridge", true),
                svc("Old Adapter", false),
            ]
        );
    }

    #[test]
    fn service_list_mac_studio_fixture() {
        let services = parse_service_list(FIX_STUDIO_SERVICES);
        let names: Vec<&str> = services.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "Ethernet",
                "Wi-Fi",
                "LAN USB Port",
                "USB Audio Device (R)",
                "USB 10/100/1000 LAN",
                "RNDIS/Ethernet Gadget",
                "iPhone USB",
                "Thunderbolt Bridge",
                "VPN Service",
            ]
        );
        assert!(services.iter().all(|s| s.enabled));
    }

    #[test]
    fn service_list_keeps_odd_names_and_strips_only_cr() {
        let text = "An asterisk (*) denotes that a network service is disabled.\r\n\
                    Café Ethernet (2)\r\n\
                    \r\n\
                    *  Spaced/Name \r\n\
                    \n";
        assert_eq!(
            parse_service_list(text),
            vec![svc("Café Ethernet (2)", true), svc("  Spaced/Name ", false)]
        );
    }

    #[test]
    fn ip_info_default_wifi_is_dhcp() {
        assert_eq!(
            parse_ip_info(FIX_DEFAULT_WIFI),
            info("10.20.30.40", "255.255.252.0", "10.20.28.1", true)
        );
    }

    #[test]
    fn ip_info_dante_manual_with_empty_router() {
        // "Router: " (trailing space) has always produced an empty gateway,
        // not "—". Keep it that way.
        assert_eq!(
            parse_ip_info(FIX_DEFAULT_DANTE),
            info("192.168.10.21", "255.255.255.0", "", false)
        );
    }

    #[test]
    fn ip_info_old_adapter_dhcp_header_without_ip_lines() {
        assert_eq!(parse_ip_info(FIX_DEFAULT_OLD), info("—", "—", "—", true));
    }

    #[test]
    fn ip_info_thunderbolt_link_local() {
        assert_eq!(
            parse_ip_info(FIX_DEFAULT_TB),
            info("169.254.12.34", "255.255.0.0", "", true)
        );
    }

    #[test]
    fn ip_info_vpn_service_without_header_is_not_dhcp() {
        let parsed = parse_ip_info(FIX_STUDIO_VPN);
        assert!(
            !parsed.is_dhcp,
            "no configuration header must mean not DHCP"
        );
        assert_eq!(parsed, info("—", "—", "—", false));
    }

    #[test]
    fn ip_info_null_ethernet_address_is_ignored() {
        assert!(FIX_STUDIO_IPHONE.contains("Ethernet Address: (null)"));
        assert_eq!(parse_ip_info(FIX_STUDIO_IPHONE), info("—", "—", "—", true));
    }

    #[test]
    fn ip_info_unknown_service_output_is_fallback() {
        assert!(is_error_output(FIX_STUDIO_UNKNOWN));
        assert_eq!(parse_ip_info(FIX_STUDIO_UNKNOWN), IpInfo::fallback());
    }

    #[test]
    fn ip_info_dhcp_only_from_header_line() {
        // A "DHCP" word outside the header no longer flips is_dhcp.
        let text = "Manual Configuration\nIP address: 10.0.0.2\nClient ID: DHCP-ish\n";
        assert!(!parse_ip_info(text).is_dhcp);
        // "Manually Using DHCP Router" was DHCP under the old rule; still is.
        let text = "Manually Using DHCP Router Configuration\nIP address: 10.0.0.2\n";
        assert!(parse_ip_info(text).is_dhcp);
    }

    #[test]
    fn error_detection() {
        assert!(is_error_output(
            "** Error: The parameters were not valid.\n"
        ));
        assert!(is_error_output(
            "Nope is not a recognized network service.\n"
        ));
        assert!(!is_error_output(FIX_DEFAULT_WIFI));
        assert!(!is_error_output(""));
    }
}
