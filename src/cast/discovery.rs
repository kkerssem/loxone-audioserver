//! Browse `_googlecast._tcp` on the LAN.

use anyhow::Result;
use mdns_sd::ResolvedService;
use std::time::Duration;

use crate::mdns_browse::{browse_mdns_resolved, resolved_primary_host, resolved_property_str};

use super::device::CastDeviceInfo;

pub const GOOGLECAST_SERVICE: &str = "_googlecast._tcp.local.";

pub async fn browse_cast_devices(timeout: Duration) -> Result<Vec<CastDeviceInfo>> {
    let resolved = browse_mdns_resolved(GOOGLECAST_SERVICE, timeout).await?;
    let mut devices = Vec::new();
    for info in resolved {
        if let Some(dev) = map_service_info(&info) {
            devices.push(dev);
        }
    }
    devices.sort_by(|a, b| a.friendly_name.cmp(&b.friendly_name));
    Ok(devices)
}

fn map_service_info(info: &ResolvedService) -> Option<CastDeviceInfo> {
    let host = resolved_primary_host(info);
    let friendly_name = resolved_property_str(info, "fn").unwrap_or_else(|| {
        info.get_fullname()
            .split('.')
            .next()
            .unwrap_or("Cast")
            .to_string()
    });
    let device_id = resolved_property_str(info, "id").unwrap_or_default();
    let model = resolved_property_str(info, "md");
    Some(CastDeviceInfo {
        host,
        port: info.get_port(),
        friendly_name,
        device_id,
        model,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_constant() {
        assert!(GOOGLECAST_SERVICE.contains("googlecast"));
    }
}
