//! Pure mapping from a polled [`SpeakerStatus`] to the Loxone [`PlayerState`] wire shape.

use crate::player_state::{Mode, PlayerState, Playerid};

use super::{SpeakerPower, SpeakerStatus};

/// Loxone `MediaEnum.AudioType` values.
const AUDIOTYPE_STREAM: u8 = 1;
const AUDIOTYPE_LINEIN: u8 = 3;
const AUDIOTYPE_AIRPLAY: u8 = 4;
const AUDIOTYPE_SPOTIFY_CONNECT: u8 = 5;

/// Physical / local inputs shown as line-in in the Loxone UI.
const LINE_IN_SOURCES: &[&str] = &[
    "analog",
    "aux",
    "bluetooth",
    "coaxial",
    "hdmi",
    "optical",
    "tv",
    "usb",
];

pub fn audiotype_for_source(source: &str) -> u8 {
    let source = source.to_ascii_lowercase();
    match source.as_str() {
        "airplay" => AUDIOTYPE_AIRPLAY,
        "spotify" => AUDIOTYPE_SPOTIFY_CONNECT,
        s if LINE_IN_SOURCES.contains(&s) => AUDIOTYPE_LINEIN,
        _ => AUDIOTYPE_STREAM,
    }
}

/// Human label for a source id (`"optical"` → `"Optical"`), used as title when a line-in
/// input carries no track metadata.
pub fn source_label(source: &str) -> String {
    match source.to_ascii_lowercase().as_str() {
        "" => String::new(),
        "wifi" => "Wi-Fi".to_string(),
        "airplay" => "AirPlay".to_string(),
        "tv" => "TV".to_string(),
        "usb" => "USB".to_string(),
        "hdmi" => "HDMI".to_string(),
        other => {
            let mut chars = other.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        }
    }
}

