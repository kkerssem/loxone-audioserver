//! mDNS advertisement for Loxone Config (`_loxaudio._tcp` on port 7091).
//!
//! Matches [lox-audioserver](https://github.com/lox-audioserver/lox-audioserver)
//! `LoxAudioMdnsService`.

use crate::app_config::AppConfig;
use crate::discovery::udp::{normalize_serial, DISCOVERY_ADVERTISE_PORT};
use crate::net_util::local_ipv4;
use mdns_sd::{ServiceDaemon, ServiceInfo};
use std::net::{IpAddr, Ipv4Addr};
use tracing::{debug, warn};

pub(crate) const MDNS_SERVICE_TYPE: &str = SERVICE_TYPE;

const SERVICE_TYPE: &str = "_loxaudio._tcp.local.";

pub fn start_mdns_advertisement(app_config: AppConfig) {
    tokio::spawn(async move {
        if let Err(e) = run(&app_config).await {
            warn!(error = %e, "mdns advertisement stopped");
        }
    });
}

async fn run(app_config: &AppConfig) -> anyhow::Result<()> {
    let ip = local_ipv4().unwrap_or(Ipv4Addr::LOCALHOST);
    let mac = normalize_serial(&app_config.serial);
    let instance = sanitize_instance_name(&app_config.display_name);
    let host = format!("{}.local.", app_config.hostname);

    let properties = [
        ("api", "/api"),
        ("linein", "/api/linein"),
        ("linein_register", "/api/linein/bridges/register"),
        ("linein_status", "/api/linein/bridges/{bridge_id}/status"),
        ("mac", mac.as_str()),
    ];

    let service = ServiceInfo::new(
        SERVICE_TYPE,
        &instance,
        &host,
        IpAddr::V4(ip),
        DISCOVERY_ADVERTISE_PORT,
        &properties[..],
    )?;

    let mdns = ServiceDaemon::new()?;
    mdns.register(service)?;

    debug!(
        %ip,
        port = DISCOVERY_ADVERTISE_PORT,
        instance,
        mac,
        service_type = SERVICE_TYPE,
        "mdns service registered"
    );

    std::future::pending::<()>().await;
    Ok(())
}

pub(crate) fn mdns_instance_name(value: &str) -> String {
    sanitize_instance_name(value)
}

fn sanitize_instance_name(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        "audioserver".to_string()
    } else {
        trimmed.chars().take(63).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_type_matches_loxone() {
        assert_eq!(SERVICE_TYPE, "_loxaudio._tcp.local.");
    }
}
