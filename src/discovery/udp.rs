//! UDP discovery for Loxone Config (listen 7070, respond from 7091).
//!
//! JSON field `s` is the **bare 12-hex serial** (`504F94FF1BB3`). Colon MAC
//! (`50:4F:94:FF:1B:B3`) freezes Loxone Config; bare serial matches `setconfig` keys.
//! Replies are sent from port 7091.

use crate::app_config::AppConfig;
use serde_json::json;
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::net::UdpSocket;
use tracing::{debug, warn};

pub const DISCOVERY_LISTEN_PORT: u16 = 7070;
/// TCP port advertised in discovery (`p`). Loxone Config contacts this after UDP find.
pub const DISCOVERY_ADVERTISE_PORT: u16 = 7091;
const DEDUPE_WINDOW: Duration = Duration::from_millis(400);

pub fn start_udp_discovery(app_config: AppConfig) {
    tokio::spawn(async move {
        if let Err(e) = run(app_config).await {
            warn!(error = %e, "udp discovery stopped");
        }
    });
}

async fn run(app_config: AppConfig) -> anyhow::Result<()> {
    let reply = Arc::new(UdpSocket::bind(format!("0.0.0.0:{DISCOVERY_ADVERTISE_PORT}")).await?);
    let response = Arc::new(discovery_payload(&app_config));
    listen_port(DISCOVERY_LISTEN_PORT, reply, response).await
}

async fn listen_port(
    port: u16,
    reply: Arc<UdpSocket>,
    response: Arc<String>,
) -> anyhow::Result<()> {
    let listen = UdpSocket::bind(format!("0.0.0.0:{port}")).await?;
    debug!(
        listen_port = port,
        reply_port = DISCOVERY_ADVERTISE_PORT,
        "udp discovery listening"
    );

    let mut last_sent: HashMap<std::net::SocketAddr, Instant> = HashMap::new();
    let mut buf = [0u8; 2048];
    loop {
        let (len, peer) = listen.recv_from(&mut buf).await?;
        debug!(
            ?peer,
            len,
            port,
            probe = %format_probe(&buf[..len]),
            "udp discovery probe"
        );

        let now = Instant::now();
        if let Some(last) = last_sent.get(&peer) {
            if now.duration_since(*last) < DEDUPE_WINDOW {
                debug!(?peer, "udp discovery duplicate probe (suppressed)");
                continue;
            }
        }
        last_sent.insert(peer, now);

        reply.send_to(response.as_bytes(), peer).await?;
        debug!(
            ?peer,
            from_port = DISCOVERY_ADVERTISE_PORT,
            "udp discovery response sent"
        );
    }
}

fn format_probe(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return "(empty)".into();
    }
    if bytes.len() <= 32
        && bytes
            .iter()
            .all(|b| b.is_ascii_graphic() || b.is_ascii_whitespace())
    {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Bare 12-hex serial for field `s` (Loxone Config / setconfig format).
pub fn normalize_serial(serial: &str) -> String {
    serial
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .collect::<String>()
        .to_uppercase()
}

/// Colon-separated MAC (reference impl only — do not use in UDP field `s`).
#[cfg(test)]
fn format_mac_colon(serial: &str) -> String {
    let normalized = normalize_serial(serial);
    if normalized.len() == 12 {
        normalized
            .as_bytes()
            .chunks(2)
            .map(|chunk| std::str::from_utf8(chunk).unwrap_or("00"))
            .collect::<Vec<_>>()
            .join(":")
    } else {
        serial.to_string()
    }
}

fn sanitize_friendly_name(value: &str) -> String {
    let trimmed = value.trim();
    let ascii: String = trimmed
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == ' ' || *c == '-' || *c == '_')
        .collect();
    let collapsed = ascii.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        "audioserver".to_string()
    } else {
        collapsed.chars().take(48).collect()
    }
}

fn sanitize_hostname(value: &str) -> String {
    let trimmed = value.trim();
    let without_local = trimmed.strip_suffix(".local").unwrap_or(trimmed);
    let sanitized = without_local.replace([' ', '.'], "-").to_ascii_lowercase();
    if sanitized.is_empty() {
        "audioserver".to_string()
    } else {
        sanitized
    }
}

pub(crate) fn discovery_payload(app_config: &AppConfig) -> String {
    serde_json::to_string(&json!({
        "t": "Audioserver",
        "tv": "3",
        "n": sanitize_friendly_name(&app_config.display_name),
        "s": normalize_serial(&app_config.serial),
        "v": app_config.version,
        "p": DISCOVERY_ADVERTISE_PORT.to_string(),
        "hn": sanitize_hostname(&app_config.hostname),
        "di": "1",
    }))
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_serial_without_colons() {
        assert_eq!(normalize_serial("504F94FF1BB3"), "504F94FF1BB3");
        assert_eq!(normalize_serial("50:4F:94:FF:1B:B3"), "504F94FF1BB3");
    }

    #[test]
    fn colon_mac_is_separate_from_serial() {
        assert_eq!(format_mac_colon("504F94FF1BB3"), "50:4F:94:FF:1B:B3");
    }

    #[test]
    fn sanitizes_display_name() {
        assert_eq!(
            sanitize_friendly_name("Loxone Audioserver"),
            "Loxone Audioserver"
        );
        assert_eq!(sanitize_friendly_name(""), "audioserver");
    }

    #[test]
    fn payload_uses_bare_serial() {
        let cfg = AppConfig::new(
            "504F94FF1BB3".into(),
            Some("audioserver".into()),
            Some("audioserver".into()),
            None,
        )
        .expect("test AppConfig");
        let payload = discovery_payload(&cfg);
        assert!(payload.contains(r#""s":"504F94FF1BB3""#));
        assert!(!payload.contains("50:4F:94"));
    }

    #[test]
    fn normalizes_serial_lowercase_and_strips_separators() {
        assert_eq!(normalize_serial("50-4f-94-ff-1b-b3"), "504F94FF1BB3");
        assert_eq!(normalize_serial("504f94ff1bb3"), "504F94FF1BB3");
    }

    #[test]
    fn format_mac_colon_leaves_non_12_hex_unchanged() {
        assert_eq!(format_mac_colon("504F94"), "504F94");
    }

    #[test]
    fn sanitize_hostname_strips_local_suffix() {
        assert_eq!(sanitize_hostname("Audioserver.local"), "audioserver");
        assert_eq!(sanitize_hostname("My Room.local"), "my-room");
    }

    #[test]
    fn payload_includes_hostname_and_advertise_port() {
        let cfg = AppConfig::new(
            "504F94FF1BB3".into(),
            Some("Loxone Audioserver".into()),
            Some("audioserver.local".into()),
            None,
        )
        .expect("test AppConfig");
        let payload = discovery_payload(&cfg);
        assert!(payload.contains(r#""p":"7091""#));
        assert!(payload.contains(r#""hn":"audioserver""#));
        assert!(payload.contains(r#""n":"Loxone Audioserver""#));
        assert!(payload.contains(r#""t":"Audioserver""#));
    }

    #[test]
    fn format_probe_binary_and_empty() {
        assert_eq!(format_probe(&[]), "(empty)");
        assert_eq!(format_probe(b"LOXONE"), "LOXONE");
        assert_eq!(format_probe(&[0x00, 0xff, 0xab]), "00ffab");
    }
}
