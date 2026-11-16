//! Radio `PlayerState` for Loxone `audio_event` fan-out.

use crate::player_state::{Mode, PlayerState, Playerid};

use super::audiopath::{AUDIOTYPE_CUSTOM_FILE, AUDIOTYPE_RADIO};
use super::tunein_resolve::ResolvedRadioStream;

pub fn build_radio_player_state(
    playerid: Playerid,
    zone_name: Option<&str>,
    resolved: &ResolvedRadioStream,
    mode: Mode,
    volume: f32,
) -> PlayerState {
    let audiotype = if resolved.is_custom {
        AUDIOTYPE_CUSTOM_FILE
    } else {
        AUDIOTYPE_RADIO
    };
    let title = if resolved.title.is_empty() {
        resolved.audiopath.clone()
    } else {
        resolved.title.clone()
    };
    PlayerState {
        playerid,
        mode,
        audiopath: resolved.audiopath.clone(),
        audiotype,
        station: title.clone(),
        title,
        artist: String::new(),
        album: String::new(),
        cover_url: resolved.cover_url.clone().unwrap_or_default(),
        duration: 0,
        time: 0,
        volume,
        name: zone_name.unwrap_or("").to_string(),
        queue_authority: "local".into(),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player_state::enrich_for_loxone_ui;

    fn sample_resolved() -> ResolvedRadioStream {
        ResolvedRadioStream {
            audiopath: "tunein:station:s6707".into(),
            stream_url: "http://example/stream".into(),
            content_type: "audio/mpeg".into(),
            title: "Test Station".into(),
            cover_url: None,
            is_custom: false,
        }
    }

    #[test]
    fn radio_state_needs_loxone_envelope() {
        let mut state = build_radio_player_state(
            Playerid(3),
            Some("Bedroom"),
            &sample_resolved(),
            Mode::Play,
            50.0,
        );
        assert_eq!(state.zone_type, 0);
        assert!(state.client_state.is_empty());
        assert!(state.players.is_empty());

        enrich_for_loxone_ui(&mut state, Some("Bedroom"), Some("504f94ff1bb3"));
        assert_eq!(state.zone_type, 3);
        assert_eq!(state.client_state, "on");
        assert_eq!(state.power, "on");
        assert_eq!(state.players.len(), 1);
        assert_eq!(state.source_name, "504F94FF1BB3");
    }
}
