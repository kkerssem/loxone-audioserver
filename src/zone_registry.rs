//! Authoritative per-zone playback state: ownership, invariants, dedup, normalization.
//!
//! Sync only — no tokio, Cast, speaker, or filesystem I/O. Only the coordinator task mutates
//! [`ZoneRegistry`]. Producers commit via [`crate::zone_publish`] (never `IoEvent::State` directly).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::player_state::{Mode, PlayerState, Playerid};
use crate::zone_volume;

/// Dedup fingerprint for fan-out (omits scrubber `time`).
pub type FanoutDedupKey = (usize, String, i64, String, String, String, i32);

const REWIND_UI_TIME_SECS: i64 = 1;

/// Who owns playback for a Loxone zone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlaybackSource {
    Idle,
    /// Playback started outside the audioserver (speaker app, Spotify Connect, AirPlay …).
    External,
    CastRadio {
        station_id: String,
    },
}

/// Incremental updates from Cast, speaker backends, or transport handlers.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug)]
pub enum ZoneUpdate {
    SetSource {
        player_id: Playerid,
        source: PlaybackSource,
    },
    SetPlayerState {
        player_id: Playerid,
        state: PlayerState,
        /// When set, updates ownership (e.g. periodic speaker publish).
        source: Option<PlaybackSource>,
    },
    #[allow(dead_code)]
    SetVolume {
        player_id: Playerid,
        level: u8,
        muted: bool,
    },
    Clear {
        player_id: Playerid,
        /// When true, bypass Cast session-end grace (disconnect / dead Cast client).
        force: bool,
    },
    /// Drop fan-out dedup keys so the next [`SetPlayerState`] reaches WebSocket clients.
    #[allow(dead_code)]
    ClearFanoutDedup { player_ids: Vec<usize> },
}

#[derive(Clone, Debug)]
struct ZoneEntry {
    source: PlaybackSource,
    state: PlayerState,
    revision: u64,
    last_cast_activity: Option<Instant>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplyOutcome {
    Changed,
    Unchanged,
    Ignored,
}

/// Single writer for zone snapshots inside the coordinator task.
#[derive(Clone, Debug, Default)]
pub struct ZoneRegistry {
    zones: HashMap<usize, ZoneEntry>,
    dedup: HashMap<usize, FanoutDedupKey>,
    cast_session_end_grace: Duration,
}

impl ZoneRegistry {
    pub fn new() -> Self {
        Self {
            zones: HashMap::new(),
            dedup: HashMap::new(),
            cast_session_end_grace: Duration::from_secs(2),
        }
    }

    #[cfg(test)]
    pub fn with_cast_session_end_grace(grace: Duration) -> Self {
        Self {
            cast_session_end_grace: grace,
            ..Self::new()
        }
    }

    pub fn source(&self, player_id: Playerid) -> PlaybackSource {
        self.zones
            .get(&player_id.0)
            .map(|e| e.source.clone())
            .unwrap_or(PlaybackSource::Idle)
    }

    #[allow(dead_code)]
    pub fn revision(&self, player_id: Playerid) -> u64 {
        self.zones
            .get(&player_id.0)
            .map(|e| e.revision)
            .unwrap_or(0)
    }

    pub fn snapshot(&self, player_id: Playerid) -> PlayerState {
        self.zones
            .get(&player_id.0)
            .map(|e| e.state.clone())
            .unwrap_or_else(|| idle_shell(player_id))
    }

    #[allow(dead_code)]
    pub fn snapshots(&self, player_ids: &[Playerid]) -> Vec<PlayerState> {
        player_ids.iter().map(|&id| self.snapshot(id)).collect()
    }

    pub fn is_cast_owned(&self, player_id: Playerid) -> bool {
        matches!(self.source(player_id), PlaybackSource::CastRadio { .. })
    }

    /// Allow the next publish for these zones to fan out even when metadata matches prior dedup.
    pub fn clear_fanout_dedup(&mut self, player_ids: &[usize]) {
        for &id in player_ids {
            self.dedup.remove(&id);
        }
    }

    /// Clear a zone to idle; `force` bypasses Cast session-end grace.
    pub fn clear_zone(&mut self, player_id: Playerid, force: bool) -> ApplyOutcome {
        self.apply(ZoneUpdate::Clear { player_id, force })
    }

