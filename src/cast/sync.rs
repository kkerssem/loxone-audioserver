//! Cast receiver → Loxone sync (volume, transport, session end).

use crate::player_state::Mode;
use oxicast::{CastApp, MediaStatus, PlayerState as CastPlayerState, ReceiverStatus};

/// Remote change reported by a per-zone Cast listener.
#[derive(Debug, Clone, PartialEq)]
pub enum CastRemoteUpdate {
    Volume {
        player_id: usize,
        level: u8,
        muted: bool,
    },
    Transport {
        player_id: usize,
        mode: Mode,
    },
    SessionEnded {
        player_id: usize,
        /// Which radio session ended (see `CastPlaybackManager::radio_session`), so a late
        /// report from a replaced session cannot end the one that replaced it.
        session: u64,
        /// Bypass registry grace when the Cast link is definitely gone.
        force: bool,
    },
}

impl CastRemoteUpdate {
    pub fn player_id(&self) -> usize {
        match self {
            Self::Volume { player_id, .. }
            | Self::Transport { player_id, .. }
            | Self::SessionEnded { player_id, .. } => *player_id,
        }
    }
}

/// Map Cast 0.0–1.0 volume to stored level (0–100) and Loxone display level.
pub fn cast_volume_to_loxone(level: f32, muted: bool) -> (u8, f32) {
    let stored = (level.clamp(0.0, 1.0) * 100.0).round() as u8;
    let display = if muted { 0.0 } else { stored as f32 };
    (stored, display)
}

/// Map Cast player state to Loxone mode. Returns `None` for idle (session teardown uses
/// dedicated session-end detection, not transport updates).
pub fn cast_player_state_to_mode(state: CastPlayerState) -> Option<Mode> {
    match state {
        CastPlayerState::Playing | CastPlayerState::Buffering => Some(Mode::Play),
        CastPlayerState::Paused => Some(Mode::Pause),
        CastPlayerState::Idle => None,
        _ => None,
    }
}

/// Idle media status with a reason — our Cast radio session is over.
pub fn cast_media_session_ended(status: &MediaStatus) -> bool {
    status.player_state == CastPlayerState::Idle
        && status.media_session_id > 0
        && status.idle_reason.is_some()
}

/// Default Media Receiver no longer listed — user switched away on the device.
pub fn cast_receiver_lost_media_app(status: &ReceiverStatus) -> bool {
    let app_id = CastApp::DefaultMediaReceiver.app_id();
    !status.applications.iter().any(|app| app.app_id == app_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cast_volume_maps_level_and_muted_display() {
        assert_eq!(cast_volume_to_loxone(0.34, false), (34, 34.0));
        assert_eq!(cast_volume_to_loxone(0.34, true), (34, 0.0));
        assert_eq!(cast_volume_to_loxone(1.0, false), (100, 100.0));
    }

    #[test]
    fn cast_player_state_maps_transport_modes() {
        assert_eq!(
            cast_player_state_to_mode(CastPlayerState::Playing),
            Some(Mode::Play)
        );
        assert_eq!(
            cast_player_state_to_mode(CastPlayerState::Buffering),
            Some(Mode::Play)
        );
        assert_eq!(
            cast_player_state_to_mode(CastPlayerState::Paused),
            Some(Mode::Pause)
        );
        assert_eq!(cast_player_state_to_mode(CastPlayerState::Idle), None);
    }

    #[test]
    fn cast_receiver_lost_media_app_when_app_list_empty() {
        use oxicast::ReceiverStatus;
        use oxicast::Volume;
        let status = ReceiverStatus {
            volume: Volume::new(0.5),
            applications: vec![],
            is_active_input: true,
            is_stand_by: false,
        };
        assert!(cast_receiver_lost_media_app(&status));
    }

    #[test]
    fn remote_update_player_id() {
        let updates = [
            CastRemoteUpdate::Volume {
                player_id: 3,
                level: 10,
                muted: false,
            },
            CastRemoteUpdate::Transport {
                player_id: 3,
                mode: Mode::Pause,
            },
            CastRemoteUpdate::SessionEnded {
                player_id: 3,
                session: 1,
                force: false,
            },
        ];
        assert!(updates.iter().all(|u| u.player_id() == 3));
    }
}
