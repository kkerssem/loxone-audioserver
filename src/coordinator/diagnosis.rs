//! `audio/cfg/diagnosis`: what the audioserver knows about its zones, speakers and Cast
//! devices, with the problems that keep a zone from working.
//!
//! The Loxone App shows the reply as indented JSON text (no other formatting), so the report
//! is plain nested objects with short one-line values, in reading order: struct field order is
//! the order on screen.

use std::collections::HashMap;

use serde::Serialize;

use crate::{
    cast::{cast_device_ids_match, known_cast_devices, CastDeviceInfo, CastPlaybackManager},
    config::{PlayerMapping, ZoneConfig},
    loxone_config::ZoneDescriptor,
    player_state::Mode,
    speaker::{
        binding::{is_bound, resolve_targets, ZoneSpeakerTarget},
        DiscoveredSpeaker, SpeakerKind, SpeakerManager, SpeakerPower, SpeakerStatus,
    },
};

#[derive(Debug, Serialize)]
pub(crate) struct DiagnosisReport {
    audioserver: AudioserverInfo,
    /// Everything below that needs attention, one line each; empty when all is well.
    problems: Vec<String>,
    zones: Vec<ZoneReport>,
    /// Speakers found on the network that no zone controls.
    unbound_speakers: Vec<SpeakerReport>,
    /// Cast devices found by the last browse.
    cast_devices: Vec<CastDeviceReport>,
}

#[derive(Debug, Serialize)]
struct AudioserverInfo {
    version: &'static str,
    serial: String,
    loxone_config: &'static str,
}

