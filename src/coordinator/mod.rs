//! Async orchestration: `IoCommand` loop, browse, config, transport dispatch, Cast routing.
//!
//! Authoritative zone snapshots live in [`crate::zone_registry`]; commit and fan-out via
//! [`crate::zone_publish`]. Loxone presentation (cover proxy, UI envelope) is
//! [`crate::zone_enrich`]. Connect-time snapshots are [`crate::connect_bootstrap`].

mod browse;
mod cast;
mod config;
mod diagnosis;
mod speaker;
mod status;
mod transport;
mod voice;

use crate::{
    app_config::AppConfig,
    cast::CastPlaybackManager,
    connect_bootstrap::load_serial_config,
    loxone_config::{LoxoneRuntimeState, SerialWithConfig, ZoneDescriptor},
    player_state::{Mode, Playerid},
    remote::{IoCommand, IoEvent},
    speaker::{DiscoveredSpeaker, SpeakerEvent, SpeakerManager},
    zone_registry::{ZoneRegistry, ZoneUpdate},
};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, RwLock};
use tracing::{debug, warn};

/// Channel IDs used for `hw_event` broadcasts (updated when config changes).
pub type HwChannelIds = Arc<RwLock<Vec<String>>>;

pub fn new_hw_channel_ids() -> HwChannelIds {
    Arc::new(RwLock::new(Vec::new()))
}

pub use crate::zone_enrich::apply_cover_proxy;
pub use crate::zone_publish::{apply_zone_update, publish_zone_state};
pub(crate) use diagnosis::DiagnosisReport;

