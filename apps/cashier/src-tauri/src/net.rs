//! Network helpers — currently just LAN IP detection for the first-run wizard
//! / cross-device hand-off flows. Kept tiny and dependency-light.
use std::net::IpAddr;

/// Best-guess LAN IPv4 for this host. Filters out loopback and IPv6; prefers
/// RFC1918 (192.168/16, 10/8, 172.16-31). Returns None if no candidate is
/// available (rare — usually means no network at all).
pub fn primary_lan_ipv4() -> Option<String> {
    let ifs = if_addrs::get_if_addrs().ok()?;
    let mut candidates: Vec<IpAddr> = ifs
        .into_iter()
        .filter(|i| !i.is_loopback())
        .map(|i| i.ip())
        .filter(|ip| matches!(ip, IpAddr::V4(_)))
        .collect();
    // Prefer private (RFC1918) addresses; link-local last.
    candidates.sort_by_key(|ip| {
        let v4 = match ip {
            IpAddr::V4(v) => *v,
            _ => return 99,
        };
        let oct = v4.octets();
        match oct {
            [192, 168, _, _] => 0,
            [10, _, _, _] => 1,
            [172, b, _, _] if (16..=31).contains(&b) => 2,
            [169, 254, _, _] => 90, // link-local last resort
            _ => 50,
        }
    });
    candidates.first().map(|ip| ip.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primary_lan_ipv4_returns_something_or_none_without_panicking() {
        // Can't assert specific value — depends on host network. Just smoke
        // for no-panic. Result may be Some(ip) or None on a fully isolated host.
        let _ = primary_lan_ipv4();
    }
}