#[derive(Debug, Serialize)]
struct ZoneReport {
    zone: usize,
    name: String,
    /// The bound speaker, or why there is none.
    speaker: SpeakerBinding,
    cast_device: CastBinding,
    playing: String,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum SpeakerBinding {
    Bound(SpeakerReport),
    None(&'static str),
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum CastBinding {
    Configured {
        id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        found: bool,
    },
    None(&'static str),
}

#[derive(Debug, Serialize)]
struct SpeakerReport {
    name: String,
    kind: &'static str,
    id: String,
    host: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    state: Option<&'static str>,
}

#[derive(Debug, Serialize)]
struct CastDeviceReport {
    name: String,
    id: String,
    host: String,
    /// Zone whose `cast_device_id` names this device.
    #[serde(skip_serializing_if = "Option::is_none")]
    zone: Option<usize>,
}

/// What a zone's backends report right now (gathered by the coordinator).
#[derive(Debug, Default)]
pub(crate) struct ZoneLive {
    /// Kind of the speaker the manager polls for the zone (`None`: not polled).
    pub polled_kind: Option<SpeakerKind>,
    pub speaker: Option<SpeakerStatus>,
    /// Audiopath of the Cast radio the audioserver plays on the zone.
    pub cast_radio: Option<String>,
}

/// Everything the report is built from.
pub(crate) struct DiagnosisInput<'a> {
    pub serial: &'a str,
    pub loxone_config: bool,
    pub loxone_zones: &'a [ZoneDescriptor],
    pub config: &'a ZoneConfig,
    pub discovered: &'a [DiscoveredSpeaker],
    pub cast_devices: &'a [CastDeviceInfo],
    pub live: &'a HashMap<usize, ZoneLive>,
}

fn power_text(power: SpeakerPower) -> &'static str {
    match power {
        SpeakerPower::On => "on",
        SpeakerPower::Standby => "standby",
        SpeakerPower::Unreachable => "not answering",
    }
}

fn playing_text(live: Option<&ZoneLive>) -> String {
    let Some(live) = live else {
        return "nothing".to_string();
    };
    if let Some(audiopath) = &live.cast_radio {
        return format!("cast radio {audiopath}");
    }
    let Some(status) = &live.speaker else {
        return "nothing".to_string();
    };
    let mode = match status.mode {
        Some(Mode::Play | Mode::Buffer) => "playing",
        Some(Mode::Pause) => "paused",
        Some(Mode::Stop) | None => return "nothing".to_string(),
    };
    let what = [status.title.as_str(), status.artist.as_str()]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" - ");
    match (status.source.is_empty(), what.is_empty()) {
        (true, true) => mode.to_string(),
        (false, true) => format!("{mode} from {}", status.source),
        (true, false) => format!("{mode}: {what}"),
        (false, false) => format!("{mode} from {}: {what}", status.source),
    }
}

fn non_blank(value: &Option<String>) -> Option<&str> {
    value.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

fn speaker_binding(
    player: &PlayerMapping,
    target: Option<&ZoneSpeakerTarget>,
    binding_problem: Option<&str>,
    input: &DiagnosisInput,
    problems: &mut Vec<String>,
    label: &str,
) -> SpeakerBinding {
    let live = input.live.get(&player.player_id);
    let Some(target) = target else {
        if let Some(problem) = binding_problem {
            problems.push(format!("{label}: {problem}"));
            return SpeakerBinding::None("misconfigured");
        }
        if let Some(id) = non_blank(&player.speaker_id) {
            problems.push(format!("{label}: speaker {id} not found on the network"));
            return SpeakerBinding::None("not found");
        }
        if !input.discovered.is_empty() {
            problems.push(format!(
                "{label}: no speaker bound; set speaker_id to one of unbound_speakers"
            ));
        }
        return SpeakerBinding::None("not bound");
    };
    let name = input
        .discovered
        .iter()
        .find(|s| s.id.eq_ignore_ascii_case(&target.speaker_id))
        .map(|s| s.name.clone())
        .unwrap_or_default();
    let polled = live.and_then(|l| l.polled_kind).is_some();
    let status = live.and_then(|l| l.speaker.as_ref());
    if !polled {
        problems.push(format!(
            "{label}: {} speakers are not supported yet",
            target.kind.as_str()
        ));
    } else if status.is_some_and(|s| s.power == SpeakerPower::Unreachable) {
        problems.push(format!(
            "{label}: speaker at {} is not answering",
            target.host
        ));
    }
    SpeakerBinding::Bound(SpeakerReport {
        name,
        kind: target.kind.as_str(),
        id: target.speaker_id.clone(),
        host: target.host.clone(),
        state: status.map(|s| power_text(s.power)),
    })
}

fn cast_binding(
    player: &PlayerMapping,
    input: &DiagnosisInput,
    problems: &mut Vec<String>,
    label: &str,
) -> CastBinding {
    let Some(id) = non_blank(&player.cast_device_id) else {
        return CastBinding::None("none (radio and voice messages need cast_device_id)");
    };
    let device = input
        .cast_devices
        .iter()
        .find(|d| cast_device_ids_match(id, &d.device_id));
    // Before the first browse nothing is known yet: not a problem with this zone.
    if device.is_none() && !input.cast_devices.is_empty() {
        problems.push(format!(
            "{label}: cast device {id} not found on the network"
        ));
    }
    CastBinding::Configured {
        id: id.to_string(),
        name: device.map(|d| d.friendly_name.clone()),
        found: device.is_some(),
    }
}

/// Collect the zones' live state and build the report.
pub(crate) fn gather(
    serial: &str,
    loxone_config: bool,
    loxone_zones: &[ZoneDescriptor],
    discovered: &[DiscoveredSpeaker],
    speakers: &SpeakerManager,
    cast: &CastPlaybackManager,
) -> DiagnosisReport {
    let config = ZoneConfig::get();
    let live = config
        .players
        .iter()
        .map(|p| {
            let id = p.player_id;
            let live = ZoneLive {
                polled_kind: speakers.kind(id),
                speaker: speakers.status_for_commands(id),
                cast_radio: cast.radio_audiopath(id),
            };
            (id, live)
        })
        .collect();
    diagnosis_report(&DiagnosisInput {
        serial,
        loxone_config,
        loxone_zones,
        config: &config,
        discovered,
        cast_devices: &known_cast_devices(),
        live: &live,
    })
}

/// Build the report (pure; [`gather`] collects the [`DiagnosisInput`]).
pub(crate) fn diagnosis_report(input: &DiagnosisInput) -> DiagnosisReport {
    let mut problems = Vec::new();
    if !input.loxone_config {
        problems.push(
            "no configuration received from the Miniserver yet; pair the audioserver in Loxone Config"
                .to_string(),
        );
    }
    let (targets, binding_problems) = resolve_targets(input.config, input.discovered);
    let mut zones = Vec::new();
    for player in &input.config.players {
        let name = if player.name.trim().is_empty() {
            input
                .loxone_zones
                .iter()
                .find(|z| z.player_id == player.player_id)
                .map(|z| z.name.clone())
                .unwrap_or_default()
        } else {
            player.name.clone()
        };
        let label = format!("zone {} ({name})", player.player_id);
        let target = targets.iter().find(|t| t.player_id == player.player_id);
        let binding_problem = binding_problems
            .iter()
            .find(|p| p.player_id == player.player_id)
            .map(|p| p.message.as_str());
        let speaker = speaker_binding(
            player,
            target,
            binding_problem,
            input,
            &mut problems,
            &label,
        );
        let cast_device = cast_binding(player, input, &mut problems, &label);
        zones.push(ZoneReport {
            zone: player.player_id,
            name,
            speaker,
            cast_device,
            playing: playing_text(input.live.get(&player.player_id)),
        });
    }
    for zone in input.loxone_zones {
        if !input
            .config
            .players
            .iter()
            .any(|p| p.player_id == zone.player_id)
        {
            problems.push(format!(
                "zone {} ({}): in the Loxone configuration but not in audioserver-config.json",
                zone.player_id, zone.name
            ));
        }
    }
    let unbound_speakers = input
        .discovered
        .iter()
        .filter(|s| !is_bound(s, &targets))
        .map(|s| SpeakerReport {
            name: s.name.clone(),
            kind: s.kind.as_str(),
            id: s.id.clone(),
            host: s.host.clone(),
            state: None,
        })
        .collect();
    let cast_devices = input
        .cast_devices
        .iter()
        .map(|d| CastDeviceReport {
            name: d.friendly_name.clone(),
            id: d.device_id.clone(),
            host: d.host.clone(),
            zone: input
                .config
                .players
                .iter()
                .find(|p| {
                    non_blank(&p.cast_device_id)
                        .is_some_and(|id| cast_device_ids_match(id, &d.device_id))
                })
                .map(|p| p.player_id),
        })
        .collect();
    DiagnosisReport {
        audioserver: AudioserverInfo {
            version: env!("CARGO_PKG_VERSION"),
            serial: input.serial.to_string(),
            loxone_config: if input.loxone_config {
                "received"
            } else {
                "not received"
            },
        },
        problems,
        zones,
        unbound_speakers,
        cast_devices,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn player(id: usize, name: &str) -> PlayerMapping {
        PlayerMapping {
            player_id: id,
            name: name.into(),
            ..Default::default()
        }
    }

    fn speaker(id: &str, name: &str, kind: SpeakerKind) -> DiscoveredSpeaker {
        DiscoveredSpeaker {
            id: id.into(),
            kind,
            name: name.into(),
            host: format!("host-{id}"),
        }
    }

    fn cast(id: &str, name: &str) -> CastDeviceInfo {
        CastDeviceInfo {
            host: format!("cast-{id}"),
            port: 8009,
            friendly_name: name.into(),
            device_id: id.into(),
            model: None,
        }
    }

    fn report(input: &DiagnosisInput) -> Value {
        serde_json::to_value(diagnosis_report(input)).unwrap()
    }

    #[test]
    fn healthy_zone_reports_its_speaker_cast_device_and_playback() {
        let mut living = player(1, "Living");
        living.speaker_id = Some("muso".into());
        living.cast_device_id = Some("c1".into());
        let config = ZoneConfig {
            players: vec![living],
        };
        let discovered = [speaker("muso", "Mu-so", SpeakerKind::NaimStream800)];
        let casts = [cast("c1", "Mu-so")];
        let live = HashMap::from([(
            1,
            ZoneLive {
                polled_kind: Some(SpeakerKind::NaimStream800),
                speaker: Some(SpeakerStatus {
                    mode: Some(Mode::Play),
                    source: "qobuz".into(),
                    title: "Song".into(),
                    artist: "Band".into(),
                    ..Default::default()
                }),
                cast_radio: None,
            },
        )]);
        let zones = [ZoneDescriptor {
            player_id: 1,
            uuid: "u1".into(),
            name: "Living".into(),
        }];
        let v = report(&DiagnosisInput {
            serial: "504F94000002",
            loxone_config: true,
            loxone_zones: &zones,
            config: &config,
            discovered: &discovered,
            cast_devices: &casts,
            live: &live,
        });
        assert_eq!(v["problems"], json!([]));
        assert_eq!(v["audioserver"]["serial"], "504F94000002");
        assert_eq!(
            v["zones"][0]["speaker"],
            json!({ "name": "Mu-so", "kind": "naim_stream800", "id": "muso", "host": "host-muso", "state": "on" })
        );
        assert_eq!(
            v["zones"][0]["cast_device"],
            json!({ "id": "c1", "name": "Mu-so", "found": true })
        );
        assert_eq!(v["zones"][0]["playing"], "playing from qobuz: Song - Band");
        assert_eq!(v["unbound_speakers"], json!([]));
        assert_eq!(v["cast_devices"][0]["zone"], 1);
    }

    #[test]
    fn problems_name_the_zone_and_the_fix() {
        let mut missing = player(2, "Kitchen");
        missing.speaker_id = Some("gone".into());
        missing.cast_device_id = Some("c9".into());
        let mut manual = player(3, "Office");
        manual.speaker_host = Some("10.0.0.9".into());
        let mut down = player(4, "Bath");
        down.speaker_id = Some("kef".into());
        let config = ZoneConfig {
            players: vec![player(1, "Living"), missing, manual, down],
        };
        let discovered = [
            speaker("muso", "Mu-so", SpeakerKind::NaimStream800),
            speaker("kef", "LS60", SpeakerKind::KefStream800),
        ];
        let casts = [cast("c1", "Mu-so")];
        let live = HashMap::from([(
            4,
            ZoneLive {
                polled_kind: Some(SpeakerKind::KefStream800),
                speaker: Some(SpeakerStatus {
                    power: SpeakerPower::Unreachable,
                    ..Default::default()
                }),
                cast_radio: None,
            },
        )]);
        let zones = [ZoneDescriptor {
            player_id: 5,
            uuid: "u5".into(),
            name: "Garden".into(),
        }];
        let v = report(&DiagnosisInput {
            serial: "504F94000002",
            loxone_config: false,
            loxone_zones: &zones,
            config: &config,
            discovered: &discovered,
            cast_devices: &casts,
            live: &live,
        });
        let problems: Vec<&str> = v["problems"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p.as_str().unwrap())
            .collect();
        assert!(problems[0].starts_with("no configuration received"));
        assert!(problems.contains(
            &"zone 1 (Living): no speaker bound; set speaker_id to one of unbound_speakers"
        ));
        assert!(problems.contains(&"zone 2 (Kitchen): speaker gone not found on the network"));
        assert!(problems.contains(&"zone 2 (Kitchen): cast device c9 not found on the network"));
        assert!(problems
            .iter()
            .any(|p| p.starts_with("zone 3 (Office): speaker_host needs speaker_kind")));
        assert!(problems.contains(&"zone 4 (Bath): speaker at host-kef is not answering"));
        assert!(problems.contains(
            &"zone 5 (Garden): in the Loxone configuration but not in audioserver-config.json"
        ));
        assert_eq!(v["zones"][0]["speaker"], "not bound");
        assert_eq!(v["zones"][1]["speaker"], "not found");
        assert_eq!(v["zones"][2]["speaker"], "misconfigured");
        assert_eq!(v["zones"][3]["speaker"]["state"], "not answering");
        assert!(v["zones"][0]["cast_device"]
            .as_str()
            .unwrap()
            .starts_with("none"));
        assert_eq!(v["unbound_speakers"][0]["id"], "muso");
        assert_eq!(v["unbound_speakers"].as_array().unwrap().len(), 1);
        assert!(v["cast_devices"][0].get("zone").is_none());
    }

    #[test]
    fn unsupported_kind_and_cast_radio_are_reported() {
        let mut old = player(1, "Den");
        old.speaker_host = Some("10.0.0.5".into());
        old.speaker_kind = Some("kef_legacy".into());
        old.cast_device_id = Some("c1".into());
        let config = ZoneConfig { players: vec![old] };
        let live = HashMap::from([(
            1,
            ZoneLive {
                cast_radio: Some("tunein:station:s1".into()),
                ..Default::default()
            },
        )]);
        let v = report(&DiagnosisInput {
            serial: "504F94000002",
            loxone_config: true,
            loxone_zones: &[],
            config: &config,
            discovered: &[],
            cast_devices: &[],
            live: &live,
        });
        // No browse result yet: an unknown Cast device is not reported as missing.
        assert_eq!(
            v["problems"],
            json!(["zone 1 (Den): kef_legacy speakers are not supported yet"])
        );
        assert_eq!(v["zones"][0]["cast_device"]["found"], false);
        assert_eq!(v["zones"][0]["playing"], "cast radio tunein:station:s1");
    }

    #[test]
    fn playing_text_covers_paused_and_idle() {
        let status = |mode, source: &str, title: &str| ZoneLive {
            speaker: Some(SpeakerStatus {
                mode,
                source: source.into(),
                title: title.into(),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(playing_text(None), "nothing");
        assert_eq!(playing_text(Some(&ZoneLive::default())), "nothing");
        assert_eq!(playing_text(Some(&status(None, "wifi", "x"))), "nothing");
        assert_eq!(
            playing_text(Some(&status(Some(Mode::Pause), "", "Song"))),
            "paused: Song"
        );
        assert_eq!(
            playing_text(Some(&status(Some(Mode::Play), "optical", ""))),
            "playing from optical"
        );
        assert_eq!(
            playing_text(Some(&status(Some(Mode::Play), "", ""))),
            "playing"
        );
    }

    #[test]
    fn report_keys_keep_reading_order() {
        let config = ZoneConfig::default();
        let live = HashMap::new();
        let text = serde_json::to_string(&diagnosis_report(&DiagnosisInput {
            serial: "504F94000002",
            loxone_config: true,
            loxone_zones: &[],
            config: &config,
            discovered: &[],
            cast_devices: &[],
            live: &live,
        }))
        .unwrap();
        let order: Vec<usize> = [
            "\"audioserver\"",
            "\"problems\"",
            "\"zones\"",
            "\"unbound_speakers\"",
            "\"cast_devices\"",
        ]
        .iter()
        .map(|k| text.find(k).unwrap())
        .collect();
        assert!(order.windows(2).all(|w| w[0] < w[1]), "{text}");
    }
}
