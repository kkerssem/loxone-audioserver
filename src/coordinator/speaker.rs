//! Speaker backends in the coordinator: discovery-driven binding, status → registry, transport.

use tokio::sync::broadcast;
use tracing::{debug, info, warn};

use crate::{
    config::ZoneConfig,
    loxone_config::ZoneDescriptor,
    player_state::Playerid,
    remote::{IoCommand, IoEvent},
    speaker::{
        binding::{auto_bind_by_name, resolve_targets},
        discovery::remember,
        map::status_to_player_state,
        DiscoveredSpeaker, SpeakerCommand, SpeakerEvent, SpeakerManager, SpeakerPower,
        SpeakerStatus,
    },
    zone_enrich::enrich_zone_state,
    zone_registry::{ZoneRegistry, ZoneUpdate},
    zone_transport::VolumeStep,
    zone_volume,
};

use super::{apply_zone_update, cast::zone_name_for, preprocess_zone_update};

/// Persist name-based bindings for unbound zones, then (re)bind the manager to the config.
pub(crate) fn sync_speakers(manager: &mut SpeakerManager, discovered: &[DiscoveredSpeaker]) {
    let mut config = ZoneConfig::get();
    let new_bindings = auto_bind_by_name(&config, discovered);
    if !new_bindings.is_empty() {
        let result = ZoneConfig::update(|cfg| {
            for (player_id, speaker_id) in &new_bindings {
                if let Some(player) = cfg.players.iter_mut().find(|p| p.player_id == *player_id) {
                    player.speaker_id = Some(speaker_id.clone());
                }
            }
            Ok(cfg.clone())
        });
        match result {
            Ok(updated) => {
                for (player_id, speaker_id) in &new_bindings {
                    info!(
                        player_id,
                        speaker_id = %speaker_id,
                        "zone bound to speaker by name"
                    );
                }
                config = updated;
            }
            Err(e) => warn!(error = %e, "failed to persist speaker bindings"),
        }
    }
    let (targets, problems) = resolve_targets(&config, discovered);
    for problem in problems {
        warn!(
            player_id = problem.player_id,
            problem = %problem.message,
            "speaker binding misconfigured"
        );
    }
    manager.sync(&targets);
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_speaker_event(
    event: SpeakerEvent,
    manager: &mut SpeakerManager,
    discovered: &mut Vec<DiscoveredSpeaker>,
    registry: &mut ZoneRegistry,
    zones: &[ZoneDescriptor],
    audioserver_serial: &str,
    cover_proxy_base: &str,
    events: &broadcast::Sender<IoEvent>,
) {
    match event {
        SpeakerEvent::Discovered(found) => {
            // Speakers missing from one browse stay known; the zone's poll loop reports a
            // speaker that is really gone as unreachable.
            if remember(discovered, found) {
                sync_speakers(manager, discovered);
            }
        }
        SpeakerEvent::Status { player_id, status } => publish_speaker_status(
            Playerid(player_id),
            &status,
            registry,
            zones,
            audioserver_serial,
            cover_proxy_base,
            events,
        ),
    }
}

/// Mirror a speaker poll into the zone registry. Skipped entirely while a Cast radio session
/// owns the zone, so the speaker's own volume does not overwrite the level set during radio;
/// the manager is refreshed when the session ends (see the coordinator loop).
pub(crate) fn publish_speaker_status(
    playerid: Playerid,
    status: &SpeakerStatus,
    registry: &mut ZoneRegistry,
    zones: &[ZoneDescriptor],
    audioserver_serial: &str,
    cover_proxy_base: &str,
    events: &broadcast::Sender<IoEvent>,
) {
    if registry.is_cast_owned(playerid) {
        return;
    }
    if status.power == SpeakerPower::On {
        let stored = zone_volume::get(playerid);
        if stored.is_none_or(|v| v.level != status.volume || v.muted != status.muted) {
            if let Err(e) = zone_volume::set(playerid, status.volume, status.muted) {
                warn!(player_id = playerid.0, error = %e, "failed to persist speaker volume");
            }
        }
    }
    let state = enrich_zone_state(
        status_to_player_state(playerid, status),
        playerid,
        zone_name_for(zones, playerid.0),
        None,
        audioserver_serial,
        cover_proxy_base,
    );
    let mut update = ZoneUpdate::SetPlayerState {
        player_id: playerid,
        state,
        source: None,
    };
    preprocess_zone_update(registry, &mut update);
    apply_zone_update(registry, update, events);
}

/// Speaker commands for one Loxone transport command (pure; see [`run_speaker_transport`]).
///
/// An absolute level goes through `clamp_absolute` (Loxone presets: step grid and max
/// volume). A relative step is only capped at `max_volume`: rounding it to the step grid would
/// pull small steps back to the current level, so the button would do nothing.
pub(crate) fn plan_speaker_commands(
    cmd: &IoCommand,
    last: Option<&SpeakerStatus>,
    fallback_volume: u8,
    max_volume: u8,
    clamp_absolute: impl Fn(u8) -> u8,
) -> Vec<SpeakerCommand> {
    match cmd {
        IoCommand::Pause(_) => vec![SpeakerCommand::Pause],
        IoCommand::Play(_, None) => {
            if last.is_some_and(|s| s.power != SpeakerPower::On) {
                vec![SpeakerCommand::Power(true), SpeakerCommand::Play]
            } else {
                vec![SpeakerCommand::Play]
            }
        }
        IoCommand::Volume(_, step) => {
            let current = last
                .filter(|s| s.power == SpeakerPower::On)
                .map(|s| s.volume)
                .unwrap_or(fallback_volume);
            let level = match step {
                VolumeStep::Absolute(level) => clamp_absolute((*level).clamp(0, 100) as u8),
                VolumeStep::Delta(delta) => {
                    (i32::from(current) + delta).clamp(0, i32::from(max_volume)) as u8
                }
            };
            vec![SpeakerCommand::SetVolume(level)]
        }
        // Only within a track that is playing or paused; live radio has no position.
        IoCommand::Position(_, seconds) => match last {
            Some(s)
                if s.power == SpeakerPower::On
                    && s.mode.is_some()
                    && !s.live
                    && s.duration_s > 0 =>
            {
                vec![SpeakerCommand::Seek(
                    (*seconds).clamp(0, s.duration_s as i32) as u32,
                )]
            }
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}

/// Queue a Loxone transport command on the zone's speaker (non-blocking; the zone's own task
/// runs the commands in order and logs failures).
pub(crate) fn run_speaker_transport(cmd: &IoCommand, playerid: Playerid, manager: &SpeakerManager) {
    // Includes a volume queued a moment ago, so quick volume steps add up.
    let last = manager.status_for_commands(playerid.0);
    let fallback = zone_volume::get(playerid)
        .map(|v| v.level)
        .unwrap_or(zone_volume::DEFAULT_FALLBACK_LEVEL);
    let mapping = ZoneConfig::get()
        .players
        .into_iter()
        .find(|p| p.player_id == playerid.0);
    let max_volume = mapping
        .as_ref()
        .and_then(|m| m.loxone_max_volume)
        .unwrap_or(100)
        .clamp(1, 100);
    let commands = plan_speaker_commands(cmd, last.as_ref(), fallback, max_volume, |level| {
        mapping
            .as_ref()
            .map(|m| zone_volume::clamp_volume_to_preset(level, m))
            .unwrap_or(level)
    });
    if commands.is_empty() {
        debug!(
            player_id = playerid.0,
            "transport command not supported by speaker backend"
        );
        return;
    }
    for command in commands {
        if let Err(e) = manager.command(playerid.0, command) {
            warn!(player_id = playerid.0, ?command, error = %e, "speaker command not queued");
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AudioserverConfigFile, PlayerMapping};
    use crate::player_state::Mode;
    use crate::speaker::{binding::ZoneSpeakerTarget, fake, fake::FakeSpeaker, SpeakerKind};
    use crate::zone_registry::PlaybackSource;
    use std::time::Duration;
    use tokio::sync::mpsc;

    fn playing_status() -> SpeakerStatus {
        SpeakerStatus {
            mode: Some(Mode::Play),
            volume: 40,
            source: "wifi".into(),
            title: "Hey Jude".into(),
            artist: "The Beatles".into(),
            cover_url: "http://192.168.50.209/art.jpg".into(),
            duration_s: 431,
            position_s: 5,
            ..Default::default()
        }
    }

    fn with_zone_config(f: impl FnOnce()) {
        AudioserverConfigFile::with_test_cache(
            AudioserverConfigFile {
                audioserver: ZoneConfig {
                    players: vec![PlayerMapping {
                        player_id: 2,
                        name: "Mu-so Qb".into(),
                        ..Default::default()
                    }],
                },
                ..Default::default()
            },
            f,
        );
    }

    #[test]
    fn speaker_status_publishes_audio_event_with_metadata() {
        with_zone_config(|| {
            let (events, mut rx) = broadcast::channel(8);
            let mut registry = ZoneRegistry::new();
            publish_speaker_status(
                Playerid(2),
                &playing_status(),
                &mut registry,
                &[],
                "504F94FF1BB3",
                "http://10.0.0.2:7091",
                &events,
            );
            let IoEvent::State(state) = rx.try_recv().expect("audio_event") else {
                panic!("expected state event");
            };
            assert_eq!(state.title, "Hey Jude");
            assert_eq!(state.mode, Mode::Play);
            assert_eq!(state.volume, 40.0);
            assert!(state.cover_url.contains("/cors-proxy?url="));
            assert_eq!(registry.source(Playerid(2)), PlaybackSource::External);
            assert_eq!(zone_volume::get(Playerid(2)).unwrap().level, 40);

            // Same status again: deduplicated, no second event.
            publish_speaker_status(
                Playerid(2),
                &playing_status(),
                &mut registry,
                &[],
                "504F94FF1BB3",
                "http://10.0.0.2:7091",
                &events,
            );
            assert!(rx.try_recv().is_err());
        });
    }

    #[test]
    fn cast_radio_session_wins_over_speaker_polls() {
        with_zone_config(|| {
            let (events, mut rx) = broadcast::channel(8);
            let mut registry = ZoneRegistry::new();
            registry.apply(ZoneUpdate::SetSource {
                player_id: Playerid(2),
                source: PlaybackSource::CastRadio {
                    station_id: "tunein:station:s1".into(),
                },
            });
            publish_speaker_status(
                Playerid(2),
                &playing_status(),
                &mut registry,
                &[],
                "504F94FF1BB3",
                "http://10.0.0.2:7091",
                &events,
            );
            assert!(rx.try_recv().is_err());
            assert!(registry.is_cast_owned(Playerid(2)));
            assert!(
                zone_volume::get(Playerid(2)).is_none(),
                "speaker volume must not overwrite the level set during radio"
            );
        });
    }

    #[tokio::test]
    async fn transport_reaches_bound_speaker_in_order() {
        // run_speaker_transport reads the process-wide config (presets, stored volume).
        let _config = AudioserverConfigFile::lock_for_test();
        let fake_speaker = FakeSpeaker::with_status(SpeakerStatus {
            power: SpeakerPower::Standby,
            volume: 20,
            ..Default::default()
        });
        let (tx, mut rx) = mpsc::channel(16);
        let mut manager = SpeakerManager::with_connector(
            tx,
            fake::connector(&fake_speaker),
            crate::speaker::manager::PollTiming {
                playing: Duration::from_millis(20),
                idle: Duration::from_millis(20),
                after_command: Duration::from_millis(20),
                after_command_window: Duration::from_millis(100),
                request_timeout: Duration::from_millis(200),
                failures_until_unreachable: 3,
                wake_timeout: Duration::from_secs(2),
            },
        );
        manager.sync(&[ZoneSpeakerTarget {
            player_id: 2,
            speaker_id: "naim".into(),
            kind: SpeakerKind::NaimStream800,
            host: "10.0.0.7".into(),
        }]);
        // Wait for the first poll so the manager knows the speaker is in standby.
        tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap();

        // Queued, not awaited: the zone task runs power-on, waits for "on", then plays.
        run_speaker_transport(&IoCommand::Play(Playerid(2), None), Playerid(2), &manager);
        loop {
            match tokio::time::timeout(Duration::from_secs(2), rx.recv())
                .await
                .unwrap()
                .unwrap()
            {
                SpeakerEvent::Status { status, .. } if status.mode == Some(Mode::Play) => break,
                _ => {}
            }
        }
        run_speaker_transport(
            &IoCommand::Volume(Playerid(2), VolumeStep::Delta(5)),
            Playerid(2),
            &manager,
        );
        run_speaker_transport(&IoCommand::QueuePlus(Playerid(2)), Playerid(2), &manager);
        loop {
            match tokio::time::timeout(Duration::from_secs(2), rx.recv())
                .await
                .unwrap()
                .unwrap()
            {
                SpeakerEvent::Status { status, .. } if status.volume == 25 => break,
                _ => {}
            }
        }
        assert_eq!(
            fake_speaker.commands(),
            vec![
                SpeakerCommand::Power(true),
                SpeakerCommand::Play,
                SpeakerCommand::SetVolume(25),
            ]
        );
    }

    fn on(volume: u8) -> SpeakerStatus {
        SpeakerStatus {
            volume,
            mode: Some(Mode::Pause),
            ..Default::default()
        }
    }

    /// Presets used by most tests: max volume 80, no step grid.
    fn plan(cmd: IoCommand, last: Option<SpeakerStatus>) -> Vec<SpeakerCommand> {
        plan_speaker_commands(&cmd, last.as_ref(), 50, 80, |v| v.min(80))
    }

    #[test]
    fn pause_and_play_map_directly() {
        assert_eq!(
            plan(IoCommand::Pause(Playerid(1)), Some(on(10))),
            vec![SpeakerCommand::Pause]
        );
        assert_eq!(
            plan(IoCommand::Play(Playerid(1), None), Some(on(10))),
            vec![SpeakerCommand::Play]
        );
        assert_eq!(
            plan(IoCommand::Play(Playerid(1), None), None),
            vec![SpeakerCommand::Play]
        );
    }

    #[test]
    fn play_from_standby_powers_on_first() {
        let standby = SpeakerStatus {
            power: SpeakerPower::Standby,
            ..Default::default()
        };
        assert_eq!(
            plan(IoCommand::Play(Playerid(1), None), Some(standby)),
            vec![SpeakerCommand::Power(true), SpeakerCommand::Play]
        );
    }

    #[test]
    fn volume_delta_uses_speaker_level_then_clamps() {
        assert_eq!(
            plan(
                IoCommand::Volume(Playerid(1), VolumeStep::Delta(3)),
                Some(on(30))
            ),
            vec![SpeakerCommand::SetVolume(33)]
        );
        assert_eq!(
            plan(
                IoCommand::Volume(Playerid(1), VolumeStep::Delta(-40)),
                Some(on(30))
            ),
            vec![SpeakerCommand::SetVolume(0)]
        );
        // Preset max volume (80 here) caps the result.
        assert_eq!(
            plan(
                IoCommand::Volume(Playerid(1), VolumeStep::Absolute(95)),
                Some(on(30))
            ),
            vec![SpeakerCommand::SetVolume(80)]
        );
    }

    #[test]
    fn relative_steps_are_not_pulled_back_by_the_step_grid() {
        // Volume step preset 5: absolute levels snap to the grid, small relative steps still
        // move the volume instead of rounding back to the current level.
        let grid5 = |v: u8| ((f32::from(v) / 5.0).round() * 5.0) as u8;
        let plan5 = |cmd: IoCommand| plan_speaker_commands(&cmd, Some(&on(40)), 50, 100, grid5);
        assert_eq!(
            plan5(IoCommand::Volume(Playerid(1), VolumeStep::Delta(1))),
            vec![SpeakerCommand::SetVolume(41)]
        );
        assert_eq!(
            plan5(IoCommand::Volume(Playerid(1), VolumeStep::Delta(-2))),
            vec![SpeakerCommand::SetVolume(38)]
        );
        assert_eq!(
            plan5(IoCommand::Volume(Playerid(1), VolumeStep::Absolute(43))),
            vec![SpeakerCommand::SetVolume(45)]
        );
        // Relative steps still respect the max volume.
        assert_eq!(
            plan(
                IoCommand::Volume(Playerid(1), VolumeStep::Delta(60)),
                Some(on(30))
            ),
            vec![SpeakerCommand::SetVolume(80)]
        );
    }

    #[test]
    fn volume_delta_falls_back_when_speaker_unknown_or_off() {
        assert_eq!(
            plan(IoCommand::Volume(Playerid(1), VolumeStep::Delta(1)), None),
            vec![SpeakerCommand::SetVolume(51)]
        );
        let standby = SpeakerStatus {
            power: SpeakerPower::Standby,
            volume: 5,
            ..Default::default()
        };
        assert_eq!(
            plan(
                IoCommand::Volume(Playerid(1), VolumeStep::Delta(1)),
                Some(standby)
            ),
            vec![SpeakerCommand::SetVolume(51)]
        );
    }

    #[test]
    fn unsupported_commands_plan_nothing() {
        assert!(plan(IoCommand::QueuePlus(Playerid(1)), Some(on(1))).is_empty());
    }

    #[test]
    fn position_seeks_within_a_track_only() {
        let track = SpeakerStatus {
            duration_s: 240,
            ..on(30)
        };
        let seek = |secs, last: &SpeakerStatus| {
            plan(IoCommand::Position(Playerid(1), secs), Some(last.clone()))
        };
        assert_eq!(seek(95, &track), vec![SpeakerCommand::Seek(95)]);
        assert_eq!(seek(-3, &track), vec![SpeakerCommand::Seek(0)]);
        assert_eq!(seek(999, &track), vec![SpeakerCommand::Seek(240)]);

        let radio = SpeakerStatus {
            live: true,
            ..track.clone()
        };
        assert!(seek(95, &radio).is_empty(), "live radio");
        assert!(seek(95, &on(30)).is_empty(), "no duration");
        let stopped = SpeakerStatus {
            mode: None,
            ..track.clone()
        };
        assert!(seek(95, &stopped).is_empty(), "nothing playing");
        assert!(plan(IoCommand::Position(Playerid(1), 95), None).is_empty());
    }
}