    pub fn apply(&mut self, update: ZoneUpdate) -> ApplyOutcome {
        match update {
            ZoneUpdate::SetSource { player_id, source } => {
                let entry = self.ensure_entry(player_id);
                if entry.source != source {
                    entry.source = source;
                    entry.revision += 1;
                    ApplyOutcome::Changed
                } else {
                    ApplyOutcome::Unchanged
                }
            }
            ZoneUpdate::SetPlayerState {
                player_id,
                mut state,
                source,
            } => {
                if matches!(self.source(player_id), PlaybackSource::CastRadio { .. }) {
                    return ApplyOutcome::Ignored;
                }
                let prior = self.snapshot(player_id);
                if state.title.is_empty() {
                    // Transient metadata loss during live playback only — not handoff-away idle (Stop).
                    if matches!(state.mode, Mode::Play | Mode::Pause)
                        && !prior.title.is_empty()
                        && matches!(prior.mode, Mode::Play | Mode::Pause)
                    {
                        state.merge_playback_metadata(&prior);
                    }
                } else if matches!(state.mode, Mode::Play | Mode::Pause)
                    && ((!prior.audiopath.is_empty() && prior.audiopath != state.audiopath)
                        || (!prior.title.is_empty() && prior.title != state.title))
                {
                    state.time = 0;
                }
                if state.audiopath.is_empty()
                    && !prior.audiopath.is_empty()
                    && matches!(state.mode, Mode::Play | Mode::Pause)
                    && !state.title.is_empty()
                    && prior.title == state.title
                {
                    state.audiopath = prior.audiopath.clone();
                }
                state.playerid = player_id;
                apply_display_volume(&mut state, player_id, &self.source(player_id));
                let entry = self.ensure_entry(player_id);
                if let Some(src) = source {
                    entry.source = src;
                } else if matches!(state.mode, Mode::Stop) {
                    entry.source = PlaybackSource::Idle;
                } else if matches!(state.mode, Mode::Play | Mode::Pause)
                    && matches!(entry.source, PlaybackSource::Idle)
                {
                    entry.source = PlaybackSource::External;
                }
                if entry.state == state {
                    return ApplyOutcome::Unchanged;
                }
                entry.state = state;
                entry.revision += 1;
                ApplyOutcome::Changed
            }
            ZoneUpdate::SetVolume {
                player_id,
                level,
                muted,
            } => {
                // Create the entry before persisting: a new entry starts from the stored volume,
                // so creating it afterwards would already hold the new level and look unchanged.
                self.ensure_entry(player_id);
                let _ = zone_volume::set(player_id, level, muted);
                let entry = self.ensure_entry(player_id);
                let vol = zone_volume::display_volume(player_id);
                if (entry.state.volume - vol).abs() < f32::EPSILON {
                    return ApplyOutcome::Unchanged;
                }
                entry.state.volume = vol;
                entry.revision += 1;
                ApplyOutcome::Changed
            }
            ZoneUpdate::Clear { player_id, force } => {
                let Some(entry) = self.zones.get(&player_id.0) else {
                    return ApplyOutcome::Unchanged;
                };
                if !force
                    && matches!(entry.source, PlaybackSource::CastRadio { .. })
                    && entry
                        .last_cast_activity
                        .is_some_and(|t| t.elapsed() < self.cast_session_end_grace)
                {
                    return ApplyOutcome::Ignored;
                }
                if matches!(entry.source, PlaybackSource::Idle)
                    && entry.state.mode == Mode::Stop
                    && entry.state.title.is_empty()
                {
                    return ApplyOutcome::Unchanged;
                }
                let idle = idle_shell(player_id);
                let entry = self.zones.get_mut(&player_id.0).expect("zone exists");
                entry.source = PlaybackSource::Idle;
                entry.state = idle;
                entry.last_cast_activity = None;
                entry.revision += 1;
                ApplyOutcome::Changed
            }
            ZoneUpdate::ClearFanoutDedup { player_ids } => {
                self.clear_fanout_dedup(player_ids.as_slice());
                ApplyOutcome::Unchanged
            }
        }
    }

