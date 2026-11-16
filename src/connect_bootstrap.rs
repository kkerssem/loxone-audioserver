//! Connect-time and lag-recovery zone snapshots via coordinator Status.

use std::collections::HashMap;
use std::time::Duration;

use tokio::sync::mpsc;

use crate::app_config::AppConfig;
use crate::config::AudioserverConfigFile;
use crate::loxone_config::{extract_zones, SerialWithConfig};
use crate::player_state::{connected_idle_state, miniserver_idle_state, PlayerState, Playerid};
use crate::remote::IoCommand;
use crate::zone_enrich::apply_cover_proxy;
use crate::zone_registry::encode_audio_event_payload;
use tracing::{debug, info, warn};

/// Which WebSocket surface a client connected on (for logging and idle fallbacks).
#[derive(Debug, Clone, Copy)]
pub enum ConnectSurface {
    Remote7091,
    Miniserver7095,
}

impl ConnectSurface {
    pub fn port(self) -> u16 {
        match self {
            ConnectSurface::Remote7091 => crate::command_audit::PORT_REMOTE,
            ConnectSurface::Miniserver7095 => crate::command_audit::PORT_MINISERVER,
        }
    }
}

pub fn load_serial_config() -> Option<SerialWithConfig> {
    AudioserverConfigFile::get().loxone_state()
}

/// Player ids from audioserver-config.json zones and/or Loxone `loxone.state`.
pub async fn configured_player_ids(app_config: &AppConfig) -> Vec<usize> {
    let mut ids = Vec::new();
    let file = AudioserverConfigFile::get();
    for p in &file.audioserver.players {
        if !ids.contains(&p.player_id) {
            ids.push(p.player_id);
        }
    }
    if let Some(cfg) = file.loxone_state() {
        for zone in extract_zones(&cfg, &app_config.serial) {
            if !ids.contains(&zone.player_id) {
                ids.push(zone.player_id);
            }
        }
    }
    if ids.is_empty() {
        ids.push(1);
    }
    ids
}

pub async fn initial_zone_names(app_config: &AppConfig) -> HashMap<usize, String> {
    let mut names = HashMap::new();
    let file = AudioserverConfigFile::get();
    for p in &file.audioserver.players {
        if !p.name.is_empty() {
            names.insert(p.player_id, p.name.clone());
        }
    }
    if let Some(cfg) = file.loxone_state() {
        for zone in extract_zones(&cfg, &app_config.serial) {
            names
                .entry(zone.player_id)
                .or_insert_with(|| zone.name.clone());
        }
    }
    names
}

fn idle_connect_state(
    surface: ConnectSurface,
    playerid: Playerid,
    zone_name: Option<&str>,
    audioserver_serial: &str,
    default_volume: Option<f32>,
) -> PlayerState {
    match surface {
        ConnectSurface::Remote7091 => connected_idle_state(playerid, zone_name, audioserver_serial),
        ConnectSurface::Miniserver7095 => {
            miniserver_idle_state(playerid, zone_name, audioserver_serial, default_volume)
        }
    }
}

/// Live zone snapshots for connect-time `audio_event` pushes (falls back to idle shell).
pub async fn fetch_connect_player_states(
    from_api: &mpsc::Sender<IoCommand>,
    app_config: &AppConfig,
    surface: ConnectSurface,
) -> Vec<PlayerState> {
    const STATUS_TIMEOUT: Duration = Duration::from_secs(3);
    let cover_base = app_config.cover_proxy_base();
    let zone_names = initial_zone_names(app_config).await;
    let mut states = Vec::new();

    for player_id in configured_player_ids(app_config).await {
        let playerid = Playerid(player_id);
        let zone_name = zone_names.get(&player_id).map(String::as_str);
        let (tx, rx) = tokio::sync::oneshot::channel();
        if from_api
            .send(IoCommand::Status(playerid, tx))
            .await
            .is_err()
        {
            warn!(player_id, "coordinator unavailable for connect status");
            states.push(idle_connect_state(
                surface,
                playerid,
                zone_name,
                &app_config.serial,
                None,
            ));
            continue;
        }
        match tokio::time::timeout(STATUS_TIMEOUT, rx).await {
            Ok(Ok(mut state)) => {
                apply_cover_proxy(&mut state, Some(cover_base.as_str()));
                info!(
                    port = surface.port(),
                    player_id,
                    mode = ?state.mode,
                    title = %state.title,
                    initial_state = "live",
                    "connect audio_event state"
                );
                states.push(state);
            }
            Ok(Err(_)) | Err(_) => {
                debug!(
                    port = surface.port(),
                    player_id,
                    initial_state = "idle",
                    "connect status timed out or dropped; using idle shell"
                );
                states.push(idle_connect_state(
                    surface,
                    playerid,
                    zone_name,
                    &app_config.serial,
                    None,
                ));
            }
        }
    }
    states
}

/// Encode connect `audio_event` payloads for lag recovery (authoritative registry via Status).
pub fn connect_audio_event_payloads(states: &[PlayerState]) -> Vec<String> {
    states
        .iter()
        .filter_map(|state| encode_audio_event_payload(state).ok())
        .collect()
}

