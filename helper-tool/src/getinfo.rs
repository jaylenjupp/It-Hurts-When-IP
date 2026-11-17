// Reads `networksetup -getinfo` text the same strict way as the rescue
// script's info_view / parse_info (recovery/ithurtswhenip-rescue.command), so
// the helper and the script agree on what a recorded original is and on when
// a service matches it. Pure functions, no I/O.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Dhcp,
    Manual,
    /// Anything else (BOOTP, "Manually Using DHCP Router", empty text…).
    Unknown,
}

/// The IPv4 part of a -getinfo text. An address is None when the line is
/// missing or its value is empty, "-", "(null)" or "none".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    pub mode: Mode,
    pub ip: Option<String>,
    pub mask: Option<String>,
    pub router: Option<String>,
}

/// Mode: the first non-blank line, trimmed, must be exactly
/// "DHCP Configuration" or "Manual Configuration". Addresses: the first line
/// that starts with "IP address:" / "Subnet mask:" / "Router:" (not
/// indented, so "IPv6 Router:" never counts) whose trimmed value is non-empty
/// and not "-".
pub fn view(text: &str) -> View {
    let lines: Vec<&str> = text.split('\n').collect();
    let first = lines
        .iter()
        .map(|l| trim(l))
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let mode = match first {
        "DHCP Configuration" => Mode::Dhcp,
        "Manual Configuration" => Mode::Manual,
        _ => Mode::Unknown,
    };
    View {
        mode,
        ip: value(&lines, "IP address:"),
        mask: value(&lines, "Subnet mask:"),
        router: value(&lines, "Router:"),
    }
}

fn value(lines: &[&str], prefix: &str) -> Option<String> {
    let v = lines
        .iter()
        .filter_map(|l| l.strip_prefix(prefix))
        .map(trim)
        .find(|v| !v.is_empty() && *v != "-")?;
    match v {
        "(null)" | "none" => None,
        v => Some(v.to_string()),
    }
}

/// ASCII whitespace only, like bash's [[:space:]] under LC_ALL=C.
fn trim(s: &str) -> &str {
    s.trim_matches(|c| matches!(c, ' ' | '\t' | '\n' | '\x0b' | '\x0c' | '\r'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn some(s: &str) -> Option<String> {
        Some(s.to_string())
    }

    #[test]
    fn reads_the_default_fixtures() {
        let dante = view(include_str!(
            "../../tools/fake-networksetup/fixtures/default/info/Dante_Primary.txt"
        ));
        assert_eq!(
            dante,
            View {
                mode: Mode::Manual,
                ip: some("192.168.10.21"),
                mask: some("255.255.255.0"),
                router: None,
            }
        );
        let wifi = view(include_str!(
            "../../tools/fake-networksetup/fixtures/default/info/Wi-Fi.txt"
        ));
        assert_eq!(
            wifi,
            View {
                mode: Mode::Dhcp,
                ip: some("10.20.30.40"),
                mask: some("255.255.252.0"),
                router: some("10.20.28.1"),
            }
        );
        let old = view(include_str!(
            "../../tools/fake-networksetup/fixtures/default/info/Old_Adapter.txt"
        ));
        assert_eq!(
            old,
            View {
                mode: Mode::Dhcp,
                ip: None,
                mask: None,
                router: None,
            }
        );
        // "IPv6 Router: none" must not be read as the router.
        let mac = view(include_str!(
            "../../tools/fake-networksetup/fixtures/mac-studio/info/Ethernet.txt"
        ));
        assert_eq!(mac.mode, Mode::Dhcp);
        assert_eq!(mac.router, None);
    }

    #[test]
    fn mode_is_the_first_non_blank_line_exactly() {
        for (text, mode) in [
            ("DHCP Configuration\n", Mode::Dhcp),
            (
                "\n  \r\n  Manual Configuration \r\nIP address: 1.2.3.4\n",
                Mode::Manual,
            ),
            ("BOOTP Configuration\n", Mode::Unknown),
            ("Manually Using DHCP Router Configuration\n", Mode::Unknown),
            ("Manual Configuration (x)\n", Mode::Unknown),
            ("dhcp configuration\n", Mode::Unknown),
            ("IP address: 1.2.3.4\nManual Configuration\n", Mode::Unknown),
            ("", Mode::Unknown),
            ("\n\n", Mode::Unknown),
        ] {
            assert_eq!(view(text).mode, mode, "{:?}", text);
        }
    }

    #[test]
    fn addresses_first_real_value_wins_and_empty_words_mean_none() {
        let v = view(
            "Manual Configuration\r\n\
             IP address: \r\n\
             IP address: -\n\
             IP address:  10.0.0.5 \r\n\
             IP address: 10.0.0.6\n\
             Subnet mask: (null)\n\
             Subnet mask: 255.0.0.0\n\
             Router: none\n\
             IPv6 Router: fe80::1\n",
        );
        assert_eq!(v.ip, some("10.0.0.5"));
        // "(null)" is a real value for parse_info, so it wins and means none.
        assert_eq!(v.mask, None);
        assert_eq!(v.router, None);

        // Indented lines don't count (parse_info matches the raw line).
        let v = view("Manual Configuration\n  IP address: 10.0.0.5\nRouter: 10.0.0.1");
        assert_eq!(v.ip, None);
        assert_eq!(v.router, some("10.0.0.1"));
    }
}