    /// Apply update; returns wire state to fan-out when publish rules allow.
    pub fn apply_for_publish(&mut self, update: ZoneUpdate) -> (ApplyOutcome, Option<PlayerState>) {
        if matches!(update, ZoneUpdate::ClearFanoutDedup { .. }) {
            let outcome = self.apply(update);
            return (outcome, None);
        }
        let player_id = player_id_from_update(&update);
        let prior = self.snapshot(player_id);
        let outcome = self.apply(update);
        if !matches!(outcome, ApplyOutcome::Changed) {
            return (outcome, None);
        }
        let state = self.snapshot(player_id);
        if self.should_publish(&prior, &state) {
            self.dedup.insert(player_id.0, state_broadcast_key(&state));
            (outcome, Some(state))
        } else {
            (ApplyOutcome::Unchanged, None)
        }
    }

    /// Commit a fully-built player state; returns `Some` when clients should receive `audio_event`.
    pub fn commit_state(
        &mut self,
        mut state: PlayerState,
        source: PlaybackSource,
    ) -> Option<PlayerState> {
        let player_id = state.playerid;
        apply_display_volume(&mut state, player_id, &source);
        let prior = self.snapshot(player_id);
        let entry = self.ensure_entry(player_id);
        entry.source = source.clone();
        if matches!(source, PlaybackSource::CastRadio { .. }) {
            entry.last_cast_activity = Some(Instant::now());
        }
        if prior == state {
            entry.state = state;
            return None;
        }
        entry.state = state.clone();
        entry.revision += 1;
        if self.should_publish(&prior, &state) {
            self.dedup.insert(player_id.0, state_broadcast_key(&state));
            Some(state)
        } else {
            None
        }
    }

    fn should_publish(&self, prior: &PlayerState, state: &PlayerState) -> bool {
        let player_id = state.playerid.0;
        if prior.mode != state.mode {
            return true;
        }
        let key = state_broadcast_key(state);
        if should_force_time_fanout(prior, state) {
            return true;
        }
        self.dedup.get(&player_id) != Some(&key)
    }

