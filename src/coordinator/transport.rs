//! Zone transport dispatch (play / pause / volume …): Cast radio sessions first, then the
//! zone's speaker backend. Zones without either ignore playback commands, but still keep
//! their Loxone volume (persisted and published) so the slider does not snap back.

use crate::{
    cast::CastPlaybackManager,
    loxone_config::ZoneDescriptor,
    player_state::Playerid,
    radio::is_radio_audiopath,
    remote::IoCommand,
    speaker::SpeakerManager,
    zone_registry::{PlaybackSource, ZoneRegistry, ZoneUpdate},
    zone_transport::VolumeStep,
    zone_volume,
};
use tokio::sync::broadcast;
use tracing::{debug, warn};

use super::{apply_zone_update, cast};
use crate::remote::IoEvent;

#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle_transport(
    cmd: IoCommand,
    zones: &[ZoneDescriptor],
    audioserver_serial: &str,
    events: &broadcast::Sender<IoEvent>,
    cover_proxy_base: &str,
    cast_manager: &mut CastPlaybackManager,
    speaker_manager: &SpeakerManager,
    registry: &mut ZoneRegistry,
) -> anyhow::Result<()> {
    let Some(playerid) = command_player_id(&cmd) else {
        return Ok(());
    };

    if let IoCommand::Play(_, Some(opts)) = &cmd {
        if is_radio_audiopath(&opts.id) {
            cast_manager.stop_zone(playerid).await.ok();
            let _ = registry.apply(ZoneUpdate::SetSource {
                player_id: playerid,
                source: PlaybackSource::CastRadio {
                    station_id: opts.id.clone(),
                },
            });
            let zone_name = cast::zone_name_for(zones, playerid.0);
            let state = cast_manager.play_radio(playerid, &opts.id, zones).await?;
            cast::fanout_cast_radio_state(
                registry,
                state,
                zone_name,
                audioserver_serial,
                cover_proxy_base,
                events,
            );
            return Ok(());
        }
        // Only radio can be started from Loxone. Ignore anything else (e.g. a legacy
        // `qobuz:` room favourite) and leave any running radio session untouched.
        debug!(
            player_id = playerid.0,
            id = %opts.id,
            "play ignored: not a radio audiopath"
        );
        return Ok(());
    }

    if cast_manager.zone_is_radio(playerid.0) {
        match &cmd {
            IoCommand::Pause(_) => {
                if let Err(e) = cast_manager.transport_pause(playerid).await {
                    cast::reset_cast_zone_on_failure(
                        playerid,
                        "pause",
                        &e,
                        cast_manager,
                        registry,
                        zones,
                        audioserver_serial,
                        cover_proxy_base,
                        events,
                    );
                    return Ok(());
                }
            }
            IoCommand::Play(_, None) => {
                if let Err(e) = cast_manager.transport_play(playerid).await {
                    cast::reset_cast_zone_on_failure(
                        playerid,
                        "play",
                        &e,
                        cast_manager,
                        registry,
                        zones,
                        audioserver_serial,
                        cover_proxy_base,
                        events,
                    );
                    return Ok(());
                }
            }
            IoCommand::Volume(_, step) => {
                let level = match step {
                    VolumeStep::Absolute(l) => *l as u8,
                    VolumeStep::Delta(d) => {
                        let cur = cast_manager
                            .current_volume(playerid)
                            .map(|(level, muted)| if muted { 0 } else { level as i32 })
                            .or_else(|| {
                                zone_volume::get(playerid).map(|v| {
                                    if v.muted {
                                        0
                                    } else {
                                        v.level as i32
                                    }
                                })
                            })
                            .unwrap_or(zone_volume::DEFAULT_FALLBACK_LEVEL as i32);
                        (cur + d).clamp(0, 100) as u8
                    }
                };
                if let Err(e) = zone_volume::set_level(playerid, level) {
                    warn!(player_id = playerid.0, error = %e, "failed to persist cast volume");
                }
                if let Err(e) = cast_manager.transport_volume(playerid, level).await {
                    cast::reset_cast_zone_on_failure(
                        playerid,
                        "volume",
                        &e,
                        cast_manager,
                        registry,
                        zones,
                        audioserver_serial,
                        cover_proxy_base,
                        events,
                    );
                    return Ok(());
                }
            }
            _ => {}
        }
        cast::fanout_cast_radio_from_manager(
            playerid,
            cast_manager,
            registry,
            zones,
            audioserver_serial,
            cover_proxy_base,
            events,
        );
        return Ok(());
    }

    if speaker_manager.is_bound(playerid.0) {
        super::speaker::run_speaker_transport(&cmd, playerid, speaker_manager);
        return Ok(());
    }

    if let IoCommand::Volume(_, step) = &cmd {
        let level = volume_target(*step, zone_volume::get(playerid).map(|v| v.level));
        apply_zone_update(
            registry,
            ZoneUpdate::SetVolume {
                player_id: playerid,
                level,
                muted: false,
            },
            events,
        );
        return Ok(());
    }

    debug!(
        player_id = playerid.0,
        "transport ignored: no backend for zone"
    );
    Ok(())
}