/// Resync WebSocket clients after broadcast lag using the same path as connect bootstrap.
pub async fn resync_connect_audio_events(
    from_api: &mpsc::Sender<IoCommand>,
    app_config: &AppConfig,
    surface: ConnectSurface,
) -> Vec<String> {
    connect_audio_event_payloads(&fetch_connect_player_states(from_api, app_config, surface).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::zone::{PlayerMapping, ZoneConfigBody};
    use crate::config::AudioserverConfigFile;
    use crate::player_state::{Mode, Playerid};

    fn player_mapping(player_id: usize, name: &str) -> PlayerMapping {
        PlayerMapping {
            player_id,
            loxone_uuid: None,
            name: name.into(),
            cast_device_id: None,
            volume_level: None,
            ..Default::default()
        }
    }

    fn audioserver_file_with_players(players: Vec<PlayerMapping>) -> AudioserverConfigFile {
        AudioserverConfigFile {
            audioserver: ZoneConfigBody { players },
            ..Default::default()
        }
    }

    fn playing_state(player_id: usize, title: &str) -> PlayerState {
        PlayerState {
            playerid: Playerid(player_id),
            mode: Mode::Play,
            title: title.to_string(),
            audiopath: format!("qobuz:{player_id}"),
            ..Default::default()
        }
    }

    #[test]
    fn connect_surface_port_remote() {
        assert_eq!(
            ConnectSurface::Remote7091.port(),
            crate::command_audit::PORT_REMOTE
        );
        assert_eq!(
            ConnectSurface::Miniserver7095.port(),
            crate::command_audit::PORT_MINISERVER
        );
    }

    #[test]
    fn load_serial_config_missing_returns_none() {
        AudioserverConfigFile::with_test_cache(AudioserverConfigFile::default(), || {
            assert!(load_serial_config().is_none());
        });
    }

    #[test]
    fn connect_audio_event_payloads_encode_lag_resync_shape() {
        let states = vec![playing_state(1, "Live"), playing_state(2, "Other")];
        let payloads = connect_audio_event_payloads(&states);
        assert_eq!(payloads.len(), 2);
        for payload in payloads {
            assert!(payload.contains("\"audio_event\""));
            assert!(payload.contains("\"playerid\""));
        }
    }

    #[test]
    fn configured_player_ids_reads_loxone_state() {
        use crate::loxone_config::extract_zones;

        let state_json: serde_json::Value = serde_json::from_str(
            r#"[{"504F94FF1BB3":{"master":"504F94000002","ip":"x","port":80,"language":"ENU","blinkpos":0,"autoUpdate":120,"unitTemperature":"C","name":"AS","errortts":false,"uuid":"srv","pwHash":"","players":[{"uuid":"zone-2","playerid":2,"speakerConfig":4,"bt":"on","outputs":[]}],"extensions":[],"groups":[]}}]"#,
        )
        .unwrap();
        let file = AudioserverConfigFile {
            loxone: crate::config::LoxonePersist {
                crc32: "00000000".into(),
                state: state_json,
            },
            ..Default::default()
        };
        AudioserverConfigFile::with_test_cache(file, || {
            let cfg = AudioserverConfigFile::get()
                .loxone_state()
                .expect("loxone state");
            let zones = extract_zones(&cfg, "504F94FF1BB3");
            assert_eq!(zones.len(), 1);
            assert_eq!(zones[0].player_id, 2);
            assert_eq!(zones[0].uuid, "zone-2");
        });
    }

    #[test]
    fn configured_player_ids_reads_audioserver_players() {
        let file = audioserver_file_with_players(vec![
            player_mapping(5, "Kitchen"),
            player_mapping(3, "Bedroom"),
        ]);
        let app_config = AppConfig::new("504F94FF1BB3".into(), None, None, None).unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        AudioserverConfigFile::with_test_cache(file, || {
            let ids = rt.block_on(configured_player_ids(&app_config));
            assert!(ids.contains(&5));
            assert!(ids.contains(&3));
        });
    }

    #[test]
    fn fetch_connect_player_states_live_when_coordinator_replies() {
        use ConnectSurface;

        let file = audioserver_file_with_players(vec![player_mapping(1, "Living")]);
        let app_config = AppConfig::new("504F94FF1BB3".into(), None, None, None).unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");

        AudioserverConfigFile::with_test_cache(file, || {
            rt.block_on(async {
                let (tx, mut rx) = mpsc::channel(4);
                let mut fetch = std::pin::pin!(fetch_connect_player_states(
                    &tx,
                    &app_config,
                    ConnectSurface::Remote7091,
                ));

                let states = loop {
                    tokio::select! {
                        states = fetch.as_mut() => break states,
                        cmd = rx.recv() => {
                            match cmd {
                                Some(IoCommand::Status(Playerid(id), reply_tx)) => {
                                    let _ = reply_tx.send(playing_state(id, "Live Title"));
                                }
                                Some(other) => panic!("unexpected coordinator command: {other:?}"),
                                None => panic!("coordinator channel closed"),
                            }
                        }
                    }
                };

                assert_eq!(states.len(), 1);
                assert_eq!(states[0].mode, Mode::Play);
                assert_eq!(states[0].title, "Live Title");
            });
        });
    }

    #[tokio::test]
    async fn fetch_connect_player_states_idle_when_coordinator_unavailable() {
        use ConnectSurface;

        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        let app_config = AppConfig::new("504F94FF1BB3".into(), None, None, None).unwrap();
        let states =
            fetch_connect_player_states(&tx, &app_config, ConnectSurface::Remote7091).await;
        assert!(!states.is_empty());
        assert_eq!(states[0].mode, Mode::Stop);
    }
}

#[cfg(test)]
mod encode_tests {
    use super::*;
    use crate::player_state::{Mode, Playerid};

    #[test]
    fn encode_audio_event_matches_live_event_shape() {
        let state = PlayerState {
            playerid: Playerid(1),
            mode: Mode::Play,
            title: "Song".into(),
            ..Default::default()
        };
        let json = encode_audio_event_payload(&state).unwrap();
        assert!(json.contains("\"audio_event\""));
        assert!(json.contains("\"title\":\"Song\""));
    }
}
