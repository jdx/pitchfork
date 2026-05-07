//! LAN IP address detection for the reverse proxy.
//!
//! Detects the local IPv4 address used for the default route by opening a UDP
//! "connection" to a public address (1.1.1.1:53).  No data is sent; the OS
//! routing table determines which local address to use, and we read it back
//! via `socket.local_addr()`.
//!
//! On Unix, falls back to interface enumeration via `nix::ifaddrs` when the UDP
//! probe fails (e.g. no route to the internet).  On Windows, only the UDP
//! probe is available.

use std::net::{Ipv4Addr, SocketAddrV4};

/// Probe target: Cloudflare DNS on a well-known anycast address.
const PROBE_HOST: Ipv4Addr = Ipv4Addr::new(1, 1, 1, 1);
const PROBE_PORT: u16 = 53;

/// Detect the LAN IPv4 address of the default outbound route.
///
/// Uses a UDP connect probe to determine which local address the OS would use
/// to reach the public internet, then validates it against the interface list
/// to exclude virtual/internal interfaces.
///
/// Returns `None` if:
/// - Only loopback addresses are available
/// - The detected interface is virtual (Docker, Hyper-V, bridges)
/// - No route to the internet exists
pub async fn detect_lan_ip() -> Option<Ipv4Addr> {
    // Try the UDP probe first (most reliable for default route).
    if let Some(ip) = probe_default_route().await {
        if let Some(iface) = find_interface_for_ip(&ip) {
            if !is_virtual_interface(&iface) {
                return Some(ip);
            }
        } else {
            // IP was found but no matching interface — could still be valid
            // (e.g. the interface disappeared between probe and enumeration).
            // Accept it as long as it's not loopback.
            if !ip.is_loopback() && !ip.is_link_local() {
                return Some(ip);
            }
        }
    }

    // Fallback: pick the first non-loopback, non-virtual IPv4 from ifaddrs.
    fallback_interface_ip()
}

/// Detect LAN IP, returning `None` if it hasn't changed since `last`.
pub async fn detect_lan_ip_if_changed(last: Ipv4Addr) -> Option<Ipv4Addr> {
    let current = detect_lan_ip().await?;
    (current != last).then_some(current)
}

/// Probe the default route by opening a UDP "connection" to a public address.
///
/// The OS picks the source address based on its routing table, which is exactly
/// the LAN IP we want.  No packets are actually sent.
async fn probe_default_route() -> Option<Ipv4Addr> {
    let sock = tokio::net::UdpSocket::bind("0.0.0.0:0").await.ok()?;
    let dest = SocketAddrV4::new(PROBE_HOST, PROBE_PORT);
    // connect() on a UDP socket doesn't send anything — it just sets the
    // default destination and lets the OS pick the source address.
    sock.connect(dest).await.ok()?;
    let local = sock.local_addr().ok()?;
    let ip = local.ip();
    match ip {
        std::net::IpAddr::V4(v4) if !v4.is_unspecified() && !v4.is_loopback() => Some(v4),
        _ => None,
    }
}

/// A network interface matched from `getifaddrs`.
struct InterfaceInfo {
    name: String,
    ip: Ipv4Addr,
    is_loopback: bool,
}

// -- Unix: interface enumeration via nix::ifaddrs ----------------------------

#[cfg(unix)]
fn find_interface_for_ip(target: &Ipv4Addr) -> Option<InterfaceInfo> {
    let addrs = nix::ifaddrs::getifaddrs().ok()?;
    for iface in addrs {
        let flags = iface.flags;
        let is_loopback = flags.contains(nix::net::if_::InterfaceFlags::IFF_LOOPBACK);
        let ip = match iface
            .address
            .as_ref()
            .and_then(|a| a.as_sockaddr_in().map(|sa| sa.ip()))
        {
            Some(ip) => ip,
            None => continue,
        };
        if &ip == target {
            return Some(InterfaceInfo {
                name: iface.interface_name.clone(),
                ip,
                is_loopback,
            });
        }
    }
    None
}

#[cfg(unix)]
fn is_virtual_interface(iface: &InterfaceInfo) -> bool {
    if iface.is_loopback {
        return true;
    }
    if iface.ip.is_link_local() {
        return true;
    }
    let name = iface.name.to_lowercase();
    // Docker virtual ethernet pairs
    if name.starts_with("veth") || name.starts_with("br-") || name.starts_with("docker") {
        return true;
    }
    // Libvirt bridges
    if name.starts_with("virbr") {
        return true;
    }
    // Windows Hyper-V (seen in WSL2)
    if name.starts_with("vethernet") || name.starts_with("bridge") {
        return true;
    }
    false
}

#[cfg(unix)]
fn fallback_interface_ip() -> Option<Ipv4Addr> {
    let addrs = nix::ifaddrs::getifaddrs().ok()?;
    for iface in addrs {
        let flags = iface.flags;
        if !flags.contains(nix::net::if_::InterfaceFlags::IFF_UP) {
            continue;
        }
        if flags.contains(nix::net::if_::InterfaceFlags::IFF_LOOPBACK) {
            continue;
        }
        let ip = match iface
            .address
            .as_ref()
            .and_then(|a| a.as_sockaddr_in().map(|sa| sa.ip()))
        {
            Some(ip) => ip,
            None => continue,
        };
        if ip.is_loopback() || ip.is_link_local() {
            continue;
        }
        let info = InterfaceInfo {
            name: iface.interface_name.clone(),
            ip,
            is_loopback: false,
        };
        if !is_virtual_interface(&info) {
            return Some(ip);
        }
    }
    None
}

// -- Windows: no nix::ifaddrs, skip interface enumeration --------------------

#[cfg(not(unix))]
fn find_interface_for_ip(_target: &Ipv4Addr) -> Option<InterfaceInfo> {
    None
}

#[cfg(not(unix))]
fn is_virtual_interface(_iface: &InterfaceInfo) -> bool {
    false
}

#[cfg(not(unix))]
fn fallback_interface_ip() -> Option<Ipv4Addr> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn test_is_virtual_interface_loopback() {
        let iface = InterfaceInfo {
            name: "lo".to_string(),
            ip: Ipv4Addr::new(127, 0, 0, 1),
            is_loopback: true,
        };
        assert!(is_virtual_interface(&iface));
    }

    #[cfg(unix)]
    #[test]
    fn test_is_virtual_interface_docker() {
        for name in &["veth1234", "br-abc", "docker0"] {
            let iface = InterfaceInfo {
                name: name.to_string(),
                ip: Ipv4Addr::new(172, 17, 0, 1),
                is_loopback: false,
            };
            assert!(
                is_virtual_interface(&iface),
                "expected {name} to be virtual"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn test_is_virtual_interface_normal() {
        let iface = InterfaceInfo {
            name: "eth0".to_string(),
            ip: Ipv4Addr::new(192, 168, 1, 42),
            is_loopback: false,
        };
        assert!(!is_virtual_interface(&iface));
    }

    #[cfg(unix)]
    #[test]
    fn test_is_virtual_interface_link_local() {
        let iface = InterfaceInfo {
            name: "en0".to_string(),
            ip: Ipv4Addr::new(169, 254, 1, 1),
            is_loopback: false,
        };
        assert!(is_virtual_interface(&iface));
    }
}