/// Run the coordinator. `speaker_events` is the channel speaker discovery and poll tasks report
/// on; its sender is also handed to the speaker manager.
pub async fn run(
    app_config: AppConfig,
    mut commands: mpsc::Receiver<IoCommand>,
    speaker_events: (mpsc::Sender<SpeakerEvent>, mpsc::Receiver<SpeakerEvent>),
    events: broadcast::Sender<IoEvent>,
    hw_channels: HwChannelIds,
) -> anyhow::Result<()> {
    let mac_id = app_config.serial.clone();
    let cover_proxy_base = app_config.cover_proxy_base();
    let mut runtime = LoxoneRuntimeState::default();
    let mut serial_with_config: Option<SerialWithConfig> = load_serial_config();
    let mut zones: Vec<ZoneDescriptor> = Vec::new();
    let (cast_tx, mut cast_rx) = mpsc::channel(32);
    let mut cast_manager = CastPlaybackManager::new(cast_tx);
    let mut registry = ZoneRegistry::new();
    let (speaker_tx, mut speaker_rx) = speaker_events;
    let mut speaker_manager = SpeakerManager::new(speaker_tx);
    let mut discovered_speakers: Vec<DiscoveredSpeaker> = Vec::new();
    let (voice_tx, mut voice_rx) = mpsc::channel::<voice::VoiceDone>(8);
    let mut voice = voice::VoiceMessages::new(
        std::path::Path::new(crate::voice_upload::UPLOAD_DIR),
        &cover_proxy_base,
        voice_tx,
    );

    config::init_from_serial_config(
        &serial_with_config,
        &mut runtime,
        &mut zones,
        &hw_channels,
        &mac_id,
    )
    .await?;
    speaker::sync_speakers(&mut speaker_manager, &discovered_speakers);
    tokio::spawn(crate::cast::warm_known_cast_devices(
        std::time::Duration::from_secs(3),
    ));

    loop {
        tokio::select! {
            cmd = commands.recv() => {
                let Some(cmd) = cmd else { break };
                match cmd {
            IoCommand::LoxoneSetConfig { raw, parsed } => {
                config::handle_loxone_set_config(
                    raw,
                    parsed,
                    &mut serial_with_config,
                    &mut runtime,
                    &mut zones,
                    &hw_channels,
                    &mac_id,
                )
                .await?;
                speaker::sync_speakers(&mut speaker_manager, &discovered_speakers);
                let _ = events.send(IoEvent::ConfigUpdated(runtime.clone()));
            }
            IoCommand::LoxoneSpeakerTypes(types) => {
                runtime.speaker_types = types;
                let _ = events.send(IoEvent::ConfigUpdated(runtime.clone()));
            }
            IoCommand::LoxoneVolumes(vols) => {
                config::handle_loxone_volumes(&vols, &mut runtime).await;
                let _ = events.send(IoEvent::ConfigUpdated(runtime.clone()));
            }
            IoCommand::Players(player_names) | IoCommand::LoxonePlayerNames(player_names) => {
                runtime.player_names = player_names.players.clone();
                config::handle_player_names(&player_names, &mut zones).await?;
                speaker::sync_speakers(&mut speaker_manager, &discovered_speakers);
                let _ = events.send(IoEvent::ConfigUpdated(runtime.clone()));
            }
            IoCommand::LoxonePlayerOpts(opts) => {
                runtime.player_opts = opts;
                let _ = events.send(IoEvent::ConfigUpdated(runtime.clone()));
            }
            IoCommand::LoxoneReady => {
                config::handle_loxone_ready(&serial_with_config).await?;
                debug!(zone_count = zones.len(), "miniserver ready");
                let _ = events.send(IoEvent::ConfigUpdated(runtime.clone()));
            }
            IoCommand::Status(playerid, reply) => {
                let mut state = registry.snapshot(playerid);
                if state.title.is_empty()
                    && matches!(state.mode, Mode::Stop)
                    && !registry.is_cast_owned(playerid)
                {
                    state = status::refresh_zone_from_backends(
                        playerid,
                        &zones,
                        &runtime,
                        &mac_id,
                        &cover_proxy_base,
                        &cast_manager,
                    )
                    .await;
                    status::seed_registry_from_status(&mut registry, playerid, &state);
                } else {
                    apply_cover_proxy(&mut state, Some(cover_proxy_base.as_str()));
                }
                let _ = reply.send(state);
            }
            IoCommand::AllPlayerStatus(reply) => {
                let states = status::all_player_states(
                    &mut registry,
                    &zones,
                    &runtime,
                    &mac_id,
                    &cover_proxy_base,
                    &cast_manager,
                )
                .await;
                let _ = reply.send(states);
            }
            IoCommand::GetMediaFolder(opts) => {
                let result = browse::handle_get_media_folder(&opts.id, opts.start).await;
                let _ = opts.sender.send(result);
            }
            IoCommand::GetServiceFolder {
                service,
                folder_id,
                start,
                max_items,
                sender,
            } => {
                let result = browse::handle_get_service_folder(
                    &service,
                    &folder_id,
                    start,
                    max_items,
                )
                .await;
                let _ = sender.send(result);
            }
            IoCommand::GlobalSearch {
                provider_spec,
                query,
                sender,
            } => {
                let result = browse::handle_global_search(&provider_spec, &query).await;
                let _ = sender.send(result);
            }
            IoCommand::SearchCategory {
                service,
                kind,
                query,
                start,
                max_items,
                sender,
            } => {
                let result = browse::handle_search_category(
                    &service,
                    &kind,
                    &query,
                    start,
                    max_items,
                )
                .await;
                let _ = sender.send(result);
            }
            IoCommand::Diagnosis(reply) => {
                let _ = reply.send(diagnosis::gather(
                    &mac_id,
                    serial_with_config.is_some(),
                    &zones,
                    &discovered_speakers,
                    &speaker_manager,
                    &cast_manager,
                ));
            }
            IoCommand::SpeakerStation { player_id, reply } => {
                let _ = reply.send(speaker::current_speaker_station(&speaker_manager, player_id));
            }
            IoCommand::PlaySpeakerStation {
                player_id,
                kind,
                media,
            } => {
                voice.note_transport(player_id);
                speaker::play_speaker_station(
                    player_id,
                    &kind,
                    media,
                    &speaker_manager,
                    &mut cast_manager,
                    &mut registry,
                    &zones,
                    &mac_id,
                    &cover_proxy_base,
                    &events,
                )
                .await;
            }
            IoCommand::PlayVoiceMessage { file, zones: targets } => {
                let released = voice
                    .start(&file, &targets, &mut cast_manager, &speaker_manager)
                    .await;
                for player_id in released {
                    cast::release_cast_zone(
                        player_id,
                        &mut registry,
                        &zones,
                        &mac_id,
                        &cover_proxy_base,
                        &events,
                    );
                    speaker_manager.refresh(player_id.0);
                }
            }
            cmd => {
                // A user command for a zone with a message playing: leave the zone as the user
                // sets it afterwards. Volume alone does not count.
                if let Some(player_id) = transport::command_player_id(&cmd) {
                    if !matches!(cmd, IoCommand::Volume(..)) {
                        voice.note_transport(player_id);
                    }
                }
                let zone = transport::command_player_id(&cmd)
                    .map(|pid| (pid, registry.is_cast_owned(pid)));
                if let Err(e) = transport::handle_transport(
                    cmd,
                    &zones,
                    &mac_id,
                    &events,
                    &cover_proxy_base,
                    &mut cast_manager,
                    &speaker_manager,
                    &mut registry,
                )
                .await
                {
                    warn!(error = %e, "remote transport command failed");
                }
                if let Some((pid, was_cast)) = zone {
                    refresh_speaker_if_cast_released(&registry, &speaker_manager, pid, was_cast);
                }
            }
                }
            }
            update = cast_rx.recv() => {
                if let Some(update) = update {
                    let pid = Playerid(update.player_id());
                    let was_cast = registry.is_cast_owned(pid);
                    cast::handle_cast_remote_update(
                        update,
                        &mut cast_manager,
                        &mut registry,
                        &zones,
                        &runtime,
                        &mac_id,
                        &cover_proxy_base,
                        &events,
                    )
                    .await;
                    refresh_speaker_if_cast_released(&registry, &speaker_manager, pid, was_cast);
                }
            }
            done = voice_rx.recv() => {
                if let Some(done) = done {
                    voice_done(
                        voice.finished(done),
                        &zones,
                        &mac_id,
                        &events,
                        &cover_proxy_base,
                        &mut cast_manager,
                        &speaker_manager,
                        &mut registry,
                    )
                    .await;
                }
            }
            event = speaker_rx.recv() => {
                if let Some(event) = event {
                    speaker::handle_speaker_event(
                        event,
                        &mut speaker_manager,
                        &mut discovered_speakers,
                        &mut registry,
                        &zones,
                        &mac_id,
                        &cover_proxy_base,
                        &events,
                    );
                }
            }
        }
    }
    Ok(())
}

