//! Audioserver extension config (zone mappings, volume presets, room favorites).

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// One persisted room favorite for a Loxone player zone (`getroomfavs_result.items` row).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RoomFavItem {
    pub id: usize,
    pub slot: usize,
    pub plus: bool,
    pub name: String,
    pub title: String,
    pub audiopath: String,
    #[serde(rename = "type")]
    pub item_type: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub coverurl: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub artist: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub album: String,
    pub service: String,
    #[serde(rename = "serviceType")]
    pub service_type: u8,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub owner: String,
}

/// One Loxone player zone and its playback backends (speaker control, Google Cast radio).
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct PlayerMapping {
    pub player_id: usize,
    #[serde(default)]
    pub loxone_uuid: Option<String>,
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cast_device_id: Option<String>,
    /// Stable mDNS id of the speaker that plays this zone (TXT `uuid`, or AirPlay `deviceid`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker_id: Option<String>,
    /// Manual speaker address (IP or host) when mDNS is unavailable, e.g. Docker without host networking.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker_host: Option<String>,
    /// Speaker API for a manual `speaker_host` that mDNS cannot see:
    /// `kef_stream800`, `naim_stream800` or `kef_legacy`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume_level: Option<u8>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub volume_muted: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub room_favorites: Vec<RoomFavItem>,
    /// Loxone Config "Variable Default Volume" (`audio/cfg/volumes` sends `default: -1`).
    #[serde(default, skip_serializing_if = "is_false")]
    pub variable_default_volume: bool,
    /// Fixed startup default % from Loxone when `variable_default_volume` is false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loxone_default_percent: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loxone_alarm: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loxone_fire: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loxone_bell: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loxone_buzzer: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loxone_tts: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loxone_volstep: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loxone_max_volume: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loxone_fading: Option<u8>,
}

impl PlayerMapping {
    pub fn is_cast_configured(&self) -> bool {
        self.cast_device_id
            .as_ref()
            .is_some_and(|id| !id.trim().is_empty())
    }

    pub fn is_speaker_configured(&self) -> bool {
        non_blank(&self.speaker_id) || non_blank(&self.speaker_host)
    }

    pub fn is_playback_configured(&self) -> bool {
        self.is_cast_configured() || self.is_speaker_configured()
    }

    /// Restore device mapping fields from disk when in-memory row lost them before save.
    pub fn restore_preserved_fields_from(&mut self, disk: &PlayerMapping) -> bool {
        let mut restored = false;
        if self
            .cast_device_id
            .as_ref()
            .is_none_or(|id| id.trim().is_empty())
        {
            if let Some(id) = disk.cast_device_id.as_ref().filter(|id| !id.is_empty()) {
                self.cast_device_id = Some(id.clone());
                restored = true;
            }
        }
        for (mem, disk) in [
            (&mut self.speaker_id, &disk.speaker_id),
            (&mut self.speaker_host, &disk.speaker_host),
            (&mut self.speaker_kind, &disk.speaker_kind),
        ] {
            if !non_blank(mem) && non_blank(disk) {
                *mem = disk.clone();
                restored = true;
            }
        }
        restored
    }

    pub fn has_preserved_mapping_fields(&self) -> bool {
        self.cast_device_id
            .as_ref()
            .is_some_and(|id| !id.is_empty())
            || self.is_speaker_configured()
    }

    /// Apply Loxone Miniserver `audio/cfg/volumes` preset for this zone.
    pub fn apply_loxone_volume_wire(&mut self, wire: &crate::loxone_config::Volume) {
        self.variable_default_volume = wire.is_variable_default();
        self.loxone_default_percent = wire.fixed_default_percent();
        self.loxone_alarm = pct_field(wire.alarm);
        self.loxone_fire = pct_field(wire.fire);
        self.loxone_bell = pct_field(wire.bell);
        self.loxone_buzzer = pct_field(wire.buzzer);
        self.loxone_tts = pct_field(wire.tts);
        self.loxone_volstep = pct_field(wire.volstep);
        self.loxone_max_volume = pct_field(wire.max_volume);
        self.loxone_fading = pct_field(wire.fading);
    }
}

