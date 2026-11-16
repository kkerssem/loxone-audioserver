//! Loxone radio `audiopath` conventions (V2 UI).

/// `AudioType::Radio` in V2 player state.
pub const AUDIOTYPE_RADIO: u8 = 1;
/// Custom streams use `AudioType::File` with `custom:station` audiopath.
pub const AUDIOTYPE_CUSTOM_FILE: u8 = 0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RadioAudiopath {
    TuneInStation { id: String },
    CustomStation { id: String },
    LegacyRadioUrl(String),
    RawUrl(String),
}

pub fn is_radio_audiopath(id: &str) -> bool {
    parse_radio_audiopath(id).is_some()
}

pub fn is_radio_service(service: &str) -> bool {
    matches!(
        service.to_ascii_lowercase().as_str(),
        "local" | "custom" | "music" | "world" | "tunein"
    )
}

pub fn parse_radio_audiopath(id: &str) -> Option<RadioAudiopath> {
    let id = id.trim();
    if let Some(rest) = id.strip_prefix("tunein:station:") {
        if !rest.is_empty() {
            return Some(RadioAudiopath::TuneInStation {
                id: rest.to_string(),
            });
        }
    }
    if let Some(rest) = id.strip_prefix("custom:station:") {
        if !rest.is_empty() {
            return Some(RadioAudiopath::CustomStation {
                id: rest.to_string(),
            });
        }
    }
    if id.starts_with("radio://") {
        return Some(RadioAudiopath::LegacyRadioUrl(id.to_string()));
    }
    if id.starts_with("http://") || id.starts_with("https://") {
        return Some(RadioAudiopath::RawUrl(id.to_string()));
    }
    None
}

/// Strip V2 `serviceplay/radio/{user}/` prefix before extracting audiopath.
pub fn strip_radio_serviceplay_prefix(rest: &str) -> &str {
    let mut path = rest.trim_start_matches('/');
    if let Some(tail) = path.strip_prefix("radio/") {
        path = tail;
        if let Some((_, after)) = path.split_once('/') {
            path = after;
        }
    }
    path
}

/// Extract the radio audiopath from a Loxone `serviceplay` / `playurl` tail path.
///
/// Accepts V2 radio paths such as `radio/nouser/tunein:station:s123` and strips the
/// `/noshuffle`, `/parentpath/…`, `/parentid/…` and `/?q&…` suffixes the UI may append.
pub fn parse_serviceplay_item_id(rest: &str) -> Option<String> {
    let mut path = rest;
    for marker in ["/?q&", "/noshuffle", "/parentpath/", "/parentid/"] {
        if let Some(idx) = path.find(marker) {
            path = &path[..idx];
        }
    }
    let path = strip_radio_serviceplay_prefix(path.trim_end_matches('/'));
    path.split('/').find_map(|segment| {
        let decoded = urlencoding::decode(segment)
            .map(|s| s.into_owned())
            .unwrap_or_else(|_| segment.to_string());
        is_radio_audiopath(&decoded).then_some(decoded)
    })
}

pub fn tunein_station_audiopath(station_id: &str) -> String {
    format!("tunein:station:{station_id}")
}

pub fn custom_station_audiopath(id: &str) -> String {
    format!("custom:station:{id}")
}

pub fn station_id_from_tunein_row(guide_id: Option<&str>, element: &str) -> String {
    guide_id
        .filter(|s| !s.is_empty())
        .unwrap_or(element)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_tunein_and_custom() {
        assert_eq!(
            parse_radio_audiopath("tunein:station:s123"),
            Some(RadioAudiopath::TuneInStation { id: "s123".into() })
        );
        assert_eq!(
            parse_radio_audiopath("custom:station:my-1"),
            Some(RadioAudiopath::CustomStation { id: "my-1".into() })
        );
    }

    #[test]
    fn strip_radio_serviceplay_prefix_examples() {
        assert_eq!(
            strip_radio_serviceplay_prefix("radio/nouser/tunein:station:abc"),
            "tunein:station:abc"
        );
        assert_eq!(
            strip_radio_serviceplay_prefix("radio/local/tunein:station:abc/noshuffle"),
            "tunein:station:abc/noshuffle"
        );
    }

    #[test]
    fn parse_serviceplay_item_id_radio_v2_path() {
        assert_eq!(
            parse_serviceplay_item_id("radio/nouser/tunein:station:s12345").as_deref(),
            Some("tunein:station:s12345")
        );
        assert_eq!(
            parse_serviceplay_item_id("radio/nouser/custom:station:abc-uuid").as_deref(),
            Some("custom:station:abc-uuid")
        );
        assert_eq!(
            parse_serviceplay_item_id("radio/local/tunein%3Astation%3As9/noshuffle/?q&x")
                .as_deref(),
            Some("tunein:station:s9")
        );
    }

    #[test]
    fn parse_serviceplay_item_id_rejects_non_radio_paths() {
        assert_eq!(
            parse_serviceplay_item_id("qobuz:track:44856655/parentpath/qobuz:playlist:1"),
            None
        );
        assert_eq!(parse_serviceplay_item_id(""), None);
    }
}
