use crate::{
    cast::CastPlaybackManager,
    config::ZoneConfig,
    loxone_config::{LoxoneRuntimeState, ZoneDescriptor},
    player_state::{player_state_for_protocol, Mode, PlayerState, Playerid},
    zone_equalizer::{bands_to_settings_string, get_bands},
    zone_registry::{PlaybackSource, ZoneRegistry},
    zone_volume,
};
use tracing::debug;

use super::{apply_cover_proxy, cast};
use crate::radio;

pub(crate) fn seed_registry_from_status(
    registry: &mut ZoneRegistry,
    _player_id: Playerid,
    state: &PlayerState,
) {
    let source = if radio::is_radio_audiopath(&state.audiopath) {
        PlaybackSource::CastRadio {
            station_id: state.audiopath.clone(),
        }
    } else if matches!(state.mode, Mode::Play | Mode::Pause) && !state.title.is_empty() {
        PlaybackSource::External
    } else {
        PlaybackSource::Idle
    };
    let _ = registry.commit_state(state.clone(), source);
}

pub(crate) async fn all_player_states(
    registry: &mut ZoneRegistry,
    zones: &[ZoneDescriptor],
    runtime: &LoxoneRuntimeState,
    audioserver_serial: &str,
    cover_proxy_base: &str,
    cast_manager: &CastPlaybackManager,
) -> Vec<PlayerState> {
    let mut player_ids: Vec<usize> = if !zones.is_empty() {
        zones.iter().map(|z| z.player_id).collect()
    } else if let Ok(qcfg) = ZoneConfig::load() {
        qcfg.players.iter().map(|p| p.player_id).collect()
    } else {
        vec![1]
    };
    if player_ids.is_empty() {
        player_ids.push(1);
    }
    let mut states = Vec::new();
    for id in player_ids {
        let playerid = Playerid(id);
        let mut state = registry.snapshot(playerid);
        if state.title.is_empty()
            && matches!(state.mode, Mode::Stop)
            && !registry.is_cast_owned(playerid)
        {
            state = refresh_zone_from_backends(
                playerid,
                zones,
                runtime,
                audioserver_serial,
                cover_proxy_base,
                cast_manager,
            )
            .await;
            seed_registry_from_status(registry, playerid, &state);
        } else {
            apply_cover_proxy(&mut state, Some(cover_proxy_base));
        }
        states.push(state);
    }
    states
}

/// Cold-start seed: poll the Cast manager when the registry slot is empty.
pub(crate) async fn refresh_zone_from_backends(
    playerid: Playerid,
    zones: &[ZoneDescriptor],
    runtime: &LoxoneRuntimeState,
    audioserver_serial: &str,
    cover_proxy_base: &str,
    cast_manager: &CastPlaybackManager,
) -> PlayerState {
    if let Some(state) = cast::cast_radio_player_state_for_status(
        playerid,
        cast_manager,
        zones,
        runtime,
        audioserver_serial,
        cover_proxy_base,
    ) {
        return state;
    }

    let zone_name = zones
        .iter()
        .find(|z| z.player_id == playerid.0)
        .map(|z| z.name.as_str());
    let mut idle = player_state_for_protocol(playerid, zone_name, Some(audioserver_serial));
    idle.equalizer_settings = bands_to_settings_string(&get_bands(playerid));
    idle.volume = zone_volume::display_volume(playerid);

    let config = ZoneConfig::load().unwrap_or_default();
    let mapping = config.players.iter().find(|p| p.player_id == playerid.0);
    if mapping.is_none_or(|p| !p.is_playback_configured()) {
        debug!(
            player_id = playerid.0,
            "zone offline — no playback backend mapped"
        );
    }
    apply_cover_proxy(&mut idle, Some(cover_proxy_base));
    idle
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player_state::{Mode, Playerid};
    use crate::zone_registry::ZoneRegistry;

    #[test]
    fn seed_registry_from_status_sets_external_source_for_playing() {
        let mut registry = ZoneRegistry::new();
        let state = PlayerState {
            playerid: Playerid(1),
            mode: Mode::Play,
            title: "Song".into(),
            audiopath: "airplay".into(),
            ..Default::default()
        };
        seed_registry_from_status(&mut registry, Playerid(1), &state);
        assert_eq!(registry.snapshot(Playerid(1)).title, "Song");
    }

    #[test]
    fn seed_registry_from_status_sets_cast_for_radio_audiopath() {
        let mut registry = ZoneRegistry::new();
        let state = PlayerState {
            playerid: Playerid(3),
            mode: Mode::Play,
            title: "FM".into(),
            audiopath: "tunein:station:9".into(),
            ..Default::default()
        };
        seed_registry_from_status(&mut registry, Playerid(3), &state);
        assert!(registry.is_cast_owned(Playerid(3)));
    }
}