/// A zone's last voice message is over: replay the speaker's own radio, restart the Cast radio
/// stopped for it at the volume it had, then re-read the speaker so the zone shows what plays.
/// A failed radio restart releases the zone again, so it is not left marked as Cast radio.
#[allow(clippy::too_many_arguments)]
async fn voice_done(
    restore: Option<voice::Restore>,
    zones: &[ZoneDescriptor],
    mac_id: &str,
    events: &broadcast::Sender<IoEvent>,
    cover_proxy_base: &str,
    cast_manager: &mut CastPlaybackManager,
    speaker_manager: &SpeakerManager,
    registry: &mut ZoneRegistry,
) {
    let Some(restore) = restore else {
        return;
    };
    let player_id = restore.player_id;
    if let Some(speaker) = restore.speaker {
        voice::spawn_speaker_resume(player_id, speaker);
    }
    if let Some(radio) = restore.radio {
        for cmd in voice::radio_restart_commands(player_id, radio) {
            if let Err(e) = transport::handle_transport(
                cmd,
                zones,
                mac_id,
                events,
                cover_proxy_base,
                cast_manager,
                speaker_manager,
                registry,
            )
            .await
            {
                warn!(player_id = player_id.0, error = %e, "radio restart after voice message failed");
                if !cast_manager.zone_is_radio(player_id.0) {
                    cast::release_cast_zone(
                        player_id,
                        registry,
                        zones,
                        mac_id,
                        cover_proxy_base,
                        events,
                    );
                }
                break;
            }
        }
    }
    speaker_manager.refresh(player_id.0);
}

/// When a Cast radio session stops owning a zone, re-publish the zone's speaker status: the
/// registry ignored it while Cast owned the zone, and the speaker poll only reports changes.
fn refresh_speaker_if_cast_released(
    registry: &ZoneRegistry,
    speaker_manager: &SpeakerManager,
    player_id: Playerid,
    was_cast_owned: bool,
) {
    if was_cast_owned && !registry.is_cast_owned(player_id) {
        speaker_manager.refresh(player_id.0);
    }
}

