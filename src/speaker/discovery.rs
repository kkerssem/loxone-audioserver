//! Find controllable speakers on the LAN via mDNS.
//!
//! - `_sues800device._tcp` — StreamUnlimited platform (KEF LS60 / LSX II, Naim Mu-so 2nd gen).
//!   TXT carries `manufacturer`, `name`, `uuid` (stable id) and `ip`.
//! - `_airplay._tcp` — only used for first-generation KEF (LSX, LS50 Wireless), which advertise
//!   nothing else. TXT `manufacturer=KEF`, `model=LSX`, `deviceid=<MAC>` (stable id); the kind
//!   is confirmed by opening TCP 50001.
//! - `_beocore._tcp` — Bang & Olufsen BeoLink / ASE (Beoplay M3, …). TXT `name` and `jid`
//!   (`<type>.<item>.<serial>@products.bang-olufsen.com`, the stable id); the control API is
//!   HTTP on port 8080 of the same host.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{bail, Result};
use tracing::{debug, warn};

use crate::mdns_browse::{browse_mdns_resolved, resolved_primary_host};

use super::SpeakerKind;

pub const STREAM800_SERVICE: &str = "_sues800device._tcp.local.";
pub const AIRPLAY_SERVICE: &str = "_airplay._tcp.local.";
pub const BEO_SERVICE: &str = "_beocore._tcp.local.";
/// First-generation KEF control port.
pub const KEF_LEGACY_PORT: u16 = 50001;

