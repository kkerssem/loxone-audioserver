//! Loxone Audioserver player status (shared by the 7091 remote and 7095 Miniserver APIs).

use std::sync::{OnceLock, RwLock};

use serde::{Serialize, Serializer};
use serde_json::Value as JsonValue;

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Mode {
    /// Loxone “buffering” mode (used when Miniserver pushes live playback state).
    #[allow(dead_code)]
    Buffer,
    Play,
    Pause,
    #[default]
    Stop,
}

impl Serialize for Mode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let s = match self {
            Mode::Buffer => "buffer",
            Mode::Play => "play",
            Mode::Pause => "pause",
            Mode::Stop => "stop",
        };
        serializer.serialize_str(s)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Player {
    pub playerid: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct Playerid(pub usize);

impl std::str::FromStr for Playerid {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse::<usize>().map(Playerid).map_err(|e| e.into())
    }
}

/// Playback status exposed to the Loxone UI (`audio/.../status` and `audio_event`).
///
/// Shape aligned with lox-audioserver [`LoxoneZoneState`](https://github.com/lox-audioserver/lox-audioserver).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct PlayerState {
    pub album: String,
    pub artist: String,
    pub audiopath: String,
    pub audiotype: u8,
    #[serde(rename = "clientState", default = "default_client_state")]
    pub client_state: String,
    #[serde(rename = "coverurl", default)]
    pub cover_url: String,
    /// Not on the wire: added to the proxied `coverurl` so the App reloads a cover whose
    /// source URL is unchanged (a restarted radio stream). Empty when unknown.
    #[serde(skip)]
    pub cover_revision: String,
    #[serde(default)]
    pub duration: u32,
    #[serde(rename = "equalizerSettings", default = "default_equalizer_settings")]
    pub equalizer_settings: String,
    pub mode: Mode,
    #[serde(default)]
    pub name: String,
    #[serde(rename = "parent", default = "default_parent")]
    pub parent: JsonValue,
    pub plrepeat: u8,
    pub plshuffle: u8,
    pub playerid: Playerid,
    pub power: String,
    #[serde(rename = "powerState", skip_serializing_if = "Option::is_none")]
    pub power_state: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub players: Vec<Player>,
    #[serde(default)]
    pub qindex: u32,
    /// V2 `QueueScreenTemplate` `currentQId` — queue row `unique_id` for the playing item.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qid: Option<String>,
    #[serde(rename = "queueAuthority", default = "default_queue_authority")]
    pub queue_authority: String,
    #[serde(rename = "sourceName", default)]
    pub source_name: String,
    #[serde(default)]
    pub station: String,
    #[serde(default)]
    pub time: i64,
    pub title: String,
    #[serde(rename = "type", default = "default_zone_type")]
    pub zone_type: u8,
    pub volume: f32,
    #[serde(rename = "syncedzones", default)]
    pub syncedzones: Vec<usize>,
    #[serde(rename = "mastervolume", default)]
    pub mastervolume: f32,
}

#[allow(dead_code)]
fn default_client_state() -> String {
    "on".to_string()
}

#[allow(dead_code)]
fn default_zone_type() -> u8 {
    3
}

fn default_equalizer_settings() -> String {
    "0,0,0,0,0,0,0,0,0,0".to_string()
}

fn default_queue_authority() -> String {
    "local".to_string()
}

fn default_parent() -> JsonValue {
    JsonValue::Null
}

/// Bare uppercase hex serial (no colons), as used in reference `sourceName`.
pub fn bare_audioserver_serial(serial: &str) -> String {
    serial
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .collect::<String>()
        .to_uppercase()
}

impl PlayerState {
    /// Idle/stopped state for a zone (used before a backend reports playback).
    pub fn idle(playerid: Playerid) -> Self {
        Self {
            playerid,
            players: vec![Player {
                playerid: playerid.0,
            }],
            mode: Mode::Stop,
            power: "on".to_string(),
            client_state: "on".to_string(),
            zone_type: 3,
            power_state: Some("off".to_string()),
            equalizer_settings: default_equalizer_settings(),
            queue_authority: default_queue_authority(),
            parent: default_parent(),
            ..Default::default()
        }
    }

    /// Reference-shaped envelope fields from [trace.txt](reference/loxone-ui/trace.txt) idle `audio_event`.
    pub fn apply_reference_envelope(&mut self, audioserver_serial: &str) {
        self.source_name = bare_audioserver_serial(audioserver_serial);
        self.queue_authority = default_queue_authority();
        self.parent = default_parent();
        self.equalizer_settings = default_equalizer_settings();
        if self.cover_url.is_empty() {
            self.cover_url = String::new();
        }
        if self.station.is_empty() {
            self.station = String::new();
        }
    }

