//! Loxone Audioserver discovery (UDP probe responder + mDNS).
//!
//! One INFO when advertising starts; per-protocol detail at debug; task failures via warn.

mod mdns;
mod udp;

use std::net::Ipv4Addr;

use tracing::{debug, info};

use crate::net_util::local_ipv4;

pub use mdns::start_mdns_advertisement;
pub use udp::{normalize_serial, start_udp_discovery, DISCOVERY_LISTEN_PORT};

use mdns::{mdns_instance_name, MDNS_SERVICE_TYPE};
use udp::{discovery_payload, DISCOVERY_ADVERTISE_PORT};

/// Start UDP discovery and mDNS advertisement for Loxone Config.
pub fn start_discovery(app_config: crate::app_config::AppConfig) {
    let ip = local_ipv4().unwrap_or(Ipv4Addr::LOCALHOST);
    let serial = normalize_serial(&app_config.serial);
    let name = mdns_instance_name(&app_config.display_name);

    info!(
        udp_listen_port = DISCOVERY_LISTEN_PORT,
        http_port = DISCOVERY_ADVERTISE_PORT,
        %ip,
        serial = %serial,
        hostname = %app_config.hostname,
        name = %name,
        mdns_service = MDNS_SERVICE_TYPE,
        "loxone discovery advertising"
    );
    debug!(
        udp_payload = %discovery_payload(&app_config),
        "discovery udp response payload"
    );

    start_udp_discovery(app_config.clone());
    start_mdns_advertisement(app_config);
}
