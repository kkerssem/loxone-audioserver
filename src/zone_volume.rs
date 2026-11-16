//! Persisted per-zone volume in audioserver-config.json (`players[].volume_level` / `volume_muted`).

use crate::config::{PlayerMapping, ZoneConfig};
use crate::loxone_config::Volume;
use crate::player_state::Playerid;
use tracing::info;

pub const DEFAULT_FALLBACK_LEVEL: u8 = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZoneVolume {
    pub level: u8,
    pub muted: bool,
}

pub fn init() {
    info!("zone volume config loaded");
}

pub fn get(playerid: Playerid) -> Option<ZoneVolume> {
    let config = ZoneConfig::get();
    config
        .players
        .iter()
        .find(|p| p.player_id == playerid.0)
        .and_then(player_volume)
}

pub fn display_volume(playerid: Playerid) -> f32 {
    let Some(vol) = get(playerid) else {
        return DEFAULT_FALLBACK_LEVEL as f32;
    };
    if vol.muted {
        0.0
    } else {
        vol.level as f32
    }
}

/// Volume to apply when starting playback from stop (Loxone Variable Default Volume aware).
pub fn startup_volume(playerid: Playerid) -> u8 {
    let config = ZoneConfig::get();
    let Some(player) = config.players.iter().find(|p| p.player_id == playerid.0) else {
        return DEFAULT_FALLBACK_LEVEL;
    };
    if player.variable_default_volume {
        player.volume_level.unwrap_or(DEFAULT_FALLBACK_LEVEL)
    } else {
        let raw = player
            .loxone_default_percent
            .unwrap_or(DEFAULT_FALLBACK_LEVEL);
        clamp_volume_to_preset(raw, player)
    }
}

pub fn clamp_volume_to_preset(raw: u8, player: &PlayerMapping) -> u8 {
    let max = player.loxone_max_volume.unwrap_or(100).clamp(1, 100);
    let stepped = if let Some(step) = player.loxone_volstep.filter(|&s| s > 0) {
        ((f32::from(raw) / f32::from(step)).round() * f32::from(step)) as i32
    } else {
        i32::from(raw)
    };
    stepped.clamp(0, i32::from(max)) as u8
}

pub fn set_level(playerid: Playerid, level: u8) -> anyhow::Result<()> {
    let level = level.min(100);
    ZoneConfig::update(|config| {
        let player = config.ensure_player_stub(playerid.0);
        player.volume_level = Some(level);
        player.volume_muted = false;
        Ok(())
    })
}

#[allow(dead_code)]
pub fn set_muted(playerid: Playerid, muted: bool) -> anyhow::Result<()> {
    ZoneConfig::update(|config| {
        let player = config.ensure_player_stub(playerid.0);
        player.volume_muted = muted;
        Ok(())
    })
}

pub fn set(playerid: Playerid, level: u8, muted: bool) -> anyhow::Result<()> {
    let level = level.min(100);
    ZoneConfig::update(|config| {
        let player = config.ensure_player_stub(playerid.0);
        player.volume_level = Some(level);
        player.volume_muted = muted;
        Ok(())
    })
}

/// Persist Loxone `audio/cfg/volumes` policy and seed fixed defaults when appropriate.
pub fn apply_loxone_volume_presets(vols: &[Volume]) -> anyhow::Result<()> {
    if vols.is_empty() {
        return Ok(());
    }
    ZoneConfig::update(|config| {
        for wire in vols {
            let player = config.ensure_player_stub(wire.playerid);
            player.apply_loxone_volume_wire(wire);
            if wire.is_variable_default() {
                continue;
            }
            if player.volume_level.is_none() {
                if let Some(default) = wire.fixed_default_percent() {
                    player.volume_level = Some(clamp_volume_to_preset(default, player));
                    player.volume_muted = false;
                }
            }
        }
        Ok(())
    })
}