/// Build the zone's `PlayerState` from a speaker poll.
///
/// Standby and unreachable speakers are reported as stopped with `power` `"off"` /
/// `"offline"` (the Loxone UI's `ZONE_STATE` values). Volume is carried as the speaker
/// level; the registry replaces it with the persisted display volume on commit.
pub fn status_to_player_state(playerid: Playerid, status: &SpeakerStatus) -> PlayerState {
    let mut state = PlayerState::idle(playerid);
    state.volume = if status.muted {
        0.0
    } else {
        f32::from(status.volume)
    };

    match status.power {
        SpeakerPower::Standby => {
            state.power = "off".to_string();
            return state;
        }
        SpeakerPower::Unreachable => {
            state.power = "offline".to_string();
            state.client_state = "offline".to_string();
            return state;
        }
        SpeakerPower::On => {}
    }

    let mode = status.mode.clone().unwrap_or(Mode::Stop);
    state.audiotype = audiotype_for_source(&status.source);
    state.title = status.title.clone();
    if state.title.is_empty() && state.audiotype == AUDIOTYPE_LINEIN {
        state.title = source_label(&status.source);
    }
    if state.title.is_empty() && matches!(mode, Mode::Stop) {
        return state;
    }
    state.mode = mode;
    state.power_state = None;
    state.artist = status.artist.clone();
    state.album = status.album.clone();
    state.cover_url = status.cover_url.clone();
    state.cover_revision = status.play_session.clone();
    state.duration = status.duration_s;
    state.time = status.position_s.max(0);
    // Radio from the speaker's own app gets an audiopath, so it can be a room favorite.
    if let Some(station) = &status.station {
        state.audiopath = station.audiopath.clone();
    } else if let Some(track) = crate::qobuz::track_audiopath(&status.track_path) {
        // A Qobuz track the speaker plays from its own Qobuz login: playable again by id.
        state.audiopath = track;
    }
    // The Loxone App shows `station` as the top line and the title below it. Without a
    // station name, station mirrors title, as for Cast radio (`radio::build_radio_player_state`).
    if status.live {
        if status.station_name.is_empty() {
            state.station = status.title.clone();
        } else {
            state.station = status.station_name.clone();
            if state.title == state.station {
                // Nothing about the song: show the station name once.
                state.title.clear();
            }
        }
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;

    fn playing() -> SpeakerStatus {
        SpeakerStatus {
            mode: Some(Mode::Play),
            volume: 34,
            source: "wifi".into(),
            title: "Hey Jude".into(),
            artist: "The Beatles".into(),
            album: "1".into(),
            cover_url: "http://192.168.50.209/cover.jpg".into(),
            duration_s: 431,
            position_s: 12,
            play_session: "77".into(),
            ..Default::default()
        }
    }

    #[test]
    fn speaker_station_becomes_the_audiopath() {
        let radio = SpeakerStatus {
            live: true,
            station: Some(std::sync::Arc::new(crate::speaker::SpeakerStation {
                audiopath: "speaker:radio:0a1b2c3d".into(),
                media: serde_json::json!({"path": "vtuner:x"}),
            })),
            ..playing()
        };
        assert_eq!(
            status_to_player_state(Playerid(2), &radio).audiopath,
            "speaker:radio:0a1b2c3d"
        );
        assert_eq!(
            status_to_player_state(Playerid(2), &playing()).audiopath,
            ""
        );
        let qobuz = SpeakerStatus {
            track_path: "qobuz:/track?id=2531160".into(),
            ..playing()
        };
        assert_eq!(
            status_to_player_state(Playerid(2), &qobuz).audiopath,
            "qobuz:track:2531160"
        );
    }

    #[test]
    fn playing_track_maps_metadata_and_mode() {
        let state = status_to_player_state(Playerid(2), &playing());
        assert_eq!(state.playerid, Playerid(2));
        assert_eq!(state.mode, Mode::Play);
        assert_eq!(state.title, "Hey Jude");
        assert_eq!(state.artist, "The Beatles");
        assert_eq!(state.album, "1");
        assert_eq!(state.cover_url, "http://192.168.50.209/cover.jpg");
        assert_eq!(state.cover_revision, "77");
        assert_eq!(state.duration, 431);
        assert_eq!(state.time, 12);
        assert_eq!(state.audiotype, 1);
        assert_eq!(state.power, "on");
        assert!(state.power_state.is_none());
        assert_eq!(state.volume, 34.0);
    }

    #[test]
    fn muted_volume_is_zero() {
        let status = SpeakerStatus {
            muted: true,
            ..playing()
        };
        assert_eq!(status_to_player_state(Playerid(1), &status).volume, 0.0);
    }

    #[test]
    fn standby_and_unreachable_are_stopped_with_power_state() {
        let standby = SpeakerStatus {
            power: SpeakerPower::Standby,
            ..playing()
        };
        let state = status_to_player_state(Playerid(1), &standby);
        assert_eq!(state.mode, Mode::Stop);
        assert_eq!(state.power, "off");
        assert!(state.title.is_empty());

        let state = status_to_player_state(Playerid(1), &SpeakerStatus::unreachable());
        assert_eq!(state.mode, Mode::Stop);
        assert_eq!(state.power, "offline");
        assert_eq!(state.client_state, "offline");
    }

    #[test]
    fn on_but_idle_is_a_stopped_shell() {
        let idle = SpeakerStatus {
            source: "wifi".into(),
            ..Default::default()
        };
        let state = status_to_player_state(Playerid(1), &idle);
        assert_eq!(state.mode, Mode::Stop);
        assert_eq!(state.power, "on");
        assert!(state.title.is_empty());
    }

    #[test]
    fn line_in_without_metadata_uses_source_label() {
        let tv = SpeakerStatus {
            mode: Some(Mode::Play),
            source: "optical".into(),
            ..Default::default()
        };
        let state = status_to_player_state(Playerid(1), &tv);
        assert_eq!(state.audiotype, 3);
        assert_eq!(state.title, "Optical");
        assert_eq!(state.mode, Mode::Play);
    }

    #[test]
    fn audiotype_and_labels() {
        assert_eq!(audiotype_for_source("AirPlay"), 4);
        assert_eq!(audiotype_for_source("spotify"), 5);
        assert_eq!(audiotype_for_source("bluetooth"), 3);
        assert_eq!(audiotype_for_source("qobuz"), 1);
        assert_eq!(source_label("wifi"), "Wi-Fi");
        assert_eq!(source_label("tv"), "TV");
        assert_eq!(source_label("bluetooth"), "Bluetooth");
        assert_eq!(source_label(""), "");
    }

    #[test]
    fn live_stream_fills_station() {
        let radio = SpeakerStatus {
            live: true,
            title: "Radio Paradise Main Mix".into(),
            source: "radio".into(),
            ..playing()
        };
        let state = status_to_player_state(Playerid(4), &radio);
        assert_eq!(state.station, "Radio Paradise Main Mix");
        assert_eq!(state.title, "Radio Paradise Main Mix");
        assert!(status_to_player_state(Playerid(4), &playing())
            .station
            .is_empty());
    }

    #[test]
    fn live_stream_shows_the_station_above_the_song() {
        let radio = SpeakerStatus {
            live: true,
            title: "The The - Cognitive Dissident".into(),
            station_name: "Radio Paradise Main Mix".into(),
            source: "radio".into(),
            ..playing()
        };
        let state = status_to_player_state(Playerid(4), &radio);
        assert_eq!(state.station, "Radio Paradise Main Mix");
        assert_eq!(state.title, "The The - Cognitive Dissident");

        // No song metadata: the speaker repeats the station as the title; show it once.
        let quiet = SpeakerStatus {
            title: "Radio Paradise Main Mix".into(),
            ..radio
        };
        let state = status_to_player_state(Playerid(4), &quiet);
        assert_eq!(state.station, "Radio Paradise Main Mix");
        assert!(state.title.is_empty());
    }

    #[test]
    fn negative_position_clamps_to_zero() {
        let status = SpeakerStatus {
            position_s: -5,
            ..playing()
        };
        assert_eq!(status_to_player_state(Playerid(1), &status).time, 0);
    }
}