fn non_blank(value: &Option<String>) -> bool {
    value.as_deref().is_some_and(|v| !v.trim().is_empty())
}

fn pct_field(value: usize) -> Option<u8> {
    Some(value.min(100) as u8)
}

fn is_false(v: &bool) -> bool {
    !*v
}

/// `audioserver` section of [`CONFIG_FILENAME`](crate::config::CONFIG_FILENAME).
///
/// Legacy Qobuz keys (`email`, `user_auth_token`, `last_queue_item_id`, per-player `qobuz_*`) are
/// ignored on load and dropped on the next save.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct ZoneConfigBody {
    #[serde(default)]
    pub players: Vec<PlayerMapping>,
}

/// Audioserver extension section of the persisted config.
pub type ZoneConfig = ZoneConfigBody;

impl ZoneConfigBody {
    pub fn get() -> Self {
        crate::config::AudioserverConfigFile::get().audioserver
    }

    pub fn load() -> Result<Self> {
        Ok(Self::get())
    }

    pub fn update<F, R>(f: F) -> Result<R>
    where
        F: FnOnce(&mut Self) -> Result<R>,
    {
        crate::config::AudioserverConfigFile::update(|file| f(&mut file.audioserver))
    }

    pub fn ensure_player_stub(&mut self, player_id: usize) -> &mut PlayerMapping {
        if !self.players.iter().any(|p| p.player_id == player_id) {
            self.players.push(PlayerMapping {
                player_id,
                name: format!("Zone {player_id}"),
                ..Default::default()
            });
        }
        self.players
            .iter_mut()
            .find(|p| p.player_id == player_id)
            .expect("player row just inserted")
    }
}

