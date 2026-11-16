//! Match a Loxone zone to a Google Cast device by configured `cast_device_id`.

use anyhow::{bail, Result};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use crate::config::ZoneConfig;

use super::device::CastDeviceInfo;
use super::discovery::browse_cast_devices;
use super::id::cast_device_ids_match;
use tracing::{debug, info};

/// Cast devices found by the last browse, so a zone's device resolves without a new mDNS
/// browse (~3 s). Cleared by [`forget_known_cast_devices`] when a known address stops working.
static KNOWN_DEVICES: LazyLock<Mutex<Vec<CastDeviceInfo>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));

fn configured_cast_id(player_id: usize) -> Result<String> {
    let config = ZoneConfig::load().unwrap_or_default();
    let configured_id = config
        .players
        .iter()
        .find(|p| p.player_id == player_id)
        .and_then(|p| p.cast_device_id.clone())
        .filter(|id| !id.is_empty());
    match configured_id {
        Some(id) => Ok(id),
        None => bail!("no cast_device_id configured for zone {player_id} — add cast_device_id to audioserver-config.json players[]"),
    }
}

fn find_device(devices: &[CastDeviceInfo], configured_id: &str) -> Option<CastDeviceInfo> {
    devices
        .iter()
        .find(|c| cast_device_ids_match(configured_id, &c.device_id))
        .cloned()
}

/// The zone's Cast device as found by an earlier browse, if any.
pub fn known_cast_device(player_id: usize) -> Option<CastDeviceInfo> {
    let configured_id = configured_cast_id(player_id).ok()?;
    let known = KNOWN_DEVICES.lock().ok()?;
    find_device(&known, &configured_id)
}

/// Drop the remembered devices, so the next lookup browses again (e.g. after an address change).
pub fn forget_known_cast_devices() {
    if let Ok(mut known) = KNOWN_DEVICES.lock() {
        known.clear();
    }
}

/// Browse once and remember the Cast devices found, so the first radio start or voice message
/// after a restart does not wait for a browse (~3 s). Best effort.
pub async fn warm_known_cast_devices(timeout: Duration) {
    match browse_cast_devices(timeout).await {
        Ok(devices) if !devices.is_empty() => {
            debug!(count = devices.len(), "cast devices remembered at startup");
            remember_cast_devices(&devices);
        }
        Ok(_) => debug!("no cast devices found at startup"),
        Err(e) => debug!(error = %e, "cast browse at startup failed"),
    }
}

fn remember_cast_devices(devices: &[CastDeviceInfo]) {
    if let Ok(mut known) = KNOWN_DEVICES.lock() {
        *known = devices.to_vec();
    }
}

/// Browse the LAN and return the zone's Cast device (`cast_device_id`). The devices found are
/// remembered for [`known_cast_device`].
pub async fn resolve_cast_device(
    player_id: usize,
    _zones: &[(usize, String)],
    timeout: Duration,
) -> Result<CastDeviceInfo> {
    let configured_id = configured_cast_id(player_id)?;
    let configured_id = configured_id.as_str();

    let cast_devices = browse_cast_devices(timeout).await?;
    if !cast_devices.is_empty() {
        remember_cast_devices(&cast_devices);
    }
    if let Some(hit) = find_device(&cast_devices, configured_id) {
        info!(
            player_id,
            cast = %hit.friendly_name,
            cast_device_id = configured_id,
            "cast target matched by cast_device_id"
        );
        return Ok(hit);
    }

    if cast_devices.is_empty() {
        bail!(
            "cast_device_id {configured_id} not found on LAN — no Google Cast devices discovered (is the device powered and on the network?)"
        );
    }
    bail!("cast_device_id {configured_id} not found on LAN (is the device powered and on the network?)")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{PlayerMapping, ZoneConfig};

    fn device(id: &str, name: &str) -> CastDeviceInfo {
        CastDeviceInfo {
            host: "192.168.1.1".into(),
            port: 8009,
            friendly_name: name.into(),
            device_id: id.into(),
            model: None,
        }
    }

    fn resolve_against(devices: &[CastDeviceInfo], configured: &str) -> Option<CastDeviceInfo> {
        find_device(devices, configured)
    }

    #[test]
    fn known_devices_resolve_the_zone_until_forgotten() {
        use crate::config::AudioserverConfigFile;
        let audioserver = ZoneConfig {
            players: vec![PlayerMapping {
                player_id: 4,
                name: "Living Room".into(),
                cast_device_id: Some("06e536d3150a09d951bcb8635b0e72dc".into()),
                ..Default::default()
            }],
        };
        AudioserverConfigFile::with_test_cache(
            AudioserverConfigFile {
                audioserver,
                ..Default::default()
            },
            || {
                remember_cast_devices(&[
                    device("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "Other"),
                    device("06e536d3150a09d951bcb8635b0e72dc", "LS60 Wireless"),
                ]);
                assert_eq!(
                    known_cast_device(4).map(|d| d.friendly_name),
                    Some("LS60 Wireless".to_string())
                );
                assert!(
                    known_cast_device(3).is_none(),
                    "zone without cast_device_id"
                );
                forget_known_cast_devices();
                assert!(known_cast_device(4).is_none());
            },
        );
    }

    #[test]
    fn explicit_id_match() {
        let devices = vec![device("c431e9d77cba7b8a3c2fde5fca5eba96", "Beoplay M3")];
        let hit = resolve_against(&devices, "c431e9d77cba7b8a3c2fde5fca5eba96");
        assert_eq!(
            hit.as_ref().map(|d| d.friendly_name.as_str()),
            Some("Beoplay M3")
        );
    }

    #[test]
    fn configured_id_miss_does_not_fallback() {
        let devices = vec![
            device("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "Other"),
            device("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", "Other2"),
        ];
        assert!(resolve_against(&devices, "c431e9d77cba7b8a3c2fde5fca5eba96").is_none());
    }

    #[test]
    fn playback_configured_requires_cast() {
        let offline = PlayerMapping {
            player_id: 1,
            name: "A".into(),
            ..Default::default()
        };
        assert!(!offline.is_playback_configured());

        let cast_only = PlayerMapping {
            cast_device_id: Some("c431e9d77cba7b8a3c2fde5fca5eba96".into()),
            ..offline
        };
        assert!(cast_only.is_playback_configured());
    }

    #[test]
    fn invalid_cast_id_does_not_match_devices() {
        let devices = vec![device("c431e9d77cba7b8a3c2fde5fca5eba96", "Beoplay M3")];
        assert!(resolve_against(&devices, "too-short").is_none());
        assert!(resolve_against(&devices, "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz").is_none());
        assert!(resolve_against(&devices, "not-a-valid-cast-id-at-all!!").is_none());
    }

    #[test]
    fn zone_config_cast_lookup() {
        use crate::config::AudioserverConfigFile;
        let audioserver = ZoneConfig {
            players: vec![PlayerMapping {
                player_id: 3,
                name: "Bedroom".into(),
                cast_device_id: Some("6b87c09a06930d30c0411a68d1434644".into()),
                ..Default::default()
            }],
        };
        let id = AudioserverConfigFile::with_test_cache(
            AudioserverConfigFile {
                audioserver,
                ..Default::default()
            },
            || {
                ZoneConfig::load()
                    .unwrap()
                    .players
                    .iter()
                    .find(|p| p.player_id == 3)
                    .and_then(|p| p.cast_device_id.clone())
            },
        );
        assert_eq!(id.as_deref(), Some("6b87c09a06930d30c0411a68d1434644"));
    }
}
