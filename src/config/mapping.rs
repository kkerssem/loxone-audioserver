//! Loxone zone ↔ persisted player row sync and mapping validation.

use crate::loxone_config::ZoneDescriptor;
use tracing::warn;

use super::zone::{PlayerMapping, ZoneConfig};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoxoneZoneSnapshot {
    pub player_id: usize,
    pub uuid: String,
    pub name: String,
}

impl From<&ZoneDescriptor> for LoxoneZoneSnapshot {
    fn from(z: &ZoneDescriptor) -> Self {
        Self {
            player_id: z.player_id,
            uuid: z.uuid.clone(),
            name: z.name.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MappingIssue {
    UuidPlayerIdMismatch {
        player_id: usize,
        stored_uuid: String,
        current_uuid: String,
    },
    UnknownPlayerId {
        player_id: usize,
    },
    MissingLoxoneUuid {
        player_id: usize,
    },
    DuplicateCastDeviceId {
        player_id: usize,
        cast_device_id: String,
    },
}

impl MappingIssue {
    #[cfg(test)]
    pub fn code(&self) -> &'static str {
        match self {
            Self::UuidPlayerIdMismatch { .. } => "uuid_playerid_mismatch",
            Self::UnknownPlayerId { .. } => "unknown_player_id",
            Self::MissingLoxoneUuid { .. } => "missing_loxone_uuid",
            Self::DuplicateCastDeviceId { .. } => "duplicate_cast_device_id",
        }
    }

    #[cfg(test)]
    pub fn player_id(&self) -> usize {
        match self {
            Self::UuidPlayerIdMismatch { player_id, .. }
            | Self::UnknownPlayerId { player_id }
            | Self::MissingLoxoneUuid { player_id }
            | Self::DuplicateCastDeviceId { player_id, .. } => *player_id,
        }
    }
}

/// Merge Loxone zone snapshots into config player rows; returns true if config changed.
pub fn apply_loxone_snapshots(config: &mut ZoneConfig, zones: &[LoxoneZoneSnapshot]) -> bool {
    let mut changed = false;
    for zone in zones {
        if let Some(existing) = config
            .players
            .iter_mut()
            .find(|p| p.player_id == zone.player_id)
        {
            if existing.name != zone.name {
                existing.name = zone.name.clone();
                changed = true;
            }
            if existing.loxone_uuid.as_deref() != Some(zone.uuid.as_str()) {
                existing.loxone_uuid = Some(zone.uuid.clone());
                changed = true;
            }
        } else {
            config.players.push(PlayerMapping {
                player_id: zone.player_id,
                loxone_uuid: Some(zone.uuid.clone()),
                name: zone.name.clone(),
                ..Default::default()
            });
            changed = true;
        }
    }
    changed
}

/// Retain only player rows whose `player_id` still exists in Loxone Config.
pub fn prune_players_not_in_loxone_zones(
    config: &mut ZoneConfig,
    loxone_zones: &[ZoneDescriptor],
) -> bool {
    let live: std::collections::HashSet<usize> = loxone_zones.iter().map(|z| z.player_id).collect();
    let before = config.players.len();
    config.players.retain(|p| live.contains(&p.player_id));
    config.players.len() != before
}

pub fn validate_mappings(config: &ZoneConfig, zones: &[LoxoneZoneSnapshot]) -> Vec<MappingIssue> {
    let mut issues = Vec::new();
    let zone_by_id: std::collections::HashMap<usize, &LoxoneZoneSnapshot> =
        zones.iter().map(|z| (z.player_id, z)).collect();

    for player in &config.players {
        match zone_by_id.get(&player.player_id) {
            None => issues.push(MappingIssue::UnknownPlayerId {
                player_id: player.player_id,
            }),
            Some(zone) => {
                if let Some(stored) = player.loxone_uuid.as_deref() {
                    if !stored.eq_ignore_ascii_case(&zone.uuid) {
                        issues.push(MappingIssue::UuidPlayerIdMismatch {
                            player_id: player.player_id,
                            stored_uuid: stored.to_string(),
                            current_uuid: zone.uuid.clone(),
                        });
                    }
                } else {
                    issues.push(MappingIssue::MissingLoxoneUuid {
                        player_id: player.player_id,
                    });
                }
            }
        }
    }

    let mut cast_ids: std::collections::HashMap<String, Vec<usize>> =
        std::collections::HashMap::new();
    for player in &config.players {
        if let Some(id) = player.cast_device_id.as_deref().filter(|s| !s.is_empty()) {
            cast_ids
                .entry(id.to_ascii_lowercase())
                .or_default()
                .push(player.player_id);
        }
    }
    for (cast_device_id, player_ids) in cast_ids {
        if player_ids.len() > 1 {
            for player_id in player_ids {
                issues.push(MappingIssue::DuplicateCastDeviceId {
                    player_id,
                    cast_device_id: cast_device_id.clone(),
                });
            }
        }
    }

    issues
}

pub fn log_mapping_issues(issues: &[MappingIssue]) {
    for issue in issues {
        match issue {
            MappingIssue::UuidPlayerIdMismatch {
                player_id,
                stored_uuid,
                current_uuid,
            } => warn!(
                player_id,
                stored_uuid,
                current_uuid,
                "loxone player mapping uuid mismatch — playerid may have been reassigned in config"
            ),
            MappingIssue::UnknownPlayerId { player_id } => warn!(
                player_id,
                "configured player_id not present in loxone config"
            ),
            MappingIssue::MissingLoxoneUuid { player_id } => warn!(
                player_id,
                "player mapping missing loxone_uuid — will sync on next setconfig"
            ),
            MappingIssue::DuplicateCastDeviceId {
                player_id,
                cast_device_id,
            } => warn!(
                player_id,
                cast_device_id, "duplicate cast_device_id — each zone needs a unique cast speaker"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RoomFavItem;

    fn zone(id: usize, uuid: &str) -> LoxoneZoneSnapshot {
        LoxoneZoneSnapshot {
            player_id: id,
            uuid: uuid.to_string(),
            name: format!("Zone {id}"),
        }
    }

    fn mapping(id: usize, loxone_uuid: Option<&str>) -> PlayerMapping {
        PlayerMapping {
            player_id: id,
            loxone_uuid: loxone_uuid.map(str::to_string),
            name: format!("Zone {id}"),
            ..Default::default()
        }
    }

    #[test]
    fn validate_detects_uuid_mismatch() {
        let config = ZoneConfig {
            players: vec![mapping(2, Some("old-uuid"))],
        };
        let issues = validate_mappings(&config, &[zone(2, "new-uuid")]);
        assert!(issues
            .iter()
            .any(|i| matches!(i, MappingIssue::UuidPlayerIdMismatch { player_id: 2, .. })));
    }

    #[test]
    fn apply_loxone_snapshots_updates_uuid_and_adds_rows() {
        let mut config = ZoneConfig {
            players: vec![mapping(1, None)],
        };
        assert!(apply_loxone_snapshots(
            &mut config,
            &[zone(1, "lox-1"), zone(2, "lox-2")]
        ));
        assert_eq!(config.players[0].loxone_uuid.as_deref(), Some("lox-1"));
        assert_eq!(config.players[1].player_id, 2);
        assert!(!apply_loxone_snapshots(
            &mut config,
            &[zone(1, "lox-1"), zone(2, "lox-2")]
        ));
    }

    #[test]
    fn prune_drops_orphan_player_and_favorites() {
        use crate::config::AudioserverConfigFile;
        use crate::room_favs;

        AudioserverConfigFile::with_test_cache(
            AudioserverConfigFile {
                audioserver: ZoneConfig {
                    players: vec![mapping(1, Some("a")), mapping(2, Some("b"))],
                },
                ..Default::default()
            },
            || {
                room_favs::add(
                    1,
                    "A",
                    "tunein:station:s1",
                    room_favs::FavoriteMetadata::default(),
                    None,
                )
                .unwrap();
                room_favs::add(
                    2,
                    "B",
                    "tunein:station:s2",
                    room_favs::FavoriteMetadata::default(),
                    None,
                )
                .unwrap();
                let mut config = ZoneConfig::get();
                let zones = vec![ZoneDescriptor {
                    player_id: 1,
                    uuid: "a".into(),
                    name: "Z1".into(),
                }];
                assert!(prune_players_not_in_loxone_zones(&mut config, &zones));
                assert_eq!(config.players.len(), 1);
                assert_eq!(config.players[0].player_id, 1);
                assert_eq!(config.players[0].room_favorites.len(), 1);
                ZoneConfig::reset_cache_for_test(config);
            },
        );
    }

    #[test]
    fn prune_keeps_zone_still_in_loxone_list() {
        let mut config = ZoneConfig {
            players: vec![mapping(3, Some("lox-3"))],
        };
        config.players[0].room_favorites.push(RoomFavItem {
            id: 1,
            slot: 1,
            plus: true,
            name: "Radio".into(),
            title: "Radio".into(),
            audiopath: "tunein:station:s1".into(),
            item_type: "tunein".into(),
            coverurl: String::new(),
            artist: String::new(),
            album: String::new(),
            service: "tunein".into(),
            service_type: 3,
            owner: String::new(),
            speaker_kind: String::new(),
            speaker_media: None,
        });
        let zones = vec![ZoneDescriptor {
            player_id: 3,
            uuid: "lox-3".into(),
            name: "Guest".into(),
        }];
        assert!(!prune_players_not_in_loxone_zones(&mut config, &zones));
        assert_eq!(config.players[0].room_favorites.len(), 1);
    }

    #[test]
    fn duplicate_cast_device_id_detected() {
        let cast_id = "c431e9d77cba7b8a3c2fde5fca5eba96";
        let mut a = mapping(1, Some("a"));
        a.cast_device_id = Some(cast_id.into());
        let mut b = mapping(2, Some("b"));
        b.cast_device_id = Some(cast_id.to_uppercase());
        let config = ZoneConfig {
            players: vec![a, b],
        };
        let issues = validate_mappings(&config, &[zone(1, "a"), zone(2, "b")]);
        assert_eq!(
            issues
                .iter()
                .filter(|i| matches!(i, MappingIssue::DuplicateCastDeviceId { .. }))
                .count(),
            2
        );
    }

    #[test]
    fn validate_unknown_player_id_and_missing_uuid() {
        let config = ZoneConfig {
            players: vec![mapping(99, Some("u")), mapping(1, None)],
        };
        let issues = validate_mappings(&config, &[zone(1, "lox-1")]);
        assert!(issues
            .iter()
            .any(|i| matches!(i, MappingIssue::UnknownPlayerId { player_id: 99 })));
        assert!(issues
            .iter()
            .any(|i| matches!(i, MappingIssue::MissingLoxoneUuid { player_id: 1 })));
    }

    #[test]
    fn mapping_issue_codes_and_player_ids() {
        let issues = [
            MappingIssue::UuidPlayerIdMismatch {
                player_id: 1,
                stored_uuid: "a".into(),
                current_uuid: "b".into(),
            },
            MappingIssue::UnknownPlayerId { player_id: 2 },
            MappingIssue::MissingLoxoneUuid { player_id: 3 },
            MappingIssue::DuplicateCastDeviceId {
                player_id: 4,
                cast_device_id: "c".into(),
            },
        ];
        assert_eq!(issues[0].code(), "uuid_playerid_mismatch");
        assert_eq!(issues[2].code(), "missing_loxone_uuid");
        assert_eq!(issues[3].player_id(), 4);
        log_mapping_issues(&issues);
    }
}
