use crate::{
    config::AudioserverConfigFile,
    config::{self as persisted, ZoneConfig},
    loxone_config::{apply_player_names, audioserver_config, extract_channel_ids, extract_zones},
    player_state::set_loxone_runtime_volumes,
    zone_volume,
};
use tracing::warn;

use super::HwChannelIds;
use crate::loxone_config::{LoxoneRuntimeState, SerialWithConfig, ZoneDescriptor};

pub(crate) async fn update_hw_channels(hw_channels: &HwChannelIds, cfg: &SerialWithConfig) {
    let mut ids = Vec::new();
    if let Some(map) = cfg.first() {
        for config in map.values() {
            ids.extend(extract_channel_ids(config));
        }
    }
    ids.sort();
    ids.dedup();
    *hw_channels.write().await = ids;
}

pub(crate) async fn sync_player_rows(zones: &[ZoneDescriptor]) -> anyhow::Result<()> {
    if zones.is_empty() {
        return Ok(());
    }
    let snapshots: Vec<persisted::LoxoneZoneSnapshot> = zones
        .iter()
        .map(persisted::LoxoneZoneSnapshot::from)
        .collect();
    ZoneConfig::update(|config| {
        persisted::prune_players_not_in_loxone_zones(config, zones);
        persisted::apply_loxone_snapshots(config, &snapshots);
        Ok(())
    })?;

    let config = ZoneConfig::load()?;
    let issues = persisted::validate_mappings(&config, &snapshots);
    persisted::log_mapping_issues(&issues);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle_loxone_set_config(
    raw: Vec<u8>,
    parsed: SerialWithConfig,
    serial_with_config: &mut Option<SerialWithConfig>,
    runtime: &mut LoxoneRuntimeState,
    zones: &mut Vec<ZoneDescriptor>,
    hw_channels: &HwChannelIds,
    mac_id: &str,
) -> anyhow::Result<()> {
    if raw == b"[]" {
        *serial_with_config = None;
        *runtime = LoxoneRuntimeState::default();
        zones.clear();
        hw_channels.write().await.clear();
    } else {
        *serial_with_config = Some(parsed);
        if let Some(ref cfg) = serial_with_config {
            if let Some(ac) = audioserver_config(cfg, mac_id) {
                runtime.config = Some(ac.clone());
                *zones = extract_zones(cfg, mac_id);
                apply_player_names(zones, &runtime.player_names);
                update_hw_channels(hw_channels, cfg).await;
                sync_player_rows(zones).await?;
            }
        }
    }
    Ok(())
}

pub(crate) async fn handle_loxone_volumes(
    vols: &[crate::loxone_config::Volume],
    runtime: &mut LoxoneRuntimeState,
) {
    runtime.volumes = vols.to_vec();
    set_loxone_runtime_volumes(
        vols.iter()
            .filter_map(|v| {
                v.fixed_default_percent()
                    .map(|pct| (v.playerid, f32::from(pct)))
            })
            .collect(),
    );
    if let Err(e) = zone_volume::apply_loxone_volume_presets(vols) {
        warn!(error = %e, "failed to persist Loxone volume presets");
    }
}

#[allow(clippy::ptr_arg)]
pub(crate) async fn handle_player_names(
    player_names: &crate::loxone_config::PlayerNames,
    zones: &mut Vec<ZoneDescriptor>,
) -> anyhow::Result<()> {
    apply_player_names(zones, &player_names.players);
    sync_player_rows(zones).await
}

pub(crate) async fn handle_loxone_ready(
    serial_with_config: &Option<SerialWithConfig>,
) -> anyhow::Result<()> {
    if let Some(ref cfg) = serial_with_config {
        AudioserverConfigFile::update(|file| {
            file.set_loxone_state_pretty(cfg);
            Ok(())
        })?;
    }
    Ok(())
}

pub(crate) async fn init_from_serial_config(
    serial_with_config: &Option<SerialWithConfig>,
    runtime: &mut LoxoneRuntimeState,
    zones: &mut Vec<ZoneDescriptor>,
    hw_channels: &HwChannelIds,
    mac_id: &str,
) -> anyhow::Result<()> {
    if let Some(ref cfg) = serial_with_config {
        if let Some(ac) = audioserver_config(cfg, mac_id) {
            runtime.config = Some(ac.clone());
            *zones = extract_zones(cfg, mac_id);
            apply_player_names(zones, &runtime.player_names);
            update_hw_channels(hw_channels, cfg).await;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinator::new_hw_channel_ids;

    #[tokio::test]
    async fn sync_player_rows_empty_zones_is_noop() {
        sync_player_rows(&[]).await.unwrap();
    }

    #[tokio::test]
    async fn update_hw_channels_extracts_channel_ids() {
        let cfg: SerialWithConfig = serde_json::from_str(
            r#"[{"504F94FF1BB3":{"master":"504F94000002","ip":"x","port":80,"language":"ENU","blinkpos":0,"autoUpdate":120,"unitTemperature":"C","name":"AS","errortts":false,"uuid":"srv","pwHash":"","players":[{"uuid":"u","playerid":1,"speakerConfig":4,"bt":"on","outputs":[{"channels":[{"id":"504F94FF1BB3#1","type":"stereo","maxvol":100,"offsetVol":0,"volmode":"normal"}]}]}],"extensions":[],"groups":[]}}]"#,
        )
        .unwrap();
        let hw = new_hw_channel_ids();
        update_hw_channels(&hw, &cfg).await;
        let ids = hw.read().await.clone();
        assert_eq!(ids, vec!["504F94FF1BB3#1".to_string()]);
    }
}