/// Zone a transport command targets (`None` for non-transport commands).
pub(crate) fn command_player_id(cmd: &IoCommand) -> Option<Playerid> {
    match cmd {
        IoCommand::Play(id, _)
        | IoCommand::Pause(id)
        | IoCommand::Volume(id, _)
        | IoCommand::QueuePlus(id)
        | IoCommand::QueueMinus(id)
        | IoCommand::Position(id, _)
        | IoCommand::Repeat(id, _)
        | IoCommand::Shuffle(id, _) => Some(*id),
        _ => None,
    }
}

/// New 0–100 level for a Loxone volume step, from the stored level (or the default).
fn volume_target(step: VolumeStep, stored: Option<u8>) -> u8 {
    let current = i32::from(stored.unwrap_or(zone_volume::DEFAULT_FALLBACK_LEVEL));
    match step {
        VolumeStep::Absolute(level) => level,
        VolumeStep::Delta(delta) => current + delta,
    }
    .clamp(0, 100) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AudioserverConfigFile;
    use crate::player_state::Playerid;
    use crate::remote::PlayOpts;
    use tokio::sync::mpsc;

    /// Run `handle_transport` once against a fresh registry, holding the config test lock.
    fn run_transport(
        cmd: IoCommand,
        registry: &mut ZoneRegistry,
        events: &broadcast::Sender<IoEvent>,
    ) {
        AudioserverConfigFile::with_test_cache(AudioserverConfigFile::default(), || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async {
                let (cast_tx, _cast_rx) = mpsc::channel(4);
                let mut cast_manager = CastPlaybackManager::new(cast_tx);
                let (speaker_tx, _speaker_rx) = mpsc::channel(4);
                let speaker_manager = SpeakerManager::new(speaker_tx);
                handle_transport(
                    cmd,
                    &[],
                    "504F94FF1BB3",
                    events,
                    "http://127.0.0.1:7091",
                    &mut cast_manager,
                    &speaker_manager,
                    registry,
                )
                .await
                .unwrap();
            });
        });
    }

    #[test]
    fn volume_without_backend_is_persisted_and_published() {
        let (events, mut rx) = broadcast::channel(8);
        let mut registry = ZoneRegistry::new();
        run_transport(
            IoCommand::Volume(Playerid(2), VolumeStep::Absolute(42)),
            &mut registry,
            &events,
        );
        match rx.try_recv().expect("audio_event") {
            IoEvent::State(state) => assert_eq!(state.volume, 42.0),
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(registry.snapshot(Playerid(2)).volume, 42.0);
    }

    #[test]
    fn non_radio_play_leaves_a_radio_zone_untouched() {
        let (events, mut rx) = broadcast::channel(8);
        let mut registry = ZoneRegistry::new();
        let radio = PlaybackSource::CastRadio {
            station_id: "tunein:station:s1".into(),
        };
        registry.apply(ZoneUpdate::SetSource {
            player_id: Playerid(4),
            source: radio.clone(),
        });
        run_transport(
            IoCommand::Play(
                Playerid(4),
                Some(PlayOpts {
                    id: "qobuz:track:1".into(),
                    session_token: String::new(),
                }),
            ),
            &mut registry,
            &events,
        );
        assert_eq!(registry.source(Playerid(4)), radio);
        assert!(rx.try_recv().is_err(), "nothing published");
    }

    #[test]
    fn volume_target_applies_steps_and_clamps() {
        assert_eq!(volume_target(VolumeStep::Delta(3), Some(40)), 43);
        assert_eq!(volume_target(VolumeStep::Delta(-50), Some(40)), 0);
        assert_eq!(volume_target(VolumeStep::Absolute(120), Some(40)), 100);
        assert_eq!(
            volume_target(VolumeStep::Delta(1), None),
            zone_volume::DEFAULT_FALLBACK_LEVEL + 1
        );
    }
}
