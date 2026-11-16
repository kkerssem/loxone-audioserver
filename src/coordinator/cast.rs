use crate::{
    cast::{CastPlaybackManager, CastRemoteUpdate},
    loxone_config::{LoxoneRuntimeState, ZoneDescriptor},
    player_state::{enrich_for_loxone_ui, PlayerState, Playerid},
    zone_enrich::enrich_zone_state,
    zone_equalizer::{bands_to_settings_string, get_bands},
    zone_registry::{ApplyOutcome, PlaybackSource, ZoneRegistry},
    zone_volume,
};
use tokio::sync::broadcast;
use tracing::{debug, warn};

use super::{apply_cover_proxy, publish_zone_state};
use crate::remote::IoEvent;

pub(crate) fn zone_name_for(zones: &[ZoneDescriptor], player_id: usize) -> Option<&str> {
    zones
        .iter()
        .find(|z| z.player_id == player_id)
        .map(|z| z.name.as_str())
}

pub(crate) fn fanout_cast_radio_state(
    registry: &mut ZoneRegistry,
    state: PlayerState,
    zone_name: Option<&str>,
    audioserver_serial: &str,
    cover_proxy_base: &str,
    events: &broadcast::Sender<IoEvent>,
) {
    let playerid = state.playerid;
    let station_id = state.audiopath.clone();
    let state = enrich_zone_state(
        state,
        playerid,
        zone_name,
        None,
        audioserver_serial,
        cover_proxy_base,
    );
    publish_zone_state(
        registry,
        state,
        PlaybackSource::CastRadio { station_id },
        events,
    );
}

pub(crate) fn fanout_cast_radio_from_manager(
    playerid: Playerid,
    cast_manager: &CastPlaybackManager,
    registry: &mut ZoneRegistry,
    zones: &[ZoneDescriptor],
    audioserver_serial: &str,
    cover_proxy_base: &str,
    events: &broadcast::Sender<IoEvent>,
) {
    let zone_name = zone_name_for(zones, playerid.0);
    let Some(state) = cast_manager.player_state(playerid, zone_name) else {
        return;
    };
    fanout_cast_radio_state(
        registry,
        state,
        zone_name,
        audioserver_serial,
        cover_proxy_base,
        events,
    );
}

/// Drop a dead Cast session and publish idle when transport to the speaker fails.
#[allow(clippy::too_many_arguments)]
pub(crate) fn reset_cast_zone_on_failure(
    playerid: Playerid,
    op: &str,
    error: &anyhow::Error,
    cast_manager: &mut CastPlaybackManager,
    registry: &mut ZoneRegistry,
    zones: &[ZoneDescriptor],
    audioserver_serial: &str,
    cover_proxy_base: &str,
    events: &broadcast::Sender<IoEvent>,
) {
    warn!(
        player_id = playerid.0,
        op,
        error = %error,
        "cast transport failed; resetting radio zone"
    );
    if cast_manager.zone_is_radio(playerid.0) {
        let _ = cast_manager.take_zone_on_session_end(playerid.0);
    }
    release_cast_zone(
        playerid,
        registry,
        zones,
        audioserver_serial,
        cover_proxy_base,
        events,
    );
}