/// AirPlay `model` values of first-generation KEF speakers.
const KEF_LEGACY_MODELS: &[&str] = &["LSX", "LS50W"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveredSpeaker {
    /// Stable id persisted as `players[].speaker_id`.
    pub id: String,
    pub kind: SpeakerKind,
    pub name: String,
    /// Current IP (or hostname) to reach the control API.
    pub host: String,
}

/// Classify one resolved mDNS record. `instance` is the service instance name (first label).
pub fn parse_service(
    service_type: &str,
    instance: &str,
    resolved_host: &str,
    txt: &HashMap<String, String>,
) -> Option<DiscoveredSpeaker> {
    let get = |key: &str| txt.get(key).map(|v| v.trim()).filter(|v| !v.is_empty());
    let manufacturer = get("manufacturer").unwrap_or_default().to_ascii_lowercase();

    if service_type.starts_with("_sues800device.") {
        let kind = if manufacturer.starts_with("kef") {
            SpeakerKind::KefStream800
        } else if manufacturer.starts_with("naim") {
            SpeakerKind::NaimStream800
        } else {
            return None;
        };
        let id = get("uuid").unwrap_or(instance).to_string();
        let name = get("name").unwrap_or(instance).to_string();
        let host = get("ip").unwrap_or(resolved_host).to_string();
        return Some(DiscoveredSpeaker {
            id,
            kind,
            name,
            host,
        });
    }

    if service_type.starts_with("_beocore.") {
        let id = get("jid").unwrap_or(instance).to_string();
        let name = get("name").unwrap_or(instance).to_string();
        return Some(DiscoveredSpeaker {
            id,
            kind: SpeakerKind::Beo,
            name,
            host: resolved_host.to_string(),
        });
    }
    if service_type.starts_with("_airplay.") {
        let model = get("model").unwrap_or_default();
        if manufacturer != "kef"
            || !KEF_LEGACY_MODELS
                .iter()
                .any(|m| m.eq_ignore_ascii_case(model))
        {
            return None;
        }
        let id = get("deviceid")?.to_ascii_lowercase();
        return Some(DiscoveredSpeaker {
            id,
            kind: SpeakerKind::KefLegacy,
            name: instance.to_string(),
            host: resolved_host.to_string(),
        });
    }

    None
}

/// Add or update `fresh` speakers in `known` (matched by id). Speakers missing from `fresh` are
/// kept: one browse can miss a sleeping speaker, and a lost speaker is reported by its zone's
/// poll loop instead. Returns whether `known` changed.
pub fn remember(known: &mut Vec<DiscoveredSpeaker>, fresh: Vec<DiscoveredSpeaker>) -> bool {
    let mut changed = false;
    for speaker in fresh {
        match known.iter_mut().find(|k| k.id == speaker.id) {
            Some(existing) if *existing == speaker => {}
            Some(existing) => {
                *existing = speaker;
                changed = true;
            }
            None => {
                known.push(speaker);
                changed = true;
            }
        }
    }
    known.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
    changed
}

/// Service instance name from a DNS-SD full name (`<instance>.<service type>`), with DNS-SD
/// escapes decoded: `\DDD` is a decimal byte (`\032` = space), `\.` a literal dot.
pub fn instance_name(fullname: &str, service_type: &str) -> String {
    let raw = fullname
        .strip_suffix(service_type)
        .map(|s| s.strip_suffix('.').unwrap_or(s))
        .unwrap_or_else(|| fullname.split('.').next().unwrap_or_default());
    let mut bytes = Vec::with_capacity(raw.len());
    let src = raw.as_bytes();
    let mut i = 0;
    while i < src.len() {
        if src[i] == b'\\' && i + 1 < src.len() {
            let digits = &src[i + 1..(i + 4).min(src.len())];
            if digits.len() == 3 && digits.iter().all(u8::is_ascii_digit) {
                let value = std::str::from_utf8(digits)
                    .ok()
                    .and_then(|d| d.parse::<u8>().ok());
                if let Some(value) = value {
                    bytes.push(value);
                    i += 4;
                    continue;
                }
            }
            bytes.push(src[i + 1]);
            i += 2;
            continue;
        }
        bytes.push(src[i]);
        i += 1;
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Merge results from several service types, keeping one entry per id, sorted by name.
pub fn merge_discovered(lists: Vec<Vec<DiscoveredSpeaker>>) -> Vec<DiscoveredSpeaker> {
    let mut by_id: HashMap<String, DiscoveredSpeaker> = HashMap::new();
    for speaker in lists.into_iter().flatten() {
        by_id.entry(speaker.id.clone()).or_insert(speaker);
    }
    let mut out: Vec<_> = by_id.into_values().collect();
    out.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
    out
}

async fn browse_one(service_type: &str, timeout: Duration) -> Result<Vec<DiscoveredSpeaker>> {
    let resolved = browse_mdns_resolved(service_type, timeout).await?;
    Ok(resolved
        .iter()
        .filter_map(|service| {
            let instance = instance_name(service.get_fullname(), service_type);
            let txt: HashMap<String, String> = service
                .get_properties()
                .iter()
                .map(|p| (p.key().to_string(), p.val_str().to_string()))
                .collect();
            parse_service(
                service_type,
                &instance,
                &resolved_primary_host(service),
                &txt,
            )
        })
        .collect())
}

/// Whether a first-generation KEF answers on its control port.
async fn kef_legacy_port_open(host: &str) -> bool {
    tokio::time::timeout(
        Duration::from_millis(1500),
        tokio::net::TcpStream::connect((host, KEF_LEGACY_PORT)),
    )
    .await
    .is_ok_and(|r| r.is_ok())
}

/// Browse all supported speaker service types for `timeout`. Fails only when every browse
/// failed (e.g. no usable network interface); a partial result is returned with a warning.
pub async fn browse_speakers(timeout: Duration) -> Result<Vec<DiscoveredSpeaker>> {
    let (stream800, airplay, beo) = tokio::join!(
        browse_one(STREAM800_SERVICE, timeout),
        browse_one(AIRPLAY_SERVICE, timeout),
        browse_one(BEO_SERVICE, timeout)
    );
    if stream800.is_err() && airplay.is_err() && beo.is_err() {
        let errors: Vec<String> = [&stream800, &airplay, &beo]
            .iter()
            .filter_map(|r| r.as_ref().err().map(|e| e.to_string()))
            .collect();
        bail!("mdns browse failed: {}", errors.join("; "));
    }
    for (service, result) in [
        (STREAM800_SERVICE, &stream800),
        (AIRPLAY_SERVICE, &airplay),
        (BEO_SERVICE, &beo),
    ] {
        if let Err(e) = result {
            warn!(service, error = %e, "speaker mdns browse failed");
        }
    }
    let (stream800, airplay, beo) = (
        stream800.unwrap_or_default(),
        airplay.unwrap_or_default(),
        beo.unwrap_or_default(),
    );
    let mut legacy = Vec::new();
    for speaker in airplay {
        if kef_legacy_port_open(&speaker.host).await {
            legacy.push(speaker);
        } else {
            debug!(
                speaker_id = %speaker.id,
                host = %speaker.host,
                "kef airplay speaker without control port 50001"
            );
        }
    }
    Ok(merge_discovered(vec![stream800, legacy, beo]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn txt(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    // TXT records as advertised on a real LAN (2026-11-14), serials trimmed.
    #[test]
    fn kef_ls60_from_stream800_record() {
        let record = txt(&[
            ("name", "LS60 Wireless"),
            ("manufacturer", "KEF"),
            ("uuid", "ls60w-444ca1e2-463e-4c3a-8877-18d46bc61e65"),
            ("ip", "192.168.50.209"),
        ]);
        let speaker = parse_service(
            "_sues800device._tcp.local.",
            "ls60w-444ca1e2-463e-4c3a-8877-18d46bc61e65",
            "ls60w-02005e000002.local",
            &record,
        )
        .unwrap();
        assert_eq!(speaker.kind, SpeakerKind::KefStream800);
        assert_eq!(speaker.id, "ls60w-444ca1e2-463e-4c3a-8877-18d46bc61e65");
        assert_eq!(speaker.name, "LS60 Wireless");
        assert_eq!(speaker.host, "192.168.50.209");
    }

    #[test]
    fn naim_muso_from_stream800_record() {
        let record = txt(&[
            ("uuid", "naims810x-c2376dfe-eb62-461c-8a48-8738b85926f5"),
            ("manufacturer", "Naim Audio"),
            ("model", "Mu-so Qb"),
            ("name", "Mu-so Qb"),
            ("ip", "192.168.50.207"),
        ]);
        let speaker = parse_service(
            "_sues800device._tcp.local.",
            "naims810x-c2376dfe-eb62-461c-8a48-8738b85926f5",
            "qb-2001041478.local",
            &record,
        )
        .unwrap();
        assert_eq!(speaker.kind, SpeakerKind::NaimStream800);
        assert_eq!(speaker.name, "Mu-so Qb");
        assert_eq!(speaker.host, "192.168.50.207");
    }

    #[test]
    fn stream800_falls_back_to_instance_and_resolved_host() {
        let speaker = parse_service(
            "_sues800device._tcp.local.",
            "ls60w-abc",
            "10.0.0.5",
            &txt(&[("manufacturer", "KEF")]),
        )
        .unwrap();
        assert_eq!(speaker.id, "ls60w-abc");
        assert_eq!(speaker.name, "ls60w-abc");
        assert_eq!(speaker.host, "10.0.0.5");
    }

    // Beoplay M3 as advertised on the LAN (2026-11-15), serial zeroed.
    #[test]
    fn beoplay_from_beocore_record() {
        let record = txt(&[
            ("name", "Beoplay M3"),
            ("jid", "2730.1200316.00000000@products.bang-olufsen.com"),
            ("node", "beocore"),
            ("txtvers", "1"),
        ]);
        let speaker = parse_service(
            "_beocore._tcp.local.",
            "2730.1200316.00000000@products.bang-olufsen.com",
            "192.168.50.236",
            &record,
        )
        .unwrap();
        assert_eq!(speaker.kind, SpeakerKind::Beo);
        assert_eq!(
            speaker.id,
            "2730.1200316.00000000@products.bang-olufsen.com"
        );
        assert_eq!(speaker.name, "Beoplay M3");
        assert_eq!(speaker.host, "192.168.50.236");
    }

    #[test]
    fn unknown_stream800_vendor_is_ignored() {
        assert!(parse_service(
            "_sues800device._tcp.local.",
            "x",
            "10.0.0.5",
            &txt(&[("manufacturer", "Acme")]),
        )
        .is_none());
    }

    #[test]
    fn kef_lsx_from_airplay_record() {
        let record = txt(&[
            ("manufacturer", "KEF"),
            ("model", "LSX"),
            ("deviceid", "02:00:5E:00:00:01"),
        ]);
        let speaker =
            parse_service("_airplay._tcp.local.", "KEF LSX", "192.168.50.50", &record).unwrap();
        assert_eq!(speaker.kind, SpeakerKind::KefLegacy);
        assert_eq!(speaker.id, "02:00:5e:00:00:01");
        assert_eq!(speaker.name, "KEF LSX");
    }

    #[test]
    fn other_airplay_devices_are_ignored() {
        let apple_tv = txt(&[("model", "AppleTV6,2"), ("deviceid", "02:00:5E:00:00:03")]);
        assert!(parse_service("_airplay._tcp.local.", "Woonkamer", "h", &apple_tv).is_none());
        let kef_new = txt(&[
            ("manufacturer", "KEF"),
            ("model", "LS60W"),
            ("deviceid", "02:00:5E:00:00:02"),
        ]);
        assert!(parse_service("_airplay._tcp.local.", "LS60 Wireless", "h", &kef_new).is_none());
        let no_id = txt(&[("manufacturer", "KEF"), ("model", "LSX")]);
        assert!(parse_service("_airplay._tcp.local.", "KEF LSX", "h", &no_id).is_none());
        assert!(parse_service("_googlecast._tcp.local.", "x", "h", &no_id).is_none());
    }

    #[test]
    fn merge_dedups_by_id_and_sorts_by_name() {
        let a = DiscoveredSpeaker {
            id: "b".into(),
            kind: SpeakerKind::KefLegacy,
            name: "Zeta".into(),
            host: "1".into(),
        };
        let b = DiscoveredSpeaker {
            id: "a".into(),
            kind: SpeakerKind::KefStream800,
            name: "Alpha".into(),
            host: "2".into(),
        };
        let merged = merge_discovered(vec![vec![a.clone(), b.clone()], vec![a.clone()]]);
        assert_eq!(merged, vec![b, a]);
    }

    #[test]
    fn instance_name_strips_service_and_decodes_escapes() {
        assert_eq!(
            instance_name("KEF\\032LSX._airplay._tcp.local.", AIRPLAY_SERVICE),
            "KEF LSX"
        );
        assert_eq!(
            instance_name("Study\\0322\\.1._airplay._tcp.local.", AIRPLAY_SERVICE),
            "Study 2.1"
        );
        assert_eq!(
            instance_name("ls60w-abc._sues800device._tcp.local.", STREAM800_SERVICE),
            "ls60w-abc"
        );
        assert_eq!(
            instance_name("Caf\\195\\169._airplay._tcp.local.", AIRPLAY_SERVICE),
            "Café"
        );
        assert_eq!(
            instance_name("plain.other.local.", AIRPLAY_SERVICE),
            "plain"
        );
    }

    #[test]
    fn remember_keeps_missing_speakers_and_updates_hosts() {
        let speaker = |id: &str, name: &str, host: &str| DiscoveredSpeaker {
            id: id.into(),
            kind: SpeakerKind::KefStream800,
            name: name.into(),
            host: host.into(),
        };
        let mut known = Vec::new();
        assert!(remember(
            &mut known,
            vec![
                speaker("a", "LS60", "10.0.0.1"),
                speaker("b", "Mu-so", "10.0.0.2")
            ]
        ));
        // A browse that misses "b" keeps it; nothing changed.
        assert!(!remember(
            &mut known,
            vec![speaker("a", "LS60", "10.0.0.1")]
        ));
        assert_eq!(known.len(), 2);
        // A new address for "a" is taken over.
        assert!(remember(&mut known, vec![speaker("a", "LS60", "10.0.0.9")]));
        assert_eq!(known.iter().find(|s| s.id == "a").unwrap().host, "10.0.0.9");
        assert!(!remember(&mut known, vec![]));
    }
}
