//! Per-zone audio preferences (loudness, etc.) stored in memory for the process lifetime.

use crate::player_state::Playerid;
use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};
use tracing::debug;

static LOUDNESS: LazyLock<RwLock<HashMap<usize, bool>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

pub fn get_loudness(playerid: Playerid) -> bool {
    LOUDNESS
        .read()
        .expect("loudness prefs lock")
        .get(&playerid.0)
        .copied()
        .unwrap_or(false)
}

pub fn set_loudness(playerid: Playerid, enabled: bool) {
    LOUDNESS
        .write()
        .expect("loudness prefs lock")
        .insert(playerid.0, enabled);
    debug!(
        player_id = playerid.0,
        enabled, "loudness preference stored (not applied to speaker playback)"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loudness_defaults_off() {
        let id = Playerid(99_002);
        assert!(!get_loudness(id));
        set_loudness(id, true);
        assert!(get_loudness(id));
        set_loudness(id, false);
        assert!(!get_loudness(id));
    }
}