    /// Copy title/artist/album/cover from a prior snapshot when a fresh build lost them.
    ///
    /// Qobuz sometimes clears `current_queue_item_id` before `playing_state` reaches paused;
    /// without this, Loxone briefly shows "no song selected".
    pub fn merge_playback_metadata(&mut self, prior: &PlayerState) {
        if prior.title.is_empty() {
            return;
        }
        if self.title.is_empty() {
            self.title = prior.title.clone();
            self.artist = prior.artist.clone();
            self.album = prior.album.clone();
            self.cover_url = prior.cover_url.clone();
            self.audiopath = prior.audiopath.clone();
            if prior.audiotype != 0 {
                self.audiotype = prior.audiotype;
            }
            if prior.duration > 0 {
                self.duration = prior.duration;
            }
        }
        if matches!(self.mode, Mode::Stop) && matches!(prior.mode, Mode::Play | Mode::Pause) {
            self.mode = prior.mode.clone();
        }
        if self.power_state.is_some() && prior.power_state.is_none() {
            self.power_state = None;
        }
        if prior.volume > 0.0
            && (self.volume == 0.0
                || self.volume == loxone_default_volume(self.playerid).unwrap_or(0.0))
        {
            self.volume = prior.volume;
        }
    }

    /// Fill protocol defaults (players, power, volume from Miniserver cfg).
    pub fn apply_protocol_defaults(
        &mut self,
        zone_name: Option<&str>,
        loxone_default_volume: Option<f32>,
    ) {
        let id = self.playerid.0;
        if self.players.is_empty() {
            self.players = vec![Player { playerid: id }];
        }
        if self.name.is_empty() {
            if let Some(name) = zone_name.filter(|n| !n.is_empty()) {
                self.name = name.to_string();
            }
        }
        if self.power.is_empty() {
            self.power = "on".to_string();
        }
        if self.client_state.is_empty() {
            self.client_state = "on".to_string();
        }
        if self.zone_type == 0 {
            self.zone_type = 3;
        }
        if self.volume == 0.0 && matches!(self.mode, Mode::Stop) {
            if let Some(vol) = loxone_default_volume {
                self.volume = vol;
            }
        }
    }
}

static RUNTIME_VOLUMES: OnceLock<RwLock<Vec<(usize, f32)>>> = OnceLock::new();

fn runtime_volumes() -> &'static RwLock<Vec<(usize, f32)>> {
    RUNTIME_VOLUMES.get_or_init(|| RwLock::new(Vec::new()))
}

/// Updated when the Miniserver sends `audio/cfg/volumes` during pairing/runtime.
#[cfg(feature = "miniserver")]
pub fn set_loxone_runtime_volumes(volumes: Vec<(usize, f32)>) {
    if let Ok(mut guard) = runtime_volumes().write() {
        *guard = volumes;
    }
}

/// Default zone volume for Loxone UI shells (persisted in zone-config.json when built with miniserver).
pub fn loxone_default_volume(playerid: Playerid) -> Option<f32> {
    #[cfg(feature = "miniserver")]
    {
        Some(crate::zone_volume::display_volume(playerid))
    }
    #[cfg(not(feature = "miniserver"))]
    {
        runtime_volumes()
            .read()
            .ok()?
            .iter()
            .find(|(id, _)| *id == playerid.0)
            .map(|(_, vol)| *vol)
    }
}

/// Apply protocol defaults (players, name, volume) after building playback state.
pub fn enrich_for_loxone_ui(
    state: &mut PlayerState,
    zone_name: Option<&str>,
    audioserver_serial: Option<&str>,
) {
    state.apply_protocol_defaults(zone_name, loxone_default_volume(state.playerid));
    if let Some(serial) = audioserver_serial {
        state.apply_reference_envelope(serial);
    }
}

/// Idle zone state for `audio/{id}/status` and `getplayersdetails`.
pub fn player_state_for_protocol(
    playerid: Playerid,
    zone_name: Option<&str>,
    audioserver_serial: Option<&str>,
) -> PlayerState {
    let mut state = PlayerState::idle(playerid);
    state.apply_protocol_defaults(zone_name, loxone_default_volume(playerid));
    if let Some(serial) = audioserver_serial {
        state.apply_reference_envelope(serial);
    }
    state
}

/// Idle state pushed immediately after a 7091 WebSocket connect (no `players` in JSON).
#[cfg(feature = "miniserver")]
pub fn connected_idle_state(
    playerid: Playerid,
    zone_name: Option<&str>,
    audioserver_serial: &str,
) -> PlayerState {
    let mut state = player_state_for_protocol(playerid, zone_name, Some(audioserver_serial));
    state.players.clear();
    state
}