pub(crate) fn preprocess_zone_update(registry: &ZoneRegistry, update: &mut ZoneUpdate) {
    let ZoneUpdate::SetPlayerState {
        player_id, state, ..
    } = update
    else {
        return;
    };
    let prior = registry.snapshot(*player_id);
    if crate::recents::should_record_recent(&prior, state) {
        if let Err(e) = crate::recents::record_from_player_state(player_id.0, state) {
            debug!(player_id = player_id.0, error = %e, "failed to record recently played");
        }
    }
}

#[cfg(test)]
mod io_command_tests {
    use super::*;
    use crate::player_state::Playerid;
    use std::time::Duration;
    use tokio::sync::oneshot;

    #[tokio::test]
    async fn get_media_folder_returns_empty_library_browse() {
        use crate::remote::BrowseOpts;

        // run() reads and (on setconfig) writes the process-wide config.
        let _config = crate::config::AudioserverConfigFile::lock_for_test();
        let app_config = AppConfig::new("504F94FF1BB3".into(), None, None, None).unwrap();
        let (tx, rx) = mpsc::channel(8);
        let speaker_channel = mpsc::channel(8);
        let (events, _) = broadcast::channel(8);
        let hw = new_hw_channel_ids();

        let coordinator = tokio::spawn(async move {
            let _ = run(app_config, rx, speaker_channel, events, hw).await;
        });

        let (reply_tx, reply_rx) = oneshot::channel();
        tx.send(IoCommand::GetMediaFolder(BrowseOpts {
            id: "0".into(),
            start: 0,
            sender: reply_tx,
            session_token: String::new(),
        }))
        .await
        .unwrap();

        let result = tokio::time::timeout(Duration::from_secs(2), reply_rx)
            .await
            .expect("browse timed out")
            .expect("browse dropped");
        assert_eq!(result.id, "0");
        assert!(result.items.is_empty());

        coordinator.abort();
    }

    #[tokio::test]
    async fn pause_on_idle_zone_without_connect_session() {
        // run() reads and (on setconfig) writes the process-wide config.
        let _config = crate::config::AudioserverConfigFile::lock_for_test();
        let app_config = AppConfig::new("504F94FF1BB3".into(), None, None, None).unwrap();
        let (tx, rx) = mpsc::channel(8);
        let speaker_channel = mpsc::channel(8);
        let (events, _) = broadcast::channel(8);
        let hw = new_hw_channel_ids();

        let coordinator = tokio::spawn(async move {
            let _ = run(app_config, rx, speaker_channel, events, hw).await;
        });

        tx.send(IoCommand::Pause(Playerid(1))).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        coordinator.abort();
    }

    #[tokio::test]
    async fn loxone_set_config_populates_zones() {
        // run() reads and (on setconfig) writes the process-wide config.
        let _config = crate::config::AudioserverConfigFile::lock_for_test();
        let app_config = AppConfig::new("504F94FF1BB3".into(), None, None, None).unwrap();
        let (tx, rx) = mpsc::channel(8);
        let speaker_channel = mpsc::channel(8);
        let (events, _) = broadcast::channel(8);
        let hw = new_hw_channel_ids();

        let coordinator = tokio::spawn(async move {
            let _ = run(app_config, rx, speaker_channel, events, hw).await;
        });

        let raw = br#"[{"504F94FF1BB3":{"master":"504F94000002","ip":"x","port":80,"language":"ENU","blinkpos":0,"autoUpdate":120,"unitTemperature":"C","name":"AS","errortts":false,"uuid":"srv","pwHash":"","players":[{"uuid":"zone-uuid-2","playerid":2,"speakerConfig":4,"bt":"on","outputs":[]}],"extensions":[],"groups":[]}}]"#.to_vec();
        let parsed: crate::loxone_config::SerialWithConfig = serde_json::from_slice(&raw).unwrap();
        tx.send(IoCommand::LoxoneSetConfig { raw, parsed })
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;

        let (reply_tx, reply_rx) = oneshot::channel();
        tx.send(IoCommand::Status(Playerid(2), reply_tx))
            .await
            .unwrap();
        let state = tokio::time::timeout(Duration::from_secs(2), reply_rx)
            .await
            .expect("status timed out")
            .expect("status dropped");
        assert_eq!(state.playerid, Playerid(2));

        coordinator.abort();
    }

