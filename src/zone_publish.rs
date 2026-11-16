//! Commit zone snapshots to [`ZoneRegistry`] and fan-out `IoEvent::State` (`audio_event`).

use tokio::sync::broadcast;

use crate::player_state::PlayerState;
use crate::remote::IoEvent;
use crate::zone_registry::{ApplyOutcome, PlaybackSource, ZoneRegistry, ZoneUpdate};

/// Send one `IoEvent::State` (`audio_event`) to all connected 7091 / 7095 clients.
///
/// A send error only means no client is subscribed right now, which is normal.
pub fn fanout_player_state(state: PlayerState, events: &broadcast::Sender<IoEvent>) {
    let _ = events.send(IoEvent::State(state));
}

/// Apply a zone update and fan-out `audio_event` when the wire snapshot changed.
pub fn apply_zone_update(
    registry: &mut ZoneRegistry,
    update: ZoneUpdate,
    events: &broadcast::Sender<IoEvent>,
) -> ApplyOutcome {
    let (outcome, published) = registry.apply_for_publish(update);
    if let Some(state) = published {
        fanout_player_state(state, events);
    }
    outcome
}

/// Commit authoritative zone state and fan-out when dedup allows.
pub fn publish_zone_state(
    registry: &mut ZoneRegistry,
    state: PlayerState,
    source: PlaybackSource,
    events: &broadcast::Sender<IoEvent>,
) {
    if let Some(published) = registry.commit_state(state, source) {
        fanout_player_state(published, events);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player_state::{Mode, Playerid};
    use tokio::sync::broadcast;

    fn playing(id: usize, title: &str) -> PlayerState {
        PlayerState {
            playerid: Playerid(id),
            mode: Mode::Play,
            title: title.into(),
            audiopath: format!("qobuz:{id}"),
            ..Default::default()
        }
    }

    #[test]
    fn apply_zone_update_fans_out_on_change() {
        let (tx, mut rx) = broadcast::channel(4);
        let mut registry = ZoneRegistry::new();
        let outcome = apply_zone_update(
            &mut registry,
            ZoneUpdate::SetPlayerState {
                player_id: Playerid(1),
                state: playing(1, "Song"),
                source: Some(PlaybackSource::External),
            },
            &tx,
        );
        assert_eq!(outcome, ApplyOutcome::Changed);
        match rx.try_recv().unwrap() {
            IoEvent::State(s) => assert_eq!(s.title, "Song"),
            other => panic!("expected State, got {other:?}"),
        }
    }

    #[test]
    fn apply_zone_update_ignored_cast_zone_no_fanout() {
        let (tx, mut rx) = broadcast::channel(4);
        let mut registry = ZoneRegistry::new();
        publish_zone_state(
            &mut registry,
            playing(4, "Radio"),
            PlaybackSource::CastRadio {
                station_id: "tunein:1".into(),
            },
            &tx,
        );
        let _ = rx.try_recv();
        let outcome = apply_zone_update(
            &mut registry,
            ZoneUpdate::SetPlayerState {
                player_id: Playerid(4),
                state: playing(4, "Qobuz"),
                source: Some(PlaybackSource::External),
            },
            &tx,
        );
        assert_eq!(outcome, ApplyOutcome::Ignored);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn publish_zone_state_dedup_skips_second_fanout() {
        let (tx, mut rx) = broadcast::channel(4);
        let mut registry = ZoneRegistry::new();
        let state = playing(1, "Same");
        publish_zone_state(&mut registry, state.clone(), PlaybackSource::External, &tx);
        let _ = rx.try_recv();
        publish_zone_state(&mut registry, state, PlaybackSource::External, &tx);
        assert!(rx.try_recv().is_err());
    }
}
