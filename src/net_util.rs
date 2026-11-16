//! LAN address helpers for discovery and cover-proxy URLs.

use std::net::{IpAddr, Ipv4Addr, UdpSocket};
use tracing::warn;

/// TCP port for Loxone Web UI / HTTP (cover proxy, WebSocket).
pub const REMOTE_HTTP_PORT: u16 = 7091;

/// Best-effort LAN IPv4 (UDP connect trick). Returns `None` if detection fails.
pub fn local_ipv4() -> Option<Ipv4Addr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    match socket.local_addr().ok()?.ip() {
        IpAddr::V4(ip) if !ip.is_loopback() => Some(ip),
        _ => None,
    }
}

/// Host clients should use to reach this audioserver (`--public-host` or auto-detect).
pub fn resolve_public_host(override_host: Option<String>) -> String {
    if let Some(host) = override_host {
        let trimmed = host.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    if let Some(ip) = local_ipv4() {
        return ip.to_string();
    }
    warn!("lan IPv4 not detected; cover proxy using 127.0.0.1");
    "127.0.0.1".to_string()
}

/// Base URL for [`crate::zone_enrich::apply_cover_proxy`] (`http://host:7091`).
pub fn cover_proxy_base_url(host: &str) -> String {
    format!("http://{host}:{REMOTE_HTTP_PORT}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cover_proxy_base_url_includes_port() {
        assert_eq!(
            cover_proxy_base_url("192.168.1.10"),
            "http://192.168.1.10:7091"
        );
    }

    #[test]
    fn resolve_public_host_uses_override() {
        assert_eq!(
            resolve_public_host(Some("audioserver.local".into())),
            "audioserver.local"
        );
    }
}
