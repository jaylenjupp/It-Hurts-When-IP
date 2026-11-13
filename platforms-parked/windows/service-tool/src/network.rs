// Win32 IP Helper API wrappers for fast network configuration.
//
// As of the netsh refactor, set_static_ip no longer uses these — it goes
// through netsh because Win32 IP Helper didn't disable the DHCP flag or
// surface its routes in Get-NetRoute. The wrappers stay because set_dhcp
// still uses them defensively to clean up addresses and routes left behind
// by older installs that did use Win32 directly.

use std::mem::zeroed;
use std::net::Ipv4Addr;

use windows_sys::Win32::NetworkManagement::IpHelper::{
    ConvertInterfaceAliasToLuid,
    CreateIpForwardEntry2,
    CreateUnicastIpAddressEntry,
    DeleteIpForwardEntry2,
    DeleteUnicastIpAddressEntry,
    FreeMibTable,
    GetIpForwardTable2,
    GetUnicastIpAddressTable,
    InitializeIpForwardEntry,
    InitializeUnicastIpAddressEntry,
    MIB_IPFORWARD_ROW2,
    MIB_IPFORWARD_TABLE2,
    MIB_UNICASTIPADDRESS_ROW,
    MIB_UNICASTIPADDRESS_TABLE,
};
use windows_sys::Win32::NetworkManagement::Ndis::NET_LUID_LH;
use windows_sys::Win32::Networking::WinSock::{
    AF_INET, IN_ADDR, IN_ADDR_0, SOCKADDR_IN, SOCKADDR_INET,
};

/// Convert a friendly interface alias (e.g. "Ethernet", "Wi-Fi") to a LUID.
pub fn alias_to_luid(alias: &str) -> Result<NET_LUID_LH, u32> {
    let wide: Vec<u16> = alias.encode_utf16().chain(Some(0)).collect();
    let mut luid: NET_LUID_LH = unsafe { zeroed() };
    let result = unsafe { ConvertInterfaceAliasToLuid(wide.as_ptr(), &mut luid) };
    if result == 0 { Ok(luid) } else { Err(result) }
}

/// Build a SOCKADDR_INET from an IPv4 address.
#[allow(dead_code)]
fn ipv4_sockaddr(addr: Ipv4Addr) -> SOCKADDR_INET {
    let mut sa: SOCKADDR_INET = unsafe { zeroed() };
    let octets = addr.octets();
    let s_addr: u32 =
        ((octets[0] as u32) << 24)
        | ((octets[1] as u32) << 16)
        | ((octets[2] as u32) << 8)
        |  (octets[3] as u32);
    sa.Ipv4 = SOCKADDR_IN {
        sin_family: AF_INET as u16,
        sin_port: 0,
        sin_addr: IN_ADDR { S_un: IN_ADDR_0 { S_addr: s_addr.to_be() } },
        sin_zero: [0; 8],
    };
    sa
}

/// Add a static IPv4 address to the interface.
/// (Kept for completeness; set_static_ip now uses netsh.)
#[allow(dead_code)]
pub fn add_unicast_ip(luid: NET_LUID_LH, ip: Ipv4Addr, prefix_len: u8) -> Result<(), u32> {
    let mut row: MIB_UNICASTIPADDRESS_ROW = unsafe { zeroed() };
    unsafe { InitializeUnicastIpAddressEntry(&mut row); }
    row.Address = ipv4_sockaddr(ip);
    row.InterfaceLuid = luid;
    row.OnLinkPrefixLength = prefix_len;
    let result = unsafe { CreateUnicastIpAddressEntry(&row) };
    if result == 0 { Ok(()) } else { Err(result) }
}

/// Remove every IPv4 unicast address from the interface.
pub fn remove_all_unicast_ipv4(luid: NET_LUID_LH) -> Result<(), u32> {
    let mut table_ptr: *mut MIB_UNICASTIPADDRESS_TABLE = std::ptr::null_mut();
    let result = unsafe { GetUnicastIpAddressTable(AF_INET as u16, &mut table_ptr) };
    if result != 0 { return Err(result); }

    let target = unsafe { luid.Value };

    let entries: &[MIB_UNICASTIPADDRESS_ROW] = unsafe {
        let table = &*table_ptr;
        std::slice::from_raw_parts(table.Table.as_ptr(), table.NumEntries as usize)
    };

    for entry in entries {
        if unsafe { entry.InterfaceLuid.Value == target } {
            unsafe { DeleteUnicastIpAddressEntry(entry); }
        }
    }

    unsafe { FreeMibTable(table_ptr as *mut _); }
    Ok(())
}

/// Add an IPv4 default route (0.0.0.0/0) via the given gateway.
/// (Kept for completeness; set_static_ip now uses netsh.)
#[allow(dead_code)]
pub fn add_default_route(luid: NET_LUID_LH, gateway: Ipv4Addr) -> Result<(), u32> {
    let mut row: MIB_IPFORWARD_ROW2 = unsafe { zeroed() };
    unsafe { InitializeIpForwardEntry(&mut row); }
    row.InterfaceLuid = luid;
    row.DestinationPrefix.Prefix = ipv4_sockaddr(Ipv4Addr::new(0, 0, 0, 0));
    row.DestinationPrefix.PrefixLength = 0;
    row.NextHop = ipv4_sockaddr(gateway);
    row.Metric = 1;
    let result = unsafe { CreateIpForwardEntry2(&row) };
    if result == 0 { Ok(()) } else { Err(result) }
}

/// Remove every IPv4 default route (0.0.0.0/0) for the interface.
pub fn remove_all_default_routes_ipv4(luid: NET_LUID_LH) -> Result<(), u32> {
    let mut table_ptr: *mut MIB_IPFORWARD_TABLE2 = std::ptr::null_mut();
    let result = unsafe { GetIpForwardTable2(AF_INET as u16, &mut table_ptr) };
    if result != 0 { return Err(result); }

    let target = unsafe { luid.Value };

    let entries: &[MIB_IPFORWARD_ROW2] = unsafe {
        let table = &*table_ptr;
        std::slice::from_raw_parts(table.Table.as_ptr(), table.NumEntries as usize)
    };

    for entry in entries {
        if unsafe { entry.InterfaceLuid.Value == target } && entry.DestinationPrefix.PrefixLength == 0 {
            unsafe { DeleteIpForwardEntry2(entry); }
        }
    }

    unsafe { FreeMibTable(table_ptr as *mut _); }
    Ok(())
}

/// Enable DHCP-managed addressing on the IPv4 interface.
///
/// Implemented via netsh because Win32 has no clean API for this — the DHCP
/// client service owns the state, not MIB_IPINTERFACE_ROW. Netsh writes its
/// errors to stdout (not stderr), so we capture both — otherwise a failure
/// surfaces to the user as a bare "failed to enable DHCP" with no detail.
pub fn enable_dhcp_ipv4(interface_alias: &str) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    use std::process::Command;
    const CREATE_NO_WINDOW: u32 = 0x08000000;

    let output = Command::new("netsh")
        .args([
            "interface",
            "ipv4",
            "set",
            "address",
            &format!("name={}", interface_alias),
            "source=dhcp",
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| e.to_string())?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let msg = if !stderr.is_empty() {
            stderr
        } else if !stdout.is_empty() {
            stdout
        } else {
            format!("netsh exit code {:?}", output.status.code())
        };
        Err(msg)
    }
}