fn player_volume(player: &PlayerMapping) -> Option<ZoneVolume> {
    player.volume_level.map(|level| ZoneVolume {
        level,
        muted: player.volume_muted,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AudioserverConfigFile;
    use crate::config::{PlayerMapping, ZoneConfig};
    use crate::loxone_config::Volume;

    fn with_config(config: ZoneConfig, f: impl FnOnce()) {
        AudioserverConfigFile::with_test_cache(
            AudioserverConfigFile {
                audioserver: config,
                ..Default::default()
            },
            f,
        );
    }

    fn sample_wire(default: i32) -> Volume {
        Volume {
            playerid: 3,
            default,
            alarm: 75,
            fire: 75,
            bell: 35,
            buzzer: 10,
            tts: 20,
            volstep: 2,
            fading: 0,
            max_volume: 80,
        }
    }

    #[test]
    fn display_volume_fallback_when_unset() {
        with_config(ZoneConfig::default(), || {
            assert_eq!(display_volume(Playerid(9)), 50.0);
        });
    }

    #[test]
    fn display_volume_zero_when_muted() {
        with_config(
            ZoneConfig {
                players: vec![PlayerMapping {
                    player_id: 1,
                    name: "A".into(),
                    volume_level: Some(40),
                    volume_muted: true,
                    ..Default::default()
                }],
            },
            || {
                assert_eq!(display_volume(Playerid(1)), 0.0);
                assert_eq!(get(Playerid(1)).unwrap().level, 40);
            },
        );
    }

    #[test]
    fn set_level_creates_stub_player_row() {
        with_config(ZoneConfig::default(), || {
            set_level(Playerid(4), 34).unwrap();
            let config = ZoneConfig::get();
            assert_eq!(config.players.len(), 1);
            assert_eq!(config.players[0].player_id, 4);
            assert_eq!(config.players[0].volume_level, Some(34));
            assert!(!config.players[0].volume_muted);
            assert_eq!(display_volume(Playerid(4)), 34.0);
        });
    }

    #[test]
    fn fixed_default_preset_does_not_overwrite_existing() {
        with_config(
            ZoneConfig {
                players: vec![PlayerMapping {
                    player_id: 1,
                    name: "A".into(),
                    volume_level: Some(34),
                    ..Default::default()
                }],
            },
            || {
                apply_loxone_volume_presets(&[
                    Volume {
                        playerid: 1,
                        default: 80,
                        ..sample_wire(80)
                    },
                    Volume {
                        playerid: 2,
                        default: 25,
                        ..sample_wire(25)
                    },
                ])
                .unwrap();
                let config = ZoneConfig::get();
                assert_eq!(config.players[0].volume_level, Some(34));
                assert_eq!(config.players[1].player_id, 2);
                assert_eq!(config.players[1].volume_level, Some(26));
            },
        );
    }

    #[test]
    fn set_level_clears_muted() {
        with_config(
            ZoneConfig {
                players: vec![PlayerMapping {
                    player_id: 1,
                    name: "A".into(),
                    volume_level: Some(10),
                    ..Default::default()
                }],
            },
            || {
                set_level(Playerid(1), 22).unwrap();
                assert_eq!(display_volume(Playerid(1)), 22.0);
            },
        );
    }

    #[test]
    fn variable_default_preset_preserves_existing_volume_level() {
        with_config(
            ZoneConfig {
                players: vec![PlayerMapping {
                    player_id: 3,
                    name: "Zone 3".into(),
                    volume_level: Some(40),
                    ..Default::default()
                }],
            },
            || {
                apply_loxone_volume_presets(&[sample_wire(-1)]).unwrap();
                let player = &ZoneConfig::get().players[0];
                assert!(player.variable_default_volume);
                assert_eq!(player.volume_level, Some(40));
                assert_eq!(startup_volume(Playerid(3)), 40);
            },
        );
    }

    #[test]
    fn fixed_default_preset_seeds_when_unset() {
        with_config(ZoneConfig::default(), || {
            apply_loxone_volume_presets(&[sample_wire(25)]).unwrap();
            let player = &ZoneConfig::get().players[0];
            assert!(!player.variable_default_volume);
            assert_eq!(player.loxone_default_percent, Some(25));
            assert_eq!(player.volume_level, Some(26));
            assert_eq!(startup_volume(Playerid(3)), 26);
        });
    }

    #[test]
    fn startup_volume_uses_last_level_in_variable_mode() {
        with_config(
            ZoneConfig {
                players: vec![PlayerMapping {
                    player_id: 3,
                    name: "Z".into(),
                    volume_level: Some(37),
                    variable_default_volume: true,
                    ..Default::default()
                }],
            },
            || {
                assert_eq!(startup_volume(Playerid(3)), 37);
            },
        );
    }

    #[test]
    fn clamp_respects_max_volume_and_volstep() {
        let player = PlayerMapping {
            player_id: 1,
            name: "Z".into(),
            loxone_volstep: Some(5),
            loxone_max_volume: Some(70),
            ..Default::default()
        };
        assert_eq!(clamp_volume_to_preset(67, &player), 65);
        assert_eq!(clamp_volume_to_preset(90, &player), 70);
    }
}
