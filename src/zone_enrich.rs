//! Loxone presentation layer: cover proxy, UI envelope, EQ for wire snapshots.

use crate::loxone_config::LoxoneRuntimeState;
use crate::player_state::{enrich_for_loxone_ui, PlayerState, Playerid};
use crate::zone_equalizer::{bands_to_settings_string, get_bands};

/// `url` through the 7091 cors-proxy at `base`; `id` names the cover (the album, or `x`).
pub fn proxied_cover_url(base: &str, url: &str, id: &str) -> String {
    format!(
        "{}/cors-proxy?url={}&id={}",
        base,
        urlencoding::encode(url),
        urlencoding::encode(id)
    )
}

/// Rewrite HTTPS cover URLs through the 7091 cors-proxy when configured.
pub fn apply_cover_proxy(state: &mut PlayerState, base: Option<&str>) {
    let Some(base) = base else {
        return;
    };
    if state.cover_url.is_empty() {
        return;
    }
    if state.cover_url.contains("/cors-proxy?") {
        return;
    }
    let id = if state.album.is_empty() {
        "x".to_string()
    } else {
        state.album.clone()
    };
    state.cover_url = proxied_cover_url(base, &state.cover_url, &id);
    // The App only reloads a cover whose URL changed; `v` (ignored by the proxy) makes a
    // restarted stream with the same icon a new URL.
    if !state.cover_revision.is_empty() {
        state.cover_url.push_str("&v=");
        state
            .cover_url
            .push_str(&urlencoding::encode(&state.cover_revision));
    }
}

/// Loxone UI envelope + cover proxy; optional runtime EQ for status queries.
pub fn enrich_zone_state(
    mut state: PlayerState,
    playerid: Playerid,
    zone_name: Option<&str>,
    runtime: Option<&LoxoneRuntimeState>,
    audioserver_serial: &str,
    cover_proxy_base: &str,
) -> PlayerState {
    enrich_for_loxone_ui(&mut state, zone_name, Some(audioserver_serial));
    apply_cover_proxy(&mut state, Some(cover_proxy_base));
    if runtime.is_some() {
        state.equalizer_settings = bands_to_settings_string(&get_bands(playerid));
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player_state::{player_state_for_protocol, Mode, Playerid};
    use crate::radio::{build_radio_player_state, ResolvedRadioStream};

    #[test]
    fn enrich_zone_state_applies_eq_when_runtime_present() {
        let resolved = ResolvedRadioStream {
            audiopath: "tunein:station:1".into(),
            stream_url: "http://example/stream".into(),
            content_type: "audio/mpeg".into(),
            title: "FM".into(),
            cover_url: None,
            is_custom: false,
        };
        let base =
            build_radio_player_state(Playerid(2), Some("Kitchen"), &resolved, Mode::Play, 50.0);
        let mut runtime = LoxoneRuntimeState::default();
        let state = enrich_zone_state(
            base,
            Playerid(2),
            Some("Kitchen"),
            Some(&runtime),
            "504f94ff1bb3",
            "http://10.0.0.1:7091",
        );
        assert!(!state.equalizer_settings.is_empty());
        let _ = &mut runtime;
    }

    #[test]
    fn apply_cover_proxy_wraps_https_url() {
        let mut state = player_state_for_protocol(Playerid(1), Some("Zone"), Some("ABC"));
        state.cover_url = "https://static.qobuz.com/cover.jpg".into();
        state.album = "Album".into();
        apply_cover_proxy(&mut state, Some("http://192.168.1.50:7091"));
        assert!(state
            .cover_url
            .starts_with("http://192.168.1.50:7091/cors-proxy?url="));
        assert!(state.cover_url.contains("id=Album"));
    }

    #[test]
    fn apply_cover_proxy_adds_cover_revision() {
        let mut state = PlayerState::idle(Playerid(3));
        state.cover_url = "https://media.focal-naim.com/stations/2822.png".into();
        state.cover_revision = "553214760".into();
        apply_cover_proxy(&mut state, Some("http://192.168.1.50:7091"));
        assert!(
            state.cover_url.ends_with("&id=x&v=553214760"),
            "{}",
            state.cover_url
        );
        assert!(
            !state.cover_url.contains('#'),
            "source URL is passed unchanged"
        );
    }

    #[test]
    fn apply_cover_proxy_skips_empty_and_already_proxied() {
        let mut state = player_state_for_protocol(Playerid(1), None, None);
        apply_cover_proxy(&mut state, Some("http://10.0.0.1:7091"));
        assert_eq!(state.cover_url, "");

        state.cover_url =
            "http://10.0.0.1:7091/cors-proxy?url=https%3A%2F%2Fexample.com%2Fa.jpg&id=x".into();
        let before = state.cover_url.clone();
        apply_cover_proxy(&mut state, Some("http://192.168.1.2:7091"));
        assert_eq!(state.cover_url, before);
    }
}
