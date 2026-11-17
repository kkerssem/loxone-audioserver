//! Bind Loxone zones (`players[]`) to speakers: explicit `speaker_id` / `speaker_host`, or an
//! automatic match when the Loxone zone name equals the speaker name.

use std::collections::HashSet;

use crate::config::{PlayerMapping, ZoneConfig};

use super::{DiscoveredSpeaker, SpeakerKind};

/// Where to reach a zone's speaker right now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZoneSpeakerTarget {
    pub player_id: usize,
    pub speaker_id: String,
    pub kind: SpeakerKind,
    pub host: String,
}

fn normalize_name(name: &str) -> String {
    name.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Zones without a `speaker_id` whose name matches exactly one unbound speaker.
/// Returns `(player_id, speaker_id)` pairs to persist.
pub fn auto_bind_by_name(
    config: &ZoneConfig,
    speakers: &[DiscoveredSpeaker],
) -> Vec<(usize, String)> {
    let mut taken: HashSet<String> = config
        .players
        .iter()
        .filter_map(|p| p.speaker_id.clone())
        .map(|id| id.to_ascii_lowercase())
        .collect();
    let mut out = Vec::new();
    for player in &config.players {
        if player.is_speaker_configured() || player.name.trim().is_empty() {
            continue;
        }
        let wanted = normalize_name(&player.name);
        let matches: Vec<&DiscoveredSpeaker> = speakers
            .iter()
            .filter(|s| normalize_name(&s.name) == wanted)
            .filter(|s| !taken.contains(&s.id.to_ascii_lowercase()))
            .collect();
        if let [speaker] = matches.as_slice() {
            taken.insert(speaker.id.to_ascii_lowercase());
            out.push((player.player_id, speaker.id.clone()));
        }
    }
    out
}

fn trimmed(value: &Option<String>) -> Option<&str> {
    value.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

/// A `players[]` row whose speaker settings cannot work as written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BindingProblem {
    pub player_id: usize,
    pub message: String,
}

/// `Ok(None)`: not bindable yet (e.g. its `speaker_id` has not been discovered).
fn target_for(
    player: &PlayerMapping,
    speakers: &[DiscoveredSpeaker],
) -> Result<Option<ZoneSpeakerTarget>, String> {
    let id = trimmed(&player.speaker_id);
    let manual_host = trimmed(&player.speaker_host);
    if let Some(found) = id.and_then(|id| speakers.iter().find(|s| s.id.eq_ignore_ascii_case(id))) {
        return Ok(Some(ZoneSpeakerTarget {
            player_id: player.player_id,
            speaker_id: found.id.clone(),
            kind: found.kind,
            host: manual_host.unwrap_or(&found.host).to_string(),
        }));
    }
    // Not discovered: a manual host plus an explicit kind still works without mDNS.
    let Some(host) = manual_host else {
        return Ok(None);
    };
    let kind = match trimmed(&player.speaker_kind) {
        None => return Err(
            "speaker_host needs speaker_kind (kef_stream800, naim_stream800, kef_legacy or beo) \
                 when the speaker is not discovered"
                .to_string(),
        ),
        Some(kind) => kind.parse::<SpeakerKind>().map_err(|e| e.to_string())?,
    };
    Ok(Some(ZoneSpeakerTarget {
        player_id: player.player_id,
        speaker_id: id.unwrap_or(host).to_string(),
        kind,
        host: host.to_string(),
    }))
}

/// Resolve every configured zone to a speaker address and API.
///
/// - `speaker_id` found by discovery: use it (a manual `speaker_host` overrides the address).
/// - Not discovered: `speaker_host` + `speaker_kind` is used as-is (no mDNS needed).
/// - Otherwise the zone is left out and shows as unmapped until the speaker reappears.
/// - A row that can never bind as written (manual host without a valid kind) is returned as a
///   [`BindingProblem`] so it can be logged.
pub fn resolve_targets(
    config: &ZoneConfig,
    speakers: &[DiscoveredSpeaker],
) -> (Vec<ZoneSpeakerTarget>, Vec<BindingProblem>) {
    let mut targets = Vec::new();
    let mut problems = Vec::new();
    for player in &config.players {
        match target_for(player, speakers) {
            Ok(Some(target)) => targets.push(target),
            Ok(None) => {}
            Err(message) => problems.push(BindingProblem {
                player_id: player.player_id,
                message,
            }),
        }
    }
    (targets, problems)
}

/// Whether one of `targets` controls `speaker`: by its discovered id, or by a manual
/// `speaker_host` that points at the same address.
pub fn is_bound(speaker: &DiscoveredSpeaker, targets: &[ZoneSpeakerTarget]) -> bool {
    targets.iter().any(|t| {
        t.speaker_id.eq_ignore_ascii_case(&speaker.id) || t.host.eq_ignore_ascii_case(&speaker.host)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn speaker(id: &str, name: &str, kind: SpeakerKind) -> DiscoveredSpeaker {
        DiscoveredSpeaker {
            id: id.into(),
            kind,
            name: name.into(),
            host: format!("host-{id}"),
        }
    }

    #[test]
    fn is_bound_matches_id_or_manual_host() {
        let naim = speaker("Naim-1", "Mu-so Qb", SpeakerKind::NaimStream800);
        let target = |speaker_id: &str, host: &str| ZoneSpeakerTarget {
            player_id: 3,
            speaker_id: speaker_id.into(),
            kind: SpeakerKind::NaimStream800,
            host: host.into(),
        };
        assert!(!is_bound(&naim, &[]));
        assert!(is_bound(&naim, &[target("naim-1", "10.0.0.7")]));
        assert!(is_bound(&naim, &[target("host-Naim-1", "host-Naim-1")]));
        assert!(!is_bound(&naim, &[target("ls60-1", "host-ls60-1")]));
    }

    fn zone(player_id: usize, name: &str) -> PlayerMapping {
        PlayerMapping {
            player_id,
            name: name.into(),
            ..Default::default()
        }
    }

    #[test]
    fn auto_bind_matches_names_case_and_space_insensitive() {
        let config = ZoneConfig {
            players: vec![zone(1, "  mu-so   QB "), zone(2, "Woonkamer"), zone(3, "")],
        };
        let speakers = vec![
            speaker("naim-1", "Mu-so Qb", SpeakerKind::NaimStream800),
            speaker("ls60-1", "LS60 Wireless", SpeakerKind::KefStream800),
        ];
        assert_eq!(
            auto_bind_by_name(&config, &speakers),
            vec![(1, "naim-1".to_string())]
        );
    }

    #[test]
    fn auto_bind_skips_configured_ambiguous_and_taken() {
        let mut bound = zone(1, "Kitchen");
        bound.speaker_id = Some("kef-a".into());
        let config = ZoneConfig {
            players: vec![bound, zone(2, "Kitchen"), zone(3, "Bath")],
        };
        let speakers = vec![
            speaker("kef-a", "Kitchen", SpeakerKind::KefStream800),
            speaker("x", "Bath", SpeakerKind::KefLegacy),
            speaker("y", "Bath", SpeakerKind::KefLegacy),
        ];
        // zone 2 would match kef-a, but zone 1 already owns it; zone 3 is ambiguous.
        assert!(auto_bind_by_name(&config, &speakers).is_empty());
    }

    #[test]
    fn auto_bind_does_not_give_one_speaker_to_two_zones() {
        let config = ZoneConfig {
            players: vec![zone(1, "LSX"), zone(2, "lsx")],
        };
        let speakers = vec![speaker("lsx-1", "LSX", SpeakerKind::KefLegacy)];
        assert_eq!(
            auto_bind_by_name(&config, &speakers),
            vec![(1, "lsx-1".to_string())]
        );
    }

    #[test]
    fn manual_host_without_kind_is_reported() {
        let mut row = zone(7, "Office");
        row.speaker_host = Some("10.0.0.11".into());
        let config = ZoneConfig { players: vec![row] };
        let (targets, problems) = resolve_targets(&config, &[]);
        assert!(targets.is_empty());
        assert_eq!(problems[0].player_id, 7);
        assert!(problems[0].message.contains("speaker_kind"));
    }

    #[test]
    fn resolve_targets_uses_discovered_host_or_manual_override() {
        let mut ls60 = zone(1, "Living");
        ls60.speaker_id = Some("LS60-1".into());
        let mut naim = zone(2, "Bedroom");
        naim.speaker_id = Some("naim-1".into());
        naim.speaker_host = Some("192.168.50.207".into());
        let mut gone = zone(3, "Garage");
        gone.speaker_id = Some("missing".into());
        let mut manual = zone(5, "Office");
        manual.speaker_host = Some("10.0.0.9".into());
        manual.speaker_kind = Some("kef_legacy".into());
        let mut manual_bad_kind = zone(6, "Attic");
        manual_bad_kind.speaker_host = Some("10.0.0.10".into());
        manual_bad_kind.speaker_kind = Some("sonos".into());
        let config = ZoneConfig {
            players: vec![
                ls60,
                naim,
                gone,
                zone(4, "Unmapped"),
                manual,
                manual_bad_kind,
            ],
        };
        let speakers = vec![
            speaker("ls60-1", "LS60 Wireless", SpeakerKind::KefStream800),
            speaker("naim-1", "Mu-so Qb", SpeakerKind::NaimStream800),
        ];
        let (targets, problems) = resolve_targets(&config, &speakers);
        assert_eq!(
            targets,
            vec![
                ZoneSpeakerTarget {
                    player_id: 1,
                    speaker_id: "ls60-1".into(),
                    kind: SpeakerKind::KefStream800,
                    host: "host-ls60-1".into(),
                },
                ZoneSpeakerTarget {
                    player_id: 2,
                    speaker_id: "naim-1".into(),
                    kind: SpeakerKind::NaimStream800,
                    host: "192.168.50.207".into(),
                },
                ZoneSpeakerTarget {
                    player_id: 5,
                    speaker_id: "10.0.0.9".into(),
                    kind: SpeakerKind::KefLegacy,
                    host: "10.0.0.9".into(),
                },
            ]
        );
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].player_id, 6);
        assert!(problems[0].message.contains("sonos"));
    }
}