    #[tokio::test]
    async fn all_player_status_returns_vec() {
        // run() reads and (on setconfig) writes the process-wide config.
        let _config = crate::config::AudioserverConfigFile::lock_for_test();
        let app_config = AppConfig::new("504F94FF1BB3".into(), None, None, None).unwrap();
        let (tx, rx) = mpsc::channel(8);
        let speaker_channel = mpsc::channel(8);
        let (events, _) = broadcast::channel(8);
        let hw = new_hw_channel_ids();

        let coordinator = tokio::spawn(async move {
            let _ = run(app_config, rx, speaker_channel, events, hw).await;
        });

        let (reply_tx, reply_rx) = oneshot::channel();
        tx.send(IoCommand::AllPlayerStatus(reply_tx)).await.unwrap();
        let states = tokio::time::timeout(Duration::from_secs(2), reply_rx)
            .await
            .expect("all status timed out")
            .expect("all status dropped");
        assert!(!states.is_empty());

        coordinator.abort();
    }

    #[tokio::test]
    async fn diagnosis_reports_the_audioserver_serial() {
        let _config = crate::config::AudioserverConfigFile::lock_for_test();
        let app_config = AppConfig::new("504F94000002".into(), None, None, None).unwrap();
        let (tx, rx) = mpsc::channel(8);
        let speaker_channel = mpsc::channel(8);
        let (events, _) = broadcast::channel(8);
        let hw = new_hw_channel_ids();

        let coordinator = tokio::spawn(async move {
            let _ = run(app_config, rx, speaker_channel, events, hw).await;
        });

        let (reply_tx, reply_rx) = oneshot::channel();
        tx.send(IoCommand::Diagnosis(reply_tx)).await.unwrap();
        let report = tokio::time::timeout(Duration::from_secs(2), reply_rx)
            .await
            .expect("diagnosis timed out")
            .expect("diagnosis dropped");
        let v = serde_json::to_value(report).unwrap();
        assert_eq!(v["audioserver"]["serial"], "504F94000002");
        assert!(v["zones"].is_array());

        coordinator.abort();
    }

    #[tokio::test]
    async fn volume_on_idle_zone_without_connect_session() {
        use crate::zone_transport::VolumeStep;

        // run() reads and (on setconfig) writes the process-wide config.
        let _config = crate::config::AudioserverConfigFile::lock_for_test();
        let app_config = AppConfig::new("504F94FF1BB3".into(), None, None, None).unwrap();
        let (tx, rx) = mpsc::channel(8);
        let speaker_channel = mpsc::channel(8);
        let (events, _) = broadcast::channel(8);
        let hw = new_hw_channel_ids();

        let coordinator = tokio::spawn(async move {
            let _ = run(app_config, rx, speaker_channel, events, hw).await;
        });

        tx.send(IoCommand::Volume(Playerid(1), VolumeStep::Absolute(42)))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        coordinator.abort();
    }

    #[tokio::test]
    async fn loxone_ready_emits_config_updated() {
        use crate::remote::IoEvent;

        // run() reads and (on setconfig) writes the process-wide config.
        let _config = crate::config::AudioserverConfigFile::lock_for_test();
        let app_config = AppConfig::new("504F94FF1BB3".into(), None, None, None).unwrap();
        let (tx, rx) = mpsc::channel(8);
        let speaker_channel = mpsc::channel(8);
        let (events, _events_tx) = broadcast::channel(8);
        let mut event_rx = events.subscribe();
        let hw = new_hw_channel_ids();

        let coordinator = tokio::spawn(async move {
            let _ = run(app_config, rx, speaker_channel, events, hw).await;
        });

        let raw = br#"[{"504F94FF1BB3":{"master":"504F94000002","ip":"x","port":80,"language":"ENU","blinkpos":0,"autoUpdate":120,"unitTemperature":"C","name":"AS","errortts":false,"uuid":"srv","pwHash":"","players":[{"uuid":"zone-uuid-1","playerid":1,"speakerConfig":4,"bt":"on","outputs":[]}],"extensions":[],"groups":[]}}]"#.to_vec();
        let parsed: crate::loxone_config::SerialWithConfig = serde_json::from_slice(&raw).unwrap();
        tx.send(IoCommand::LoxoneSetConfig { raw, parsed })
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        tx.send(IoCommand::LoxoneReady).await.unwrap();
        let evt = tokio::time::timeout(Duration::from_secs(2), event_rx.recv())
            .await
            .expect("loxone ready timed out")
            .expect("event channel closed");
        assert!(matches!(evt, IoEvent::ConfigUpdated(_)));

        coordinator.abort();
    }