/// Idle zone state for 7095 Miniserver `audio_event` ([trace-7095.txt](reference/loxone-ui/trace-7095.txt)).
#[cfg(feature = "miniserver")]
pub fn miniserver_idle_state(
    playerid: Playerid,
    zone_name: Option<&str>,
    audioserver_serial: &str,
    default_volume: Option<f32>,
) -> PlayerState {
    let mut state = connected_idle_state(playerid, zone_name, audioserver_serial);
    if let Some(vol) = default_volume {
        state.volume = vol;
    }
    state.syncedzones = Vec::new();
    state.mastervolume = 0.0;
    state
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn apply_protocol_defaults_fills_shell_fields() {
        let mut state = PlayerState::default();
        state.playerid = Playerid(1);
        state.apply_protocol_defaults(Some("Utility Room"), Some(10.0));
        assert_eq!(state.players, vec![Player { playerid: 1 }]);
        assert_eq!(state.name, "Utility Room");
        assert_eq!(state.power, "on");
        assert_eq!(state.volume, 10.0);
        assert_eq!(state.client_state, "on");
        assert_eq!(state.zone_type, 3);
    }

    #[cfg(feature = "miniserver")]
    #[test]
    fn runtime_volumes_used_for_default_volume() {
        use crate::config::{AudioserverConfigFile, PlayerMapping, ZoneConfig};
        let audioserver = ZoneConfig {
            players: vec![PlayerMapping {
                player_id: 2,
                name: "B".into(),
                volume_level: Some(25),
                ..Default::default()
            }],
        };
        AudioserverConfigFile::with_test_cache(
            AudioserverConfigFile {
                audioserver,
                ..Default::default()
            },
            || assert_eq!(loxone_default_volume(Playerid(2)), Some(25.0)),
        );
    }

    #[cfg(feature = "miniserver")]
    #[test]
    fn player_state_for_protocol_idle_shape() {
        use crate::config::{AudioserverConfigFile, PlayerMapping, ZoneConfig};
        let audioserver = ZoneConfig {
            players: vec![PlayerMapping {
                player_id: 1,
                name: "A".into(),
                volume_level: Some(10),
                ..Default::default()
            }],
        };
        let state = AudioserverConfigFile::with_test_cache(
            AudioserverConfigFile {
                audioserver,
                ..Default::default()
            },
            || player_state_for_protocol(Playerid(1), Some("Utility Room"), Some("504f94ff1bb3")),
        );
        assert_eq!(state.name, "Utility Room");
        assert_eq!(state.power, "on");
        assert_eq!(state.volume, 10.0);
        assert!(matches!(state.mode, Mode::Stop));

        let value = serde_json::to_value(&state).unwrap();
        assert_eq!(value.get("clientState"), Some(&json!("on")));
        assert_eq!(value.get("type"), Some(&json!(3)));
        assert_eq!(value.get("mode"), Some(&json!("stop")));
        assert_eq!(value.get("power"), Some(&json!("on")));
        assert_eq!(value.get("queueAuthority"), Some(&json!("local")));
        assert_eq!(value.get("parent"), Some(&json!(null)));
        assert_eq!(value.get("sourceName"), Some(&json!("504F94FF1BB3")));
        assert_eq!(value.get("coverurl"), Some(&json!("")));
        assert_eq!(value.get("station"), Some(&json!("")));
    }

    #[cfg(feature = "miniserver")]
    #[test]
    fn connected_idle_omits_players() {
        let state = connected_idle_state(Playerid(1), Some("Room"), "504F94FF1BB3");
        let value = serde_json::to_value(&state).unwrap();
        assert!(value.get("players").is_none());
    }

    #[cfg(feature = "miniserver")]
    #[test]
    fn miniserver_idle_includes_sync_fields() {
        let state = miniserver_idle_state(
            Playerid(1),
            Some("Utility Room"),
            "504F94FF1BB3",
            Some(30.0),
        );
        let value = serde_json::to_value(&state).unwrap();
        assert!(value.get("players").is_none());
        assert_eq!(value.get("syncedzones"), Some(&json!([])));
        assert_eq!(value.get("mastervolume"), Some(&json!(0.0)));
        assert_eq!(value.get("volume"), Some(&json!(30.0)));
    }

    #[test]
    fn playing_state_includes_client_state_and_type() {
        let state = PlayerState {
            audiotype: 2,
            mode: Mode::Play,
            playerid: Playerid(1),
            power: "on".to_string(),
            client_state: "on".to_string(),
            zone_type: 3,
            title: "Track".into(),
            ..Default::default()
        };
        let value = serde_json::to_value(&state).unwrap();
        assert_eq!(value.get("clientState"), Some(&json!("on")));
        assert_eq!(value.get("type"), Some(&json!(3)));
        assert_eq!(value.get("mode"), Some(&json!("play")));
    }

    #[test]
    fn merge_playback_metadata_preserves_title_on_transient_clear() {
        let prior = PlayerState {
            title: "Helicopters".into(),
            artist: "Ezra Collective".into(),
            album: "HELP(2)".into(),
            audiotype: 2,
            mode: Mode::Pause,
            duration: 238,
            time: 13,
            ..Default::default()
        };
        let mut fresh = PlayerState {
            mode: Mode::Stop,
            power_state: Some("off".into()),
            ..Default::default()
        };
        fresh.merge_playback_metadata(&prior);
        assert_eq!(fresh.title, "Helicopters");
        assert_eq!(fresh.mode, Mode::Pause);
        assert!(fresh.power_state.is_none());
    }
}
