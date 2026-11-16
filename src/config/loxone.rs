//! Loxone Miniserver configuration payloads (`setconfig`, `playername`, etc.).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub type SerialWithConfig = [HashMap<String, AudioserverConfig>; 1];

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioserverConfig {
    pub master: String,
    pub ip: String,
    pub port: usize,
    pub language: String,
    pub blinkpos: usize,
    #[serde(rename = "autoUpdate")]
    pub auto_update: usize,
    pub unit_temperature: String,
    pub name: String,
    pub errortts: bool,
    pub uuid: String,
    #[serde(rename = "pwHash")]
    pub pw_hash: String,
    pub players: Vec<Player>,
    pub extensions: Vec<StereoExtension>,
    pub groups: Vec<Group>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Group {
    pub uuid: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Channel {
    pub id: String,
    #[serde(rename = "type")]
    pub channel_type: String,
    pub maxvol: usize,
    #[serde(rename = "offsetVol")]
    pub offset_vol: usize,
    pub volmode: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Output {
    pub channels: Vec<Channel>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Player {
    pub uuid: String,
    pub playerid: usize,
    #[serde(rename = "speakerConfig")]
    pub speaker_config: usize,
    pub bt: String,
    pub outputs: Vec<Output>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PlayerName {
    pub playerid: usize,
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PlayerNames {
    pub players: Vec<PlayerName>,
}

#[derive(Deserialize)]
pub struct PlayerOpts {
    pub players: Vec<PlayerOpt>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PlayerOpt {
    pub playerid: usize,
    pub rf: String,
    pub lock: String,
    pub ap: String,
    pub sp: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeakerType {
    pub id: String,
    pub speaker_type: usize,
}

#[derive(Deserialize)]
pub struct SpeakerTypes {
    pub speakers: Vec<SpeakerType>,
}

#[derive(Deserialize)]
pub struct Volumes {
    pub players: Vec<Volume>,
}

#[derive(Deserialize)]
pub struct GroupOpts {
    pub groups: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Volume {
    pub playerid: usize,
    /// Fixed default %, or `-1` when Variable Default Volume is enabled in Loxone Config.
    pub default: i32,
    pub alarm: usize,
    pub fire: usize,
    pub bell: usize,
    pub buzzer: usize,
    pub tts: usize,
    pub volstep: usize,
    #[serde(default)]
    pub fading: usize,
    #[serde(rename = "maxVolume", default = "default_max_volume")]
    pub max_volume: usize,
}

fn default_max_volume() -> usize {
    100
}

impl Volume {
    pub fn is_variable_default(&self) -> bool {
        self.default == -1
    }

    pub fn fixed_default_percent(&self) -> Option<u8> {
        (self.default >= 0).then(|| self.default.clamp(0, 100) as u8)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StereoExtension {
    pub blinkpos: usize,
    pub btenable: bool,
    pub mac: String,
    pub serial: String,
    pub subtype: usize,
    #[serde(rename = "type")]
    pub extension_type: usize,
    pub version: String,
}

/// Aggregated runtime state built during Miniserver handshake (before `ready`).
#[derive(Clone, Debug, Default, Serialize)]
pub struct LoxoneRuntimeState {
    pub config: Option<AudioserverConfig>,
    pub player_names: Vec<PlayerName>,
    pub player_opts: Vec<PlayerOpt>,
    pub speaker_types: Vec<SpeakerType>,
    pub volumes: Vec<Volume>,
}

/// One configured audioplayer from Loxone `setconfig` (`players[]`).
#[derive(Clone, Debug)]
pub struct ZoneDescriptor {
    pub player_id: usize,
    pub uuid: String,
    pub name: String,
}

pub fn audioserver_config<'a>(
    serial_with_config: &'a SerialWithConfig,
    mac_id: &str,
) -> Option<&'a AudioserverConfig> {
    let map = serial_with_config.first()?;
    let key = map.keys().find(|k| k.eq_ignore_ascii_case(mac_id))?;
    map.get(key)
}

pub fn extract_channel_ids(config: &AudioserverConfig) -> Vec<String> {
    let mut ids = Vec::new();
    for player in &config.players {
        for output in &player.outputs {
            for ch in &output.channels {
                if !ch.id.is_empty() && !ids.contains(&ch.id) {
                    ids.push(ch.id.clone());
                }
            }
        }
    }
    ids
}

pub fn extract_zones(serial_with_config: &SerialWithConfig, mac_id: &str) -> Vec<ZoneDescriptor> {
    let Some(config) = audioserver_config(serial_with_config, mac_id) else {
        return Vec::new();
    };

    config
        .players
        .iter()
        .map(|player| ZoneDescriptor {
            player_id: player.playerid,
            uuid: player.uuid.clone(),
            name: format!("Zone {}", player.playerid),
        })
        .collect()
}

pub fn apply_player_names(zones: &mut [ZoneDescriptor], names: &[PlayerName]) {
    for zone in zones {
        if let Some(pn) = names.iter().find(|p| p.playerid == zone.player_id) {
            zone.name = pn.name.clone();
        }
    }
}

/// Parse persisted `loxone.state` JSON into `SerialWithConfig` when non-empty.
pub fn parse_loxone_state(state: &serde_json::Value) -> Option<SerialWithConfig> {
    let arr = state.as_array()?;
    if arr.is_empty() {
        return None;
    }
    serde_json::from_value(state.clone()).ok()
}

#[cfg(test)]
mod zone_tests {
    use super::*;

    #[test]
    fn extract_zones_includes_player_uuid() {
        let cfg: SerialWithConfig = serde_json::from_str(
            r#"[{"504F94FF1BB3":{"master":"504F94000002","ip":"x","port":80,"language":"ENU","blinkpos":0,"autoUpdate":120,"unitTemperature":"C","name":"AS","errortts":false,"uuid":"srv","pwHash":"","players":[{"uuid":"zone-uuid-2","playerid":2,"speakerConfig":4,"bt":"on","outputs":[]}],"extensions":[],"groups":[]}}]"#,
        )
        .unwrap();
        let zones = extract_zones(&cfg, "504F94FF1BB3");
        assert_eq!(zones.len(), 1);
        assert_eq!(zones[0].player_id, 2);
        assert_eq!(zones[0].uuid, "zone-uuid-2");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_channel_ids_from_player_outputs() {
        let cfg = AudioserverConfig {
            players: vec![Player {
                uuid: "u1".into(),
                playerid: 3,
                speaker_config: 4,
                bt: "on".into(),
                outputs: vec![Output {
                    channels: vec![Channel {
                        id: "504F94FF1BB3#1".into(),
                        channel_type: "stereo".into(),
                        maxvol: 100,
                        offset_vol: 0,
                        volmode: "normal".into(),
                    }],
                }],
            }],
            ..Default::default()
        };
        let ids = extract_channel_ids(&cfg);
        assert_eq!(ids, vec!["504F94FF1BB3#1"]);
    }

    #[test]
    fn volume_deserializes_variable_default_sentinel() {
        let json = r#"{
            "playerid": 3,
            "default": -1,
            "alarm": 75,
            "fire": 75,
            "bell": 35,
            "buzzer": 10,
            "tts": 20,
            "volstep": 1,
            "fading": 0,
            "maxVolume": 100
        }"#;
        let vol: Volume = serde_json::from_str(json).unwrap();
        assert!(vol.is_variable_default());
        assert_eq!(vol.fixed_default_percent(), None);
        assert_eq!(vol.max_volume, 100);
    }

    #[test]
    fn volume_deserializes_fixed_default_percent() {
        let json = r#"{
            "playerid": 3,
            "default": 25,
            "alarm": 75,
            "fire": 75,
            "bell": 35,
            "buzzer": 10,
            "tts": 20,
            "volstep": 1,
            "fading": 0,
            "maxVolume": 100
        }"#;
        let vol: Volume = serde_json::from_str(json).unwrap();
        assert!(!vol.is_variable_default());
        assert_eq!(vol.fixed_default_percent(), Some(25));
    }
}