    #[tokio::test]
    async fn loxone_set_config_empty_clears_state() {
        use crate::remote::IoEvent;

        // run() reads and (on setconfig) writes the process-wide config.
        let _config = crate::config::AudioserverConfigFile::lock_for_test();
        let app_config = AppConfig::new("504F94FF1BB3".into(), None, None, None).unwrap();
        let (tx, rx) = mpsc::channel(8);
        let speaker_channel = mpsc::channel(8);
        let (events, _events_tx) = broadcast::channel(8);
        let mut event_rx = events.subscribe();
        let hw = new_hw_channel_ids();

        let coordinator = tokio::spawn(async move {
            let _ = run(app_config, rx, speaker_channel, events, hw).await;
        });

        let raw = br#"[{"504F94FF1BB3":{"master":"504F94000002","ip":"x","port":80,"language":"ENU","blinkpos":0,"autoUpdate":120,"unitTemperature":"C","name":"AS","errortts":false,"uuid":"srv","pwHash":"","players":[{"uuid":"zone-uuid-2","playerid":2,"speakerConfig":4,"bt":"on","outputs":[]}],"extensions":[],"groups":[]}}]"#.to_vec();
        let parsed: crate::loxone_config::SerialWithConfig = serde_json::from_slice(&raw).unwrap();
        tx.send(IoCommand::LoxoneSetConfig {
            raw: raw.clone(),
            parsed,
        })
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        while event_rx.try_recv().is_ok() {}

        let clear_raw = b"[]".to_vec();
        let clear_parsed = [std::collections::HashMap::new()];
        tx.send(IoCommand::LoxoneSetConfig {
            raw: clear_raw,
            parsed: clear_parsed,
        })
        .await
        .unwrap();

        let evt = tokio::time::timeout(Duration::from_secs(2), event_rx.recv())
            .await
            .expect("clear timed out")
            .expect("event channel closed");
        match evt {
            IoEvent::ConfigUpdated(runtime) => assert!(runtime.config.is_none()),
            other => panic!("unexpected event: {other:?}"),
        }

        coordinator.abort();
    }
}

#[cfg(test)]
mod zone_registry_harness_tests {
    use super::*;
    use crate::player_state::{PlayerState, Playerid};
    use crate::remote::IoEvent;
    use crate::zone_registry::{ApplyOutcome, PlaybackSource, ZoneUpdate};
    use std::time::Duration;
    use tokio::sync::{broadcast, oneshot};