#[cfg(test)]
impl ZoneConfigBody {
    pub fn reset_cache_for_test(config: ZoneConfigBody) {
        crate::config::AudioserverConfigFile::reset_cache_for_test(
            crate::config::AudioserverConfigFile {
                audioserver: config,
                ..Default::default()
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restore_preserved_fields_from_disk_row() {
        let disk = PlayerMapping {
            player_id: 3,
            loxone_uuid: Some("u".into()),
            name: "Bedroom".into(),
            cast_device_id: Some("cast-id".into()),
            volume_level: Some(40),
            ..Default::default()
        };
        let mut mem = PlayerMapping {
            player_id: 3,
            loxone_uuid: Some("u".into()),
            name: "Zone 3".into(),
            cast_device_id: None,
            volume_level: Some(80),
            ..Default::default()
        };
        assert!(mem.restore_preserved_fields_from(&disk));
        assert_eq!(mem.cast_device_id.as_deref(), Some("cast-id"));
        assert_eq!(mem.volume_level, Some(80));
        assert_eq!(mem.name, "Zone 3");
    }

    #[test]
    fn loads_legacy_qobuz_config_and_drops_qobuz_keys() {
        let json = r#"{
            "email": "a@b.com",
            "user_auth_token": "tok",
            "last_queue_item_id": 7,
            "players": [{
                "player_id": 2,
                "name": "Zone 2",
                "qobuz_device_uuid": "dev",
                "qobuz_renderer_id": 5,
                "cast_device_id": "cast"
            }]
        }"#;
        let config: ZoneConfigBody = serde_json::from_str(json).unwrap();
        assert_eq!(config.players[0].player_id, 2);
        assert!(config.players[0].loxone_uuid.is_none());
        assert_eq!(config.players[0].cast_device_id.as_deref(), Some("cast"));
        let saved = serde_json::to_string(&config).unwrap();
        assert!(!saved.contains("qobuz"));
        assert!(!saved.contains("user_auth_token"));
    }

    #[test]
    fn playback_capabilities_and_configured_flags() {
        let cast_only = PlayerMapping {
            cast_device_id: Some("abc123".into()),
            ..Default::default()
        };
        assert!(cast_only.is_cast_configured());
        assert!(cast_only.is_playback_configured());

        let none = PlayerMapping {
            cast_device_id: Some("   ".into()),
            ..Default::default()
        };
        assert!(!none.is_cast_configured());
        assert!(!none.is_playback_configured());
    }

    #[test]
    fn has_preserved_mapping_fields_detects_cast() {
        assert!(PlayerMapping {
            cast_device_id: Some("cast".into()),
            ..Default::default()
        }
        .has_preserved_mapping_fields());
        assert!(!PlayerMapping::default().has_preserved_mapping_fields());
    }

    #[test]
    fn restore_preserved_fields_whitespace_cast_and_no_op() {
        let disk = PlayerMapping {
            player_id: 1,
            cast_device_id: Some("real-cast".into()),
            ..Default::default()
        };
        let mut whitespace_cast = PlayerMapping {
            player_id: 1,
            cast_device_id: Some("  ".into()),
            ..Default::default()
        };
        assert!(whitespace_cast.restore_preserved_fields_from(&disk));
        assert_eq!(whitespace_cast.cast_device_id.as_deref(), Some("real-cast"));

        let mut unchanged = PlayerMapping {
            player_id: 1,
            cast_device_id: Some("keep-cast".into()),
            ..Default::default()
        };
        assert!(!unchanged.restore_preserved_fields_from(&disk));
    }

    #[test]
    fn speaker_fields_count_as_playback_and_are_restored() {
        let by_id = PlayerMapping {
            speaker_id: Some("ls60w-444ca1e2".into()),
            ..Default::default()
        };
        assert!(by_id.is_speaker_configured());
        assert!(by_id.is_playback_configured());
        assert!(by_id.has_preserved_mapping_fields());

        let by_host = PlayerMapping {
            speaker_host: Some("192.168.50.209".into()),
            ..Default::default()
        };
        assert!(by_host.is_speaker_configured());

        let blank = PlayerMapping {
            speaker_id: Some("  ".into()),
            ..Default::default()
        };
        assert!(!blank.is_speaker_configured());

        let disk = PlayerMapping {
            player_id: 2,
            speaker_id: Some("naims810x-c2376dfe".into()),
            speaker_host: Some("qb.local".into()),
            ..Default::default()
        };
        let mut mem = PlayerMapping {
            player_id: 2,
            speaker_id: Some(" ".into()),
            ..Default::default()
        };
        assert!(mem.restore_preserved_fields_from(&disk));
        assert_eq!(mem.speaker_id.as_deref(), Some("naims810x-c2376dfe"));
        assert_eq!(mem.speaker_host.as_deref(), Some("qb.local"));
        assert!(!mem.restore_preserved_fields_from(&disk));
    }

    #[test]
    fn apply_loxone_volume_wire_and_ensure_player_stub() {
        use crate::loxone_config::Volume;

        let mut player = PlayerMapping {
            player_id: 7,
            ..Default::default()
        };
        player.apply_loxone_volume_wire(&Volume {
            playerid: 7,
            default: -1,
            alarm: 75,
            fire: 80,
            bell: 35,
            buzzer: 10,
            tts: 20,
            volstep: 2,
            fading: 150,
            max_volume: 120,
        });
        assert!(player.variable_default_volume);
        assert_eq!(player.loxone_default_percent, None);
        assert_eq!(player.loxone_alarm, Some(75));
        assert_eq!(player.loxone_fading, Some(100));
        assert_eq!(player.loxone_max_volume, Some(100));

        let mut config = ZoneConfigBody::default();
        {
            let stub = config.ensure_player_stub(9);
            assert_eq!(stub.player_id, 9);
            assert_eq!(stub.name, "Zone 9");
        }
        assert_eq!(config.players.len(), 1);
        config.ensure_player_stub(9).name = "Living room".into();
        assert_eq!(config.players.len(), 1);
        assert_eq!(config.players[0].name, "Living room");
    }

    #[test]
    fn room_fav_item_omits_empty_optional_fields() {
        let item = RoomFavItem {
            id: 1,
            slot: 0,
            plus: false,
            name: "Fav".into(),
            title: "Track".into(),
            audiopath: "spotify:track:1".into(),
            item_type: "track".into(),
            coverurl: String::new(),
            artist: String::new(),
            album: String::new(),
            service: "spotify".into(),
            service_type: 3,
            owner: String::new(),
        };
        let json = serde_json::to_value(&item).unwrap();
        assert!(json.get("coverurl").is_none());
        assert!(json.get("artist").is_none());
        assert!(json.get("album").is_none());
        assert!(json.get("owner").is_none());
    }
}
