// Input validation. Everything a client sends is checked here before any
// write command runs. Pure functions, no I/O: the service list is parsed from
// text the caller already captured.

use crate::error::{ErrCode, HelperError};
use std::net::Ipv4Addr;

/// Longest service name accepted, in bytes.
pub const MAX_SERVICE_LEN: usize = 256;

const SERVICE_LIST_HEADER: &str = "An asterisk";

/// One line of `networksetup -listallnetworkservices`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Service {
    pub name: String,
    pub enabled: bool,
}

/// Parse `-listallnetworkservices`. Skips the "An asterisk (*) denotes…"
/// header and blank lines; a leading `*` marks a disabled service and is
/// stripped from its name. Names are otherwise kept exactly as printed
/// (spaces, `/`, parentheses, non-ASCII); one trailing `\r` is dropped.
pub fn parse_service_list(stdout: &str) -> Vec<Service> {
    stdout
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
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

/// Cheap checks on a service name, done before networksetup is run at all:
/// non-empty, at most `MAX_SERVICE_LEN` bytes, no control characters
/// (newline, tab, DEL, C1 controls…).
pub fn check_service_shape(name: &str) -> Result<(), HelperError> {
    if name.is_empty() {
        return Err(HelperError::new(
            ErrCode::InvalidService,
            "The network service name is empty.",
        ));
    }
    if name.len() > MAX_SERVICE_LEN {
        return Err(HelperError::new(
            ErrCode::InvalidService,
            format!(
                "The network service name is longer than {} bytes.",
                MAX_SERVICE_LEN
            ),
        ));
    }
    if name.chars().any(char::is_control) {
        return Err(HelperError::new(
            ErrCode::InvalidService,
            "The network service name contains control characters.",
        ));
    }
    Ok(())
}

/// The name must exactly match an enabled service from the list.
pub fn check_service_enabled(name: &str, services: &[Service]) -> Result<(), HelperError> {
    match services.iter().find(|s| s.name == name) {
        Some(s) if s.enabled => Ok(()),
        Some(_) => Err(HelperError::new(
            ErrCode::ServiceDisabled,
            format!(
                "The network service {:?} is disabled. Turn it on in System Settings > Network first.",
                name
            ),
        )),
        None => Err(HelperError::new(
            ErrCode::UnknownService,
            format!("There is no network service called {:?} on this Mac.", name),
        )),
    }
}

/// Strict dotted-quad parse: `std::net::Ipv4Addr` rejects surrounding
/// whitespace, leading zeros and anything that isn't exactly four octets.
fn parse_ipv4(text: &str) -> Option<Ipv4Addr> {
    text.parse::<Ipv4Addr>().ok()
}

/// The address to give the service. Any valid IPv4 except the obviously
/// nonsensical ones for a host: 0.0.0.0, 255.255.255.255, loopback 127/8 and
/// multicast 224/4. Link-local 169.254/16 is allowed.
pub fn parse_host_ip(text: &str) -> Result<Ipv4Addr, HelperError> {
    let ip = parse_ipv4(text).ok_or_else(|| {
        HelperError::new(
            ErrCode::InvalidIp,
            format!("IP address {:?} is not a valid IPv4 address.", text),
        )
    })?;
    let refused = if ip.is_unspecified() {
        Some("is not a usable address")
    } else if ip.is_broadcast() {
        Some("is the broadcast address")
    } else if ip.is_loopback() {
        Some("is a loopback address")
    } else if ip.is_multicast() {
        Some("is a multicast address")
    } else {
        None
    };
    match refused {
        Some(why) => Err(HelperError::new(
            ErrCode::InvalidIp,
            format!("IP address {} {} and can't be used here.", ip, why),
        )),
        None => Ok(ip),
    }
}

/// A subnet mask: valid IPv4, non-zero, with contiguous ones (/1 to /32).
pub fn parse_subnet_mask(text: &str) -> Result<Ipv4Addr, HelperError> {
    let mask = parse_ipv4(text).ok_or_else(|| {
        HelperError::new(
            ErrCode::InvalidSubnet,
            format!("Subnet mask {:?} is not a valid IPv4 address.", text),
        )
    })?;
    let bits = u32::from(mask);
    let host_bits = !bits;
    // Contiguous means the host part is all ones at the bottom: adding one
    // to it carries into a single bit, so it shares no bits with itself.
    let contiguous = host_bits & host_bits.wrapping_add(1) == 0;
    if bits == 0 || !contiguous {
        return Err(HelperError::new(
            ErrCode::InvalidSubnet,
            format!(
                "Subnet mask {} is not valid. Use a mask like 255.255.255.0.",
                mask
            ),
        ));
    }
    Ok(mask)
}

/// The router: empty (no router) or any valid IPv4. No further policy: an
/// out-of-subnet or odd gateway (even 0.0.0.0) is allowed, because saved quick
/// sets may already carry one and must keep working (owner decision P2-D3).
pub fn parse_gateway(text: &str) -> Result<Option<Ipv4Addr>, HelperError> {
    if text.is_empty() {
        return Ok(None);
    }
    parse_ipv4(text).map(Some).ok_or_else(|| {
        HelperError::new(
            ErrCode::InvalidGateway,
            format!(
                "Gateway {:?} is not a valid IPv4 address. Leave it empty for no router.",
                text
            ),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIX_DEFAULT_SERVICES: &str =
        include_str!("../../tools/fake-networksetup/fixtures/default/services.txt");
    const FIX_STUDIO_SERVICES: &str =
        include_str!("../../tools/fake-networksetup/fixtures/mac-studio/services.txt");

    fn svc(name: &str, enabled: bool) -> Service {
        Service {
            name: name.to_string(),
            enabled,
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
    fn service_shape_accepts_real_names() {
        for name in [
            "USB 10/100/1000 LAN",
            "USB Audio Device (R)",
            "RNDIS/Ethernet Gadget",
            "Café Ethernet",
            "'; rm -rf /'",
            "with \"quotes\"",
        ] {
            assert_eq!(check_service_shape(name), Ok(()), "{:?}", name);
        }
        assert_eq!(check_service_shape(&"a".repeat(MAX_SERVICE_LEN)), Ok(()));
    }

    #[test]
    fn service_shape_rejects_empty_long_and_control_chars() {
        let long = "a".repeat(MAX_SERVICE_LEN + 1);
        for name in [
            "",
            long.as_str(),
            "Wi-Fi\n",
            "Wi\nFi",
            "Wi-Fi\r",
            "Wi\tFi",
            "Wi\u{0}Fi",
            "Wi\u{7f}Fi",
            "Wi\u{85}Fi",
        ] {
            let err = check_service_shape(name).unwrap_err();
            assert_eq!(err.code, ErrCode::InvalidService, "{:?}", name);
        }
    }

    #[test]
    fn service_must_match_an_enabled_service_exactly() {
        let list = parse_service_list(FIX_DEFAULT_SERVICES);
        assert_eq!(check_service_enabled("Dante Primary", &list), Ok(()));
        assert_eq!(check_service_enabled("USB 10/100/1000 LAN", &list), Ok(()));

        let disabled = check_service_enabled("Old Adapter", &list).unwrap_err();
        assert_eq!(disabled.code, ErrCode::ServiceDisabled);
        assert!(
            disabled.message.contains("disabled"),
            "{}",
            disabled.message
        );

        for name in ["*Old Adapter", "dante primary", "Dante Primary ", "Nope"] {
            let err = check_service_enabled(name, &list).unwrap_err();
            assert_eq!(err.code, ErrCode::UnknownService, "{:?}", name);
        }
    }

    #[test]
    fn host_ip_accepts_normal_and_link_local_addresses() {
        for ip in [
            "10.0.0.1",
            "192.168.10.50",
            "169.254.10.20",
            "1.2.3.4",
            "172.16.0.255",
        ] {
            assert_eq!(parse_host_ip(ip).unwrap().to_string(), ip);
        }
    }

    #[test]
    fn host_ip_rejects_malformed_and_nonsensical_addresses() {
        for ip in [
            "",
            "1.2.3",
            "256.1.1.1",
            " 10.0.0.1",
            "10.0.0.1 ",
            "10.0.0.1\n",
            "10.0.0.1; reboot",
            "010.0.0.1",
            "1.2.3.4.5",
            "::1",
            "0.0.0.0",
            "255.255.255.255",
            "127.0.0.1",
            "127.1.2.3",
            "224.0.0.1",
            "239.255.255.250",
        ] {
            let err = parse_host_ip(ip).unwrap_err();
            assert_eq!(err.code, ErrCode::InvalidIp, "{:?}", ip);
        }
    }

    #[test]
    fn subnet_mask_accepts_contiguous_masks() {
        for mask in [
            "128.0.0.0",
            "255.0.0.0",
            "255.255.0.0",
            "255.255.252.0",
            "255.255.255.0",
            "255.255.255.252",
            "255.255.255.255",
        ] {
            assert_eq!(parse_subnet_mask(mask).unwrap().to_string(), mask);
        }
    }

    #[test]
    fn subnet_mask_rejects_non_contiguous_zero_and_malformed() {
        for mask in [
            "255.0.255.0",
            "0.0.0.0",
            "255.255.255.1",
            "0.255.255.255",
            "255.255.254.255",
            "",
            "255.255.255",
            " 255.255.255.0",
            "24",
        ] {
            let err = parse_subnet_mask(mask).unwrap_err();
            assert_eq!(err.code, ErrCode::InvalidSubnet, "{:?}", mask);
        }
    }

    #[test]
    fn gateway_empty_or_any_valid_ipv4() {
        assert_eq!(parse_gateway(""), Ok(None));
        // Odd and out-of-subnet gateways are allowed (P2-D3).
        for gw in ["0.0.0.0", "10.99.0.1", "192.168.10.1", "255.255.255.255"] {
            assert_eq!(parse_gateway(gw).unwrap().unwrap().to_string(), gw);
        }
    }

    #[test]
    fn gateway_rejects_malformed() {
        for gw in [
            " ",
            "1.2.3",
            "256.0.0.1",
            "10.0.0.1 ",
            "x",
            "10.0.0.1; reboot",
        ] {
            let err = parse_gateway(gw).unwrap_err();
            assert_eq!(err.code, ErrCode::InvalidGateway, "{:?}", gw);
        }
    }
}