/// Hand a zone whose Cast radio session is gone back to idle and publish it, so the App stops
/// showing the station and speaker statuses are mirrored again.
pub(crate) fn release_cast_zone(
    playerid: Playerid,
    registry: &mut ZoneRegistry,
    zones: &[ZoneDescriptor],
    audioserver_serial: &str,
    cover_proxy_base: &str,
    events: &broadcast::Sender<IoEvent>,
) {
    if registry.clear_zone(playerid, true) != ApplyOutcome::Changed {
        return;
    }
    let zone_name = zone_name_for(zones, playerid.0);
    let mut state = registry.snapshot(playerid);
    enrich_for_loxone_ui(&mut state, zone_name, Some(audioserver_serial));
    state.equalizer_settings = bands_to_settings_string(&get_bands(playerid));
    apply_cover_proxy(&mut state, Some(cover_proxy_base));
    publish_zone_state(registry, state, PlaybackSource::Idle, events);
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle_cast_remote_update(
    update: CastRemoteUpdate,
    cast_manager: &mut CastPlaybackManager,
    registry: &mut ZoneRegistry,
    zones: &[ZoneDescriptor],
    runtime: &LoxoneRuntimeState,
    audioserver_serial: &str,
    cover_proxy_base: &str,
    events: &broadcast::Sender<IoEvent>,
) {
    match update {
        CastRemoteUpdate::Volume {
            player_id,
            level,
            muted,
        } => {
            if cast_manager.is_stale_volume_echo(player_id, level, muted) {
                debug!(player_id, level, muted, "ignored stale cast volume echo");
                return;
            }
            if !cast_manager.apply_remote_volume(player_id, level, muted) {
                return;
            }
            cast_manager.confirm_volume_echo(player_id, level, muted);
            if let Err(e) = zone_volume::set(Playerid(player_id), level, muted) {
                warn!(player_id, error = %e, "failed to persist cast remote volume");
            }
            fanout_cast_radio_from_manager(
                Playerid(player_id),
                cast_manager,
                registry,
                zones,
                audioserver_serial,
                cover_proxy_base,
                events,
            );
        }
        CastRemoteUpdate::Transport { player_id, mode } => {
            if !cast_manager.apply_remote_mode(player_id, mode) {
                return;
            }
            fanout_cast_radio_from_manager(
                Playerid(player_id),
                cast_manager,
                registry,
                zones,
                audioserver_serial,
                cover_proxy_base,
                events,
            );
        }
        CastRemoteUpdate::SessionEnded {
            player_id,
            session,
            force,
        } => {
            match session_end_action(cast_manager.radio_session(player_id), session) {
                SessionEndAction::Stale => {
                    debug!(player_id, session, "ignored end of a replaced cast session");
                    return;
                }
                // The zone's live session ended: no newer session can be starting, so clear the
                // zone even right after its last Cast update (the registry grace would otherwise
                // drop this one report, and the listener sends no other).
                SessionEndAction::EndCurrent => {
                    let _ = cast_manager.take_zone_on_session_end(player_id);
                    if registry.clear_zone(Playerid(player_id), true) != ApplyOutcome::Changed {
                        return;
                    }
                }
                SessionEndAction::NoSession => {
                    // No radio session in the manager (a start that failed after connecting):
                    // a definite loss still clears a zone left marked as Cast radio.
                    if !force
                        || registry.clear_zone(Playerid(player_id), true) != ApplyOutcome::Changed
                    {
                        return;
                    }
                }
            }
            let zone_name = zone_name_for(zones, player_id);
            let mut state = registry.snapshot(Playerid(player_id));
            enrich_for_loxone_ui(&mut state, zone_name, Some(audioserver_serial));
            state.equalizer_settings = bands_to_settings_string(&get_bands(Playerid(player_id)));
            apply_cover_proxy(&mut state, Some(cover_proxy_base));
            publish_zone_state(registry, state, PlaybackSource::Idle, events);
            let _ = runtime;
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum SessionEndAction {
    /// The report comes from a session that has since been replaced (a new station started).
    Stale,
    /// The zone's live radio session ended.
    EndCurrent,
    /// The manager has no radio session for the zone.
    NoSession,
}

/// Decide what a `SessionEnded` report means, given the zone's current radio session.
fn session_end_action(current: Option<u64>, ended: u64) -> SessionEndAction {
    match current {
        Some(current) if current == ended => SessionEndAction::EndCurrent,
        Some(_) => SessionEndAction::Stale,
        None => SessionEndAction::NoSession,
    }
}

pub(crate) fn cast_radio_player_state_for_status(
    playerid: Playerid,
    cast_manager: &CastPlaybackManager,
    zones: &[ZoneDescriptor],
    runtime: &LoxoneRuntimeState,
    audioserver_serial: &str,
    cover_proxy_base: &str,
) -> Option<PlayerState> {
    if !cast_manager.zone_is_radio(playerid.0) {
        return None;
    }
    let zone_name = zone_name_for(zones, playerid.0);
    let state = cast_manager.player_state(playerid, zone_name)?;
    Some(enrich_zone_state(
        state,
        playerid,
        zone_name,
        Some(runtime),
        audioserver_serial,
        cover_proxy_base,
    ))
}

#[cfg(test)]
mod radio_status_tests {
    use super::*;
    use crate::cast::CastPlaybackManager;
    use crate::player_state::{Mode, Playerid};
    use crate::radio::{build_radio_player_state, ResolvedRadioStream};
    use std::time::Duration;
    use tokio::sync::mpsc;

    fn sample_resolved() -> ResolvedRadioStream {
        ResolvedRadioStream {
            audiopath: "tunein:station:s6707".into(),
            stream_url: "http://example/stream".into(),
            content_type: "audio/mpeg".into(),
            title: "NPO 3FM".into(),
            cover_url: Some("https://cdn.example/cover.jpg".into()),
            is_custom: false,
        }
    }

    #[test]
    fn zone_name_for_returns_matching_zone() {
        let zones = vec![ZoneDescriptor {
            player_id: 3,
            uuid: "u".into(),
            name: "Kitchen".into(),
        }];
        assert_eq!(zone_name_for(&zones, 3), Some("Kitchen"));
        assert_eq!(zone_name_for(&zones, 9), None);
    }

    #[test]
    fn fanout_cast_radio_state_publishes_to_registry() {
        use crate::player_state::{Mode, Playerid};
        use crate::radio::{build_radio_player_state, ResolvedRadioStream};
        use tokio::sync::broadcast;

        let resolved = ResolvedRadioStream {
            audiopath: "tunein:station:1".into(),
            stream_url: "http://example/stream".into(),
            content_type: "audio/mpeg".into(),
            title: "FM".into(),
            cover_url: None,
            is_custom: false,
        };
        let base =
            build_radio_player_state(Playerid(3), Some("Kitchen"), &resolved, Mode::Play, 40.0);
        let (tx, mut rx) = broadcast::channel(4);
        let mut registry = ZoneRegistry::new();
        fanout_cast_radio_state(
            &mut registry,
            base,
            Some("Kitchen"),
            "504f94ff1bb3",
            "http://10.0.0.1:7091",
            &tx,
        );
        assert!(registry.is_cast_owned(Playerid(3)));
        assert_eq!(
            recv_state_helper(&mut rx).map(|s| s.title),
            Some("FM".to_string())
        );
    }

    #[test]
    fn session_end_from_the_live_session_ends_it() {
        assert_eq!(session_end_action(Some(4), 4), SessionEndAction::EndCurrent);
    }

    #[test]
    fn session_end_from_a_replaced_session_is_stale() {
        assert_eq!(session_end_action(Some(5), 4), SessionEndAction::Stale);
    }

    #[test]
    fn session_end_without_a_radio_session() {
        assert_eq!(session_end_action(None, 4), SessionEndAction::NoSession);
    }

    /// The bug this guards against: a live session's end report arrives within the registry's
    /// grace after the last Cast update, is ignored, and the zone stays Cast-owned, so the
    /// speaker's own playback is never shown again.
    #[test]
    fn live_session_end_inside_grace_releases_zone_to_speaker_polls() {
        let resolved = sample_resolved();
        let base =
            build_radio_player_state(Playerid(3), Some("Kitchen"), &resolved, Mode::Play, 40.0);
        let (tx, _rx) = broadcast::channel(8);
        let mut registry = ZoneRegistry::with_cast_session_end_grace(Duration::from_secs(60));
        fanout_cast_radio_state(
            &mut registry,
            base,
            Some("Kitchen"),
            "504f94ff1bb3",
            "http://10.0.0.1:7091",
            &tx,
        );
        assert!(registry.is_cast_owned(Playerid(3)));
        // What the old handling did for a non-forced end: ignored inside the grace.
        assert_eq!(
            registry.clear_zone(Playerid(3), false),
            ApplyOutcome::Ignored
        );
        assert!(registry.is_cast_owned(Playerid(3)));
        // EndCurrent clears with force, so the zone is released.
        assert_eq!(
            registry.clear_zone(Playerid(3), true),
            ApplyOutcome::Changed
        );
        assert!(!registry.is_cast_owned(Playerid(3)));
    }

    fn recv_state_helper(rx: &mut broadcast::Receiver<IoEvent>) -> Option<PlayerState> {
        loop {
            match rx.try_recv() {
                Ok(IoEvent::State(s)) => return Some(s),
                Ok(_) => continue,
                Err(broadcast::error::TryRecvError::Empty) => return None,
                Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(broadcast::error::TryRecvError::Closed) => return None,
            }
        }
    }

    #[test]
    fn enrich_zone_state_preserves_live_cast_volume() {
        let resolved = sample_resolved();
        let base =
            build_radio_player_state(Playerid(3), Some("Kitchen"), &resolved, Mode::Play, 34.0);
        let mut runtime = LoxoneRuntimeState::default();
        runtime.volumes.push(crate::loxone_config::Volume {
            playerid: 3,
            default: 80,
            alarm: 80,
            fire: 80,
            bell: 80,
            buzzer: 80,
            tts: 80,
            volstep: 4,
            fading: 0,
            max_volume: 100,
        });
        let state = enrich_zone_state(
            base,
            Playerid(3),
            Some("Kitchen"),
            Some(&runtime),
            "504f94ff1bb3",
            "http://192.168.1.50:7091",
        );
        assert_eq!(state.volume, 34.0);
    }

    #[test]
    fn enrich_zone_state_applies_loxone_envelope_and_cover_proxy() {
        let resolved = sample_resolved();
        let base =
            build_radio_player_state(Playerid(3), Some("Kitchen"), &resolved, Mode::Play, 50.0);
        let state = enrich_zone_state(
            base,
            Playerid(3),
            Some("Kitchen"),
            None,
            "504f94ff1bb3",
            "http://192.168.1.50:7091",
        );
        assert_eq!(state.mode, Mode::Play);
        assert_eq!(state.audiopath, "tunein:station:s6707");
        assert_eq!(state.title, "NPO 3FM");
        assert_eq!(state.zone_type, 3);
        assert_eq!(state.client_state, "on");
        assert!(state
            .cover_url
            .starts_with("http://192.168.1.50:7091/cors-proxy?url="));
    }

    #[test]
    fn cast_radio_player_state_for_status_none_when_zone_not_radio() {
        let (tx, _rx) = mpsc::channel(4);
        let manager = CastPlaybackManager::new(tx);
        let runtime = LoxoneRuntimeState::default();
        let zones = vec![ZoneDescriptor {
            player_id: 3,
            uuid: "zone-uuid".into(),
            name: "Kitchen".into(),
        }];
        assert!(cast_radio_player_state_for_status(
            Playerid(3),
            &manager,
            &zones,
            &runtime,
            "504f94ff1bb3",
            "http://10.0.0.1:7091",
        )
        .is_none());
    }
}