    fn recv_state(rx: &mut broadcast::Receiver<IoEvent>) -> Option<PlayerState> {
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
    fn cast_ownership_blocks_qobuz_periodic_update() {
        let mut registry = ZoneRegistry::new();
        let radio = PlayerState {
            playerid: Playerid(4),
            mode: Mode::Play,
            title: "NPR".into(),
            audiopath: "tunein:station:1".into(),
            ..Default::default()
        };
        publish_zone_state(
            &mut registry,
            radio,
            PlaybackSource::CastRadio {
                station_id: "tunein:station:1".into(),
            },
            &broadcast::channel(4).0,
        );
        let outcome = registry.apply(ZoneUpdate::SetPlayerState {
            player_id: Playerid(4),
            state: PlayerState {
                playerid: Playerid(4),
                mode: Mode::Play,
                title: "Qobuz".into(),
                audiopath: "qobuz:track:1".into(),
                ..Default::default()
            },
            source: Some(PlaybackSource::External),
        });
        assert_eq!(outcome, ApplyOutcome::Ignored);
        assert_eq!(registry.snapshot(Playerid(4)).title, "NPR");
    }

    #[test]
    fn transport_pause_reaches_all_subscribers() {
        let (events, mut rx1) = broadcast::channel(16);
        let mut rx2 = events.subscribe();
        let mut registry = ZoneRegistry::new();
        publish_zone_state(
            &mut registry,
            PlayerState {
                playerid: Playerid(4),
                mode: Mode::Play,
                title: "Track".into(),
                audiopath: "qobuz:1".into(),
                ..Default::default()
            },
            PlaybackSource::External,
            &events,
        );
        let _ = recv_state(&mut rx1);
        let _ = recv_state(&mut rx2);
        publish_zone_state(
            &mut registry,
            PlayerState {
                playerid: Playerid(4),
                mode: Mode::Pause,
                title: "Track".into(),
                audiopath: "qobuz:1".into(),
                ..Default::default()
            },
            PlaybackSource::External,
            &events,
        );
        assert_eq!(recv_state(&mut rx1).map(|s| s.mode), Some(Mode::Pause));
        assert_eq!(recv_state(&mut rx2).map(|s| s.mode), Some(Mode::Pause));
    }

    #[test]
    fn status_snapshot_matches_last_published_event() {
        let (events, mut rx) = broadcast::channel(8);
        let mut registry = ZoneRegistry::new();
        let live = PlayerState {
            playerid: Playerid(2),
            mode: Mode::Play,
            title: "Live".into(),
            audiopath: "qobuz:live".into(),
            ..Default::default()
        };
        publish_zone_state(
            &mut registry,
            live.clone(),
            PlaybackSource::External,
            &events,
        );
        let published = recv_state(&mut rx).expect("published");
        let status = registry.snapshot(Playerid(2));
        assert_eq!(published, status);
    }

    #[tokio::test]
    async fn status_command_reads_registry_snapshot() {
        // run() reads and (on setconfig) writes the process-wide config.
        let _config = crate::config::AudioserverConfigFile::lock_for_test();
        let app_config = AppConfig::new("504F94FF1BB3".into(), None, None, None).unwrap();
        let (tx, rx) = mpsc::channel(8);
        let speaker_channel = mpsc::channel(8);
        let (events, mut event_rx) = broadcast::channel(8);
        let hw = new_hw_channel_ids();

        let coordinator = tokio::spawn(async move {
            let _ = run(app_config, rx, speaker_channel, events, hw).await;
        });

        let (reply_tx, reply_rx) = oneshot::channel();
        tx.send(IoCommand::Status(Playerid(1), reply_tx))
            .await
            .unwrap();
        let idle = tokio::time::timeout(Duration::from_secs(2), reply_rx)
            .await
            .expect("status timed out")
            .expect("status dropped");
        assert_eq!(idle.playerid, Playerid(1));
        assert!(event_rx.try_recv().is_err());

        coordinator.abort();
    }
}

#[cfg(test)]
mod speaker_refresh_tests {
    use super::*;
    use crate::speaker::{
        binding::ZoneSpeakerTarget, fake, fake::FakeSpeaker, manager::PollTiming, SpeakerKind,
        SpeakerPower, SpeakerStatus,
    };
    use crate::zone_registry::PlaybackSource;
    use std::time::Duration;

    #[tokio::test]
    async fn speaker_status_is_republished_when_cast_releases_the_zone() {
        let fake_speaker = FakeSpeaker::with_status(SpeakerStatus {
            power: SpeakerPower::Standby,
            ..Default::default()
        });
        let (tx, mut rx) = mpsc::channel(8);
        let timing = PollTiming {
            idle: Duration::from_secs(60),
            ..PollTiming::default()
        };
        let mut manager =
            SpeakerManager::with_connector(tx, fake::connector(&fake_speaker), timing);
        manager.sync(&[ZoneSpeakerTarget {
            player_id: 4,
            speaker_id: "ls60".into(),
            kind: SpeakerKind::KefStream800,
            host: "10.0.0.4".into(),
        }]);
        let first = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await;
        assert!(matches!(first, Ok(Some(SpeakerEvent::Status { .. }))));

        let mut registry = ZoneRegistry::new();
        registry.apply(ZoneUpdate::SetSource {
            player_id: Playerid(4),
            source: PlaybackSource::CastRadio {
                station_id: "tunein:station:s1".into(),
            },
        });
        // Still owned by Cast: no refresh.
        refresh_speaker_if_cast_released(&registry, &manager, Playerid(4), true);
        assert!(tokio::time::timeout(Duration::from_millis(200), rx.recv())
            .await
            .is_err());

        // Session over: the unchanged standby status is sent again.
        registry.clear_zone(Playerid(4), true);
        refresh_speaker_if_cast_released(&registry, &manager, Playerid(4), true);
        let again = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("status re-sent")
            .unwrap();
        assert!(matches!(
            again,
            SpeakerEvent::Status { player_id: 4, status } if status.power == SpeakerPower::Standby
        ));
    }
}