    fn ensure_entry(&mut self, player_id: Playerid) -> &mut ZoneEntry {
        self.zones.entry(player_id.0).or_insert_with(|| ZoneEntry {
            source: PlaybackSource::Idle,
            state: idle_shell(player_id),
            revision: 0,
            last_cast_activity: None,
        })
    }
}

pub fn state_broadcast_key(state: &PlayerState) -> FanoutDedupKey {
    (
        state.playerid.0,
        format!("{:?}", state.mode),
        i64::from(state.duration),
        state.title.clone(),
        state.cover_url.clone(),
        state.audiopath.clone(),
        state.volume as i32,
    )
}

fn same_track_for_time_fanout(prior: &PlayerState, state: &PlayerState) -> bool {
    if !state.audiopath.is_empty() && prior.audiopath == state.audiopath {
        return true;
    }
    !state.title.is_empty() && prior.title == state.title
}

pub fn should_force_time_fanout(prior: &PlayerState, state: &PlayerState) -> bool {
    if matches!(state.mode, Mode::Play | Mode::Pause)
        && state.time == REWIND_UI_TIME_SECS
        && prior.time > 5
        && same_track_for_time_fanout(prior, state)
    {
        return true;
    }
    if !matches!(state.mode, Mode::Play | Mode::Pause) || state.duration == 0 {
        return false;
    }
    if prior.time == 0 && state.time >= 2 {
        return true;
    }
    if state.time < 2 || !same_track_for_time_fanout(prior, state) {
        return false;
    }
    if prior.time / 10 != state.time / 10 {
        return true;
    }
    (state.time - prior.time).abs() >= 2
}

fn player_id_from_update(update: &ZoneUpdate) -> Playerid {
    match update {
        ZoneUpdate::SetSource { player_id, .. }
        | ZoneUpdate::SetPlayerState { player_id, .. }
        | ZoneUpdate::SetVolume { player_id, .. }
        | ZoneUpdate::Clear { player_id, .. } => *player_id,
        ZoneUpdate::ClearFanoutDedup { .. } => Playerid(0),
    }
}

fn idle_shell(player_id: Playerid) -> PlayerState {
    let mut state = PlayerState {
        playerid: player_id,
        ..Default::default()
    };
    apply_display_volume(&mut state, player_id, &PlaybackSource::Idle);
    state
}

fn apply_display_volume(state: &mut PlayerState, player_id: Playerid, source: &PlaybackSource) {
    if matches!(source, PlaybackSource::CastRadio { .. })
        && matches!(state.mode, Mode::Play | Mode::Pause)
    {
        return;
    }
    state.volume = zone_volume::display_volume(player_id);
}

/// Encode connect-time `audio_event` payloads (shared by 7091 and 7095).
pub fn encode_audio_event_payload(state: &PlayerState) -> Result<String, serde_json::Error> {
    serde_json::to_string(&serde_json::json!({ "audio_event": [state] }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::broadcast;

    use crate::remote::IoEvent;

    fn playing_state(player_id: usize, title: &str, audiopath: &str) -> PlayerState {
        PlayerState {
            playerid: Playerid(player_id),
            mode: Mode::Play,
            title: title.to_string(),
            audiopath: audiopath.to_string(),
            duration: 200,
            volume: 50.0,
            ..Default::default()
        }
    }

    fn recv_state(rx: &mut broadcast::Receiver<IoEvent>) -> Option<PlayerState> {
        loop {
            match rx.try_recv() {
                Ok(IoEvent::State(s)) => return Some(s),
                Ok(_) => continue,
                Err(broadcast::error::TryRecvError::Empty) => return None,
                Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(broadcast::error::TryRecvError::Closed) => return None,
            }
        }
    }

    fn fanout(reg: &mut ZoneRegistry, state: PlayerState, events: &broadcast::Sender<IoEvent>) {
        if let Some(s) = reg.commit_state(state, PlaybackSource::External) {
            let _ = events.send(IoEvent::State(s));
        }
    }

    #[test]
    fn set_player_state_merges_empty_title_from_prior() {
        let mut reg = ZoneRegistry::new();
        reg.commit_state(
            playing_state(1, "Song", "qobuz:1"),
            PlaybackSource::External,
        );
        let mut partial = playing_state(1, "", "qobuz:1");
        partial.title.clear();
        reg.apply(ZoneUpdate::SetPlayerState {
            player_id: Playerid(1),
            state: partial,
            source: None,
        });
        assert_eq!(reg.snapshot(Playerid(1)).title, "Song");
    }

    #[test]
    fn set_player_state_scrubs_time_on_track_change() {
        let mut reg = ZoneRegistry::new();
        reg.commit_state(
            playing_state(1, "First", "qobuz:1"),
            PlaybackSource::External,
        );
        let mut next = playing_state(1, "Second", "qobuz:2");
        next.time = 164;
        reg.apply(ZoneUpdate::SetPlayerState {
            player_id: Playerid(1),
            state: next,
            source: None,
        });
        assert_eq!(reg.snapshot(Playerid(1)).time, 0);
    }

    #[test]
    fn clear_zone_force_bypasses_cast_grace() {
        let mut reg = ZoneRegistry::with_cast_session_end_grace(Duration::from_secs(60));
        reg.commit_state(
            playing_state(2, "FM", "tunein:9"),
            PlaybackSource::CastRadio {
                station_id: "tunein:9".into(),
            },
        );
        assert_eq!(reg.clear_zone(Playerid(2), false), ApplyOutcome::Ignored);
        assert_eq!(reg.clear_zone(Playerid(2), true), ApplyOutcome::Changed);
        assert_eq!(reg.snapshot(Playerid(2)).mode, Mode::Stop);
    }

    #[test]
    fn set_source_bumps_revision() {
        let mut reg = ZoneRegistry::new();
        assert_eq!(
            reg.apply(ZoneUpdate::SetSource {
                player_id: Playerid(1),
                source: PlaybackSource::CastRadio {
                    station_id: "tunein:1".into(),
                },
            }),
            ApplyOutcome::Changed
        );
        assert_eq!(reg.revision(Playerid(1)), 1);
    }

    #[test]
    fn qobuz_state_ignored_when_cast_owns_zone() {
        let mut reg = ZoneRegistry::new();
        let (tx, _) = broadcast::channel::<IoEvent>(4);
        reg.commit_state(
            playing_state(4, "Radio", "tunein:station:1"),
            PlaybackSource::CastRadio {
                station_id: "tunein:station:1".into(),
            },
        );
        let outcome = reg.apply(ZoneUpdate::SetPlayerState {
            player_id: Playerid(4),
            state: playing_state(4, "Qobuz Track", "qobuz:track:1"),
            source: Some(PlaybackSource::External),
        });
        assert_eq!(outcome, ApplyOutcome::Ignored);
        assert_eq!(reg.snapshot(Playerid(4)).title, "Radio");
        let _ = tx;
    }

    #[test]
    fn clear_ignored_during_cast_grace_period() {
        let mut reg = ZoneRegistry::with_cast_session_end_grace(Duration::from_secs(5));
        reg.commit_state(
            playing_state(2, "FM", "tunein:station:9"),
            PlaybackSource::CastRadio {
                station_id: "tunein:station:9".into(),
            },
        );
        assert_eq!(
            reg.apply(ZoneUpdate::Clear {
                player_id: Playerid(2),
                force: false,
            }),
            ApplyOutcome::Ignored
        );
        assert_eq!(reg.snapshot(Playerid(2)).title, "FM");
    }

    #[test]
    fn clear_after_grace_clears_to_idle() {
        let mut reg = ZoneRegistry::with_cast_session_end_grace(Duration::from_millis(0));
        reg.commit_state(
            playing_state(2, "FM", "tunein:station:9"),
            PlaybackSource::CastRadio {
                station_id: "tunein:station:9".into(),
            },
        );
        assert_eq!(
            reg.apply(ZoneUpdate::Clear {
                player_id: Playerid(2),
                force: false,
            }),
            ApplyOutcome::Changed
        );
        assert_eq!(reg.snapshot(Playerid(2)).mode, Mode::Stop);
        assert!(reg.snapshot(Playerid(2)).title.is_empty());
    }

    #[test]
    fn transport_change_bumps_revision_and_publishes() {
        let (tx, mut rx) = broadcast::channel::<IoEvent>(8);
        let mut reg = ZoneRegistry::new();
        let playing = playing_state(1, "Song", "qobuz:1");
        if let Some(s) = reg.commit_state(playing, PlaybackSource::External) {
            let _ = tx.send(IoEvent::State(s));
        }
        let _ = recv_state(&mut rx);
        let mut paused = playing_state(1, "Song", "qobuz:1");
        paused.mode = Mode::Pause;
        let published = reg
            .commit_state(paused, PlaybackSource::External)
            .expect("pause fan-out");
        let _ = tx.send(IoEvent::State(published.clone()));
        assert_eq!(published.mode, Mode::Pause);
        assert!(reg.revision(Playerid(1)) >= 2);
        assert_eq!(recv_state(&mut rx).map(|s| s.mode), Some(Mode::Pause));
    }

    #[test]
    fn dedup_suppresses_identical_publish() {
        let (tx, mut rx) = broadcast::channel(8);
        let mut reg = ZoneRegistry::new();
        let state = playing_state(3, "Same", "qobuz:3");
        reg.commit_state(state.clone(), PlaybackSource::External);
        fanout(&mut reg, state.clone(), &tx);
        let _ = recv_state(&mut rx);
        assert!(reg
            .commit_state(state.clone(), PlaybackSource::External)
            .is_none());
        assert!(reg.commit_state(state, PlaybackSource::External).is_none());
        assert!(recv_state(&mut rx).is_none());
    }

    #[test]
    fn clear_fanout_dedup_allows_identical_republish() {
        let mut reg = ZoneRegistry::new();
        let mut state = playing_state(3, "Same", "qobuz:3");
        state.time = 50;
        reg.commit_state(state.clone(), PlaybackSource::External);
        state.time = 51;
        let (_, suppressed) = reg.apply_for_publish(ZoneUpdate::SetPlayerState {
            player_id: Playerid(3),
            state: state.clone(),
            source: Some(PlaybackSource::External),
        });
        assert!(
            suppressed.is_none(),
            "dedup suppresses position-only update with same metadata key"
        );
        reg.apply(ZoneUpdate::ClearFanoutDedup {
            player_ids: vec![3],
        });
        state.time = 60;
        let (_, republished) = reg.apply_for_publish(ZoneUpdate::SetPlayerState {
            player_id: Playerid(3),
            state,
            source: Some(PlaybackSource::External),
        });
        assert!(republished.is_some(), "handoff republish after dedup clear");
    }

    #[test]
    fn handoff_idle_stop_is_not_merged_back_to_play() {
        use crate::player_state::player_state_for_protocol;

        let mut reg = ZoneRegistry::new();
        reg.commit_state(
            playing_state(4, "Track on Mu-so", "qobuz:track:392410101"),
            PlaybackSource::External,
        );
        let idle = player_state_for_protocol(Playerid(4), Some("Mu-so"), None);
        assert_eq!(idle.mode, Mode::Stop);
        assert!(idle.title.is_empty());

        let (outcome, published) = reg.apply_for_publish(ZoneUpdate::SetPlayerState {
            player_id: Playerid(4),
            state: idle,
            source: Some(PlaybackSource::External),
        });
        assert_eq!(outcome, ApplyOutcome::Changed);
        let published = published.expect("handoff-away idle must fan out");
        assert_eq!(published.mode, Mode::Stop);
        assert!(published.title.is_empty());
        assert!(published.audiopath.is_empty());
    }

    #[test]
    fn mode_change_always_publishes_even_when_dedup_matches() {
        let mut reg = ZoneRegistry::new();
        let play = playing_state(1, "Song", "qobuz:1");
        reg.commit_state(play.clone(), PlaybackSource::External);
        let mut stop = play;
        stop.mode = Mode::Stop;
        stop.title.clear();
        let published = reg
            .commit_state(stop, PlaybackSource::Idle)
            .expect("stop after play");
        assert_eq!(published.mode, Mode::Stop);
    }

    #[test]
    fn volume_change_publishes() {
        use crate::config::AudioserverConfigFile;

        let (tx, mut rx) = broadcast::channel(8);
        let mut reg = ZoneRegistry::new();
        reg.commit_state(
            playing_state(1, "Song", "qobuz:1"),
            PlaybackSource::External,
        );
        let snapshot = reg.snapshot(Playerid(1));
        fanout(&mut reg, snapshot, &tx);
        let _ = recv_state(&mut rx);
        // `SetVolume` persists through zone_volume, so it must hold the config test lock too.
        let (outcome, published) =
            AudioserverConfigFile::with_test_cache(AudioserverConfigFile::default(), || {
                let _ = crate::zone_volume::set_level(Playerid(1), 80);
                reg.apply_for_publish(ZoneUpdate::SetVolume {
                    player_id: Playerid(1),
                    level: 80,
                    muted: false,
                })
            });
        assert_eq!(outcome, ApplyOutcome::Changed);
        let published = published.expect("volume fan-out");
        let _ = tx.send(IoEvent::State(published.clone()));
        let got = recv_state(&mut rx).expect("volume event");
        assert!((got.volume - 80.0).abs() < f32::EPSILON);
    }

    #[test]
    fn multi_zone_isolation() {
        let mut reg = ZoneRegistry::new();
        reg.apply(ZoneUpdate::SetPlayerState {
            player_id: Playerid(4),
            state: playing_state(4, "Z4", "qobuz:4"),
            source: Some(PlaybackSource::External),
        });
        assert_eq!(reg.snapshot(Playerid(1)).title, "");
        assert_eq!(reg.snapshot(Playerid(4)).title, "Z4");
    }

    #[test]
    fn snapshots_returns_all_requested() {
        let mut reg = ZoneRegistry::new();
        reg.apply(ZoneUpdate::SetPlayerState {
            player_id: Playerid(1),
            state: playing_state(1, "A", "qobuz:a"),
            source: None,
        });
        reg.apply(ZoneUpdate::SetPlayerState {
            player_id: Playerid(2),
            state: playing_state(2, "B", "qobuz:b"),
            source: None,
        });
        let snaps = reg.snapshots(&[Playerid(1), Playerid(2), Playerid(99)]);
        assert_eq!(snaps.len(), 3);
        assert_eq!(snaps[0].title, "A");
        assert_eq!(snaps[1].title, "B");
        assert_eq!(snaps[2].playerid, Playerid(99));
    }

    #[test]
    fn force_clear_bypasses_cast_grace_period() {
        let mut reg = ZoneRegistry::with_cast_session_end_grace(Duration::from_secs(5));
        reg.commit_state(
            playing_state(2, "FM", "tunein:station:9"),
            PlaybackSource::CastRadio {
                station_id: "tunein:station:9".into(),
            },
        );
        assert_eq!(
            reg.apply(ZoneUpdate::Clear {
                player_id: Playerid(2),
                force: true,
            }),
            ApplyOutcome::Changed
        );
        assert_eq!(reg.snapshot(Playerid(2)).mode, Mode::Stop);
        assert!(reg.snapshot(Playerid(2)).title.is_empty());
    }

    #[test]
    fn encode_audio_event_payload_shape() {
        let state = playing_state(1, "T", "qobuz:1");
        let json = encode_audio_event_payload(&state).unwrap();
        assert!(json.contains("\"audio_event\""));
        assert!(json.contains("\"title\":\"T\""));
    }

    #[test]
    fn set_player_state_unchanged_when_equal() {
        let mut reg = ZoneRegistry::new();
        let state = playing_state(1, "X", "qobuz:x");
        assert_eq!(
            reg.apply(ZoneUpdate::SetPlayerState {
                player_id: Playerid(1),
                state: state.clone(),
                source: None,
            }),
            ApplyOutcome::Changed
        );
        assert_eq!(
            reg.apply(ZoneUpdate::SetPlayerState {
                player_id: Playerid(1),
                state,
                source: None,
            }),
            ApplyOutcome::Unchanged
        );
    }

    #[test]
    fn set_source_unchanged_when_same() {
        let mut reg = ZoneRegistry::new();
        let src = PlaybackSource::CastRadio {
            station_id: "custom:1".into(),
        };
        assert_eq!(
            reg.apply(ZoneUpdate::SetSource {
                player_id: Playerid(5),
                source: src.clone(),
            }),
            ApplyOutcome::Changed
        );
        assert_eq!(
            reg.apply(ZoneUpdate::SetSource {
                player_id: Playerid(5),
                source: src,
            }),
            ApplyOutcome::Unchanged
        );
    }

    #[test]
    fn radio_flicker_regression_qobuz_then_session_end() {
        let mut reg = ZoneRegistry::with_cast_session_end_grace(Duration::from_secs(5));
        reg.commit_state(
            playing_state(4, "NPR", "tunein:station:123"),
            PlaybackSource::CastRadio {
                station_id: "tunein:station:123".into(),
            },
        );
        assert_eq!(
            reg.apply(ZoneUpdate::SetPlayerState {
                player_id: Playerid(4),
                state: PlayerState::default(),
                source: Some(PlaybackSource::External),
            }),
            ApplyOutcome::Ignored
        );
        assert_eq!(
            reg.apply(ZoneUpdate::Clear {
                player_id: Playerid(4),
                force: false,
            }),
            ApplyOutcome::Ignored
        );
        assert_eq!(reg.snapshot(Playerid(4)).title, "NPR");
    }

    #[test]
    fn apply_for_publish_emits_on_change() {
        let mut reg = ZoneRegistry::new();
        let (outcome, published) = reg.apply_for_publish(ZoneUpdate::SetPlayerState {
            player_id: Playerid(1),
            state: playing_state(1, "Live", "qobuz:live"),
            source: Some(PlaybackSource::External),
        });
        assert_eq!(outcome, ApplyOutcome::Changed);
        assert_eq!(published.map(|s| s.title), Some("Live".to_string()));
    }

    #[test]
    fn is_cast_owned_reflects_source() {
        let mut reg = ZoneRegistry::new();
        assert!(!reg.is_cast_owned(Playerid(1)));
        reg.apply(ZoneUpdate::SetSource {
            player_id: Playerid(1),
            source: PlaybackSource::CastRadio {
                station_id: "tunein:1".into(),
            },
        });
        assert!(reg.is_cast_owned(Playerid(1)));
    }

    #[test]
    fn broadcast_key_ignores_time() {
        let mut a = playing_state(1, "Song", "qobuz:1");
        a.time = 0;
        let mut b = a.clone();
        b.time = 164;
        assert_eq!(state_broadcast_key(&a), state_broadcast_key(&b));
    }

    #[test]
    fn force_time_fanout_on_coarse_bucket() {
        let mut prior = playing_state(1, "Song", "qobuz:1");
        prior.time = 95;
        let mut state = prior.clone();
        state.time = 102;
        assert!(should_force_time_fanout(&prior, &state));
    }
}
