//! Per-zone Cast playback state and transport routing.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::Result;
use oxicast::{CastClient, CastEvent, PlayerState as CastPlayerState};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::loxone_config::ZoneDescriptor;
use crate::player_state::PlayerState;
use crate::player_state::{Mode, Playerid};
use crate::radio::{build_radio_player_state, resolve_radio_audiopath, ResolvedRadioStream};

use super::device::CastDeviceInfo;
use super::session::{
    cast_load_stream, cast_pause, cast_play, cast_set_volume, cast_stop, connect_zone_cast,
};
use super::sync::{
    cast_media_session_ended, cast_player_state_to_mode, cast_receiver_lost_media_app,
    cast_volume_to_loxone, CastRemoteUpdate,
};
use tracing::{info, warn};

struct RadioZoneState {
    resolved: ResolvedRadioStream,
    _cast_device: CastDeviceInfo,
    client: CastClient,
    _listener: JoinHandle<()>,
    mode: Mode,
    volume: u8,
    muted: bool,
    /// Suppress stale Cast receiver echoes after a Loxone-initiated volume step.
    local_volume_command_at: Option<std::time::Instant>,
    local_volume_command_level: Option<u8>,
}

/// Ignore mismatched Cast volume echoes for this long after a local volume command.
const LOCAL_VOLUME_ECHO_GUARD: Duration = Duration::from_secs(2);

async fn send_session_ended(
    player_id: usize,
    update_tx: &mpsc::Sender<CastRemoteUpdate>,
    reason: &str,
    force: bool,
) -> bool {
    info!(player_id, reason, force, "cast radio session ended");
    update_tx
        .send(CastRemoteUpdate::SessionEnded { player_id, force })
        .await
        .is_ok()
}

fn spawn_cast_listener(
    player_id: usize,
    client: CastClient,
    update_tx: mpsc::Sender<CastRemoteUpdate>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut playback_seen = false;
        let mut session_ended = false;
        while let Some(event) = client.next_event().await {
            match event {
                CastEvent::ReceiverStatusChanged(status) => {
                    if playback_seen
                        && cast_receiver_lost_media_app(&status)
                        && send_session_ended(
                            player_id,
                            &update_tx,
                            "receiver lost Default Media Receiver app",
                            false,
                        )
                        .await
                    {
                        session_ended = true;
                        break;
                    }
                    let (level, _) =
                        cast_volume_to_loxone(status.volume.level, status.volume.muted);
                    if update_tx
                        .send(CastRemoteUpdate::Volume {
                            player_id,
                            level,
                            muted: status.volume.muted,
                        })
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                CastEvent::MediaStatusChanged(status) => {
                    if matches!(
                        status.player_state,
                        CastPlayerState::Playing
                            | CastPlayerState::Paused
                            | CastPlayerState::Buffering
                    ) {
                        playback_seen = true;
                    }
                    if playback_seen
                        && cast_media_session_ended(&status)
                        && send_session_ended(
                            player_id,
                            &update_tx,
                            "media idle with reason",
                            false,
                        )
                        .await
                    {
                        session_ended = true;
                        break;
                    }
                    if let Some(mode) = cast_player_state_to_mode(status.player_state) {
                        if update_tx
                            .send(CastRemoteUpdate::Transport { player_id, mode })
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    // MediaStatus volume is the stream session level (defaults to 1.0), not
                    // receiver volume — do not mirror it to Loxone (causes pause → 100% bug).
                }
                CastEvent::MediaSessionEnded { .. } => {
                    if send_session_ended(player_id, &update_tx, "MediaSessionEnded", false).await {
                        session_ended = true;
                        break;
                    }
                }
                CastEvent::Disconnected(reason) => {
                    warn!(player_id, ?reason, "cast disconnected; clearing radio zone");
                    let _ = send_session_ended(player_id, &update_tx, "disconnected", true).await;
                    session_ended = true;
                    break;
                }
                _ => {}
            }
        }
        if !session_ended {
            let _ = send_session_ended(player_id, &update_tx, "listener exit", true).await;
        }
    })
}

pub struct CastPlaybackManager {
    zones: HashMap<usize, RadioZoneState>,
    discovery_timeout: Duration,
    update_tx: mpsc::Sender<CastRemoteUpdate>,
}

impl CastPlaybackManager {
    pub fn new(update_tx: mpsc::Sender<CastRemoteUpdate>) -> Self {
        Self {
            zones: HashMap::new(),
            discovery_timeout: Duration::from_secs(3),
            update_tx,
        }
    }

    /// Audiopath of the station the zone's Cast radio session plays.
    pub fn radio_audiopath(&self, player_id: usize) -> Option<String> {
        self.zones
            .get(&player_id)
            .map(|zone| zone.resolved.audiopath.clone())
    }

    pub fn zone_is_radio(&self, player_id: usize) -> bool {
        self.zones.contains_key(&player_id)
    }

    /// Remove zone state immediately; disconnect Cast in the background (non-blocking).
    pub fn take_zone_on_session_end(&mut self, player_id: usize) -> bool {
        let Some(zone) = self.zones.remove(&player_id) else {
            return false;
        };
        zone._listener.abort();
        tokio::spawn(async move {
            if let Err(e) = zone.client.disconnect().await {
                warn!(player_id, error = %e, "cast disconnect after session end failed");
            }
        });
        true
    }

    pub async fn play_radio(
        &mut self,
        player_id: Playerid,
        audiopath: &str,
        zones: &[ZoneDescriptor],
    ) -> Result<PlayerState> {
        let resolved = resolve_radio_audiopath(audiopath).await?;
        let title = resolved.title.clone();
        let stream_url = resolved.stream_url.clone();
        let content_type = resolved.content_type.clone();

        let (cast_device, client) = connect_zone_cast(player_id.0, self.discovery_timeout).await?;
        let listener = spawn_cast_listener(player_id.0, client.clone(), self.update_tx.clone());
        cast_load_stream(&client, &stream_url, &title, &content_type).await?;

        info!(
            player_id = player_id.0,
            station = %resolved.audiopath,
            cast = %cast_device.friendly_name,
            "radio playback started via cast"
        );

        let zone_name = zones
            .iter()
            .find(|z| z.player_id == player_id.0)
            .map(|z| z.name.as_str());

        let (volume, muted) = {
            let target = crate::zone_volume::startup_volume(player_id);
            let _ = crate::zone_volume::set_level(player_id, target);
            (target, false)
        };

        cast_set_volume(&client, volume).await?;

        let state = build_radio_player_state(
            player_id,
            zone_name,
            &resolved,
            Mode::Play,
            display_volume(volume, muted),
        );

        self.zones.insert(
            player_id.0,
            RadioZoneState {
                resolved,
                _cast_device: cast_device,
                client,
                _listener: listener,
                mode: Mode::Play,
                volume,
                muted,
                local_volume_command_at: None,
                local_volume_command_level: None,
            },
        );
        Ok(state)
    }

    pub fn player_state(
        &self,
        player_id: Playerid,
        zone_name: Option<&str>,
    ) -> Option<PlayerState> {
        let zone = self.zones.get(&player_id.0)?;
        Some(build_radio_player_state(
            player_id,
            zone_name,
            &zone.resolved,
            zone.mode.clone(),
            display_volume(zone.volume, zone.muted),
        ))
    }

    pub fn current_volume(&self, player_id: Playerid) -> Option<(u8, bool)> {
        self.zones.get(&player_id.0).map(|z| (z.volume, z.muted))
    }

    /// Returns true when a Cast receiver echo should be dropped (stale vs last local command).
    pub fn is_stale_volume_echo(&self, player_id: usize, level: u8, _muted: bool) -> bool {
        let Some(zone) = self.zones.get(&player_id) else {
            return true;
        };
        volume_echo_is_stale(
            zone.local_volume_command_at,
            zone.local_volume_command_level,
            level,
            LOCAL_VOLUME_ECHO_GUARD,
        )
    }

    pub fn clear_local_volume_guard(&mut self, player_id: usize) {
        if let Some(zone) = self.zones.get_mut(&player_id) {
            zone.local_volume_command_at = None;
            zone.local_volume_command_level = None;
        }
    }

    /// Clear the echo guard when Cast confirms the last local volume command.
    pub fn confirm_volume_echo(&mut self, player_id: usize, level: u8, _muted: bool) {
        let should_clear = self
            .zones
            .get(&player_id)
            .and_then(|z| z.local_volume_command_level)
            .is_some_and(|expected| expected == level);
        if should_clear {
            self.clear_local_volume_guard(player_id);
        }
    }

    pub fn apply_remote_volume(&mut self, player_id: usize, level: u8, muted: bool) -> bool {
        let Some(zone) = self.zones.get_mut(&player_id) else {
            return false;
        };
        if zone.volume == level && zone.muted == muted {
            return false;
        }
        zone.volume = level;
        zone.muted = muted;
        true
    }

    pub fn apply_remote_mode(&mut self, player_id: usize, mode: Mode) -> bool {
        let Some(zone) = self.zones.get_mut(&player_id) else {
            return false;
        };
        if zone.mode == mode {
            return false;
        }
        zone.mode = mode;
        true
    }

    pub async fn transport_play(&mut self, player_id: Playerid) -> Result<()> {
        let Some(zone) = self.zones.get_mut(&player_id.0) else {
            return Ok(());
        };
        cast_play(&zone.client).await?;
        zone.mode = Mode::Play;
        Ok(())
    }

    pub async fn transport_pause(&mut self, player_id: Playerid) -> Result<()> {
        let Some(zone) = self.zones.get_mut(&player_id.0) else {
            return Ok(());
        };
        cast_pause(&zone.client).await?;
        zone.mode = Mode::Pause;
        Ok(())
    }

    pub async fn transport_volume(&mut self, player_id: Playerid, level: u8) -> Result<()> {
        let Some(zone) = self.zones.get_mut(&player_id.0) else {
            return Ok(());
        };
        let level = level.clamp(0, 100);
        zone.local_volume_command_at = Some(std::time::Instant::now());
        zone.local_volume_command_level = Some(level);
        cast_set_volume(&zone.client, level).await?;
        zone.volume = level;
        zone.muted = false;
        Ok(())
    }

    /// Stop the Cast radio sessions of `player_ids` concurrently (zones without one are
    /// skipped) and wait until all are stopped.
    pub async fn stop_zones(&mut self, player_ids: &[Playerid]) {
        let mut stops = tokio::task::JoinSet::new();
        for player_id in player_ids {
            if let Some(zone) = self.zones.remove(&player_id.0) {
                zone._listener.abort();
                let player_id = player_id.0;
                stops.spawn(async move {
                    if let Err(e) = cast_stop(&zone.client).await {
                        warn!(player_id, error = %e, "cast stop failed");
                    }
                });
            }
        }
        while stops.join_next().await.is_some() {}
    }

    pub async fn stop_zone(&mut self, player_id: Playerid) -> Result<()> {
        if let Some(zone) = self.zones.remove(&player_id.0) {
            zone._listener.abort();
            if let Err(e) = cast_stop(&zone.client).await {
                warn!(player_id = player_id.0, error = %e, "cast stop failed");
            }
        }
        Ok(())
    }
}

fn display_volume(stored: u8, muted: bool) -> f32 {
    if muted {
        0.0
    } else {
        stored as f32
    }
}

fn volume_echo_is_stale(
    local_at: Option<std::time::Instant>,
    local_level: Option<u8>,
    echo_level: u8,
    guard: Duration,
) -> bool {
    let Some(at) = local_at else {
        return false;
    };
    let Some(expected) = local_level else {
        return false;
    };
    if at.elapsed() > guard {
        return false;
    }
    echo_level != expected
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_echo_is_stale_when_level_mismatches_within_guard() {
        let at = std::time::Instant::now();
        assert!(volume_echo_is_stale(
            Some(at),
            Some(58),
            60,
            Duration::from_secs(2)
        ));
    }

    #[test]
    fn volume_echo_is_accepted_when_level_matches_within_guard() {
        let at = std::time::Instant::now();
        assert!(!volume_echo_is_stale(
            Some(at),
            Some(58),
            58,
            Duration::from_secs(2)
        ));
    }

    #[test]
    fn volume_echo_is_accepted_after_guard_expires() {
        let at = std::time::Instant::now() - Duration::from_secs(3);
        assert!(!volume_echo_is_stale(
            Some(at),
            Some(58),
            60,
            Duration::from_secs(2)
        ));
    }

    #[test]
    fn take_zone_on_session_end_returns_false_when_empty() {
        let (tx, _rx) = mpsc::channel(4);
        let mut manager = CastPlaybackManager::new(tx);
        assert!(!manager.take_zone_on_session_end(3));
    }

    #[test]
    fn zone_is_radio_false_for_unknown_player() {
        let (tx, _rx) = mpsc::channel(4);
        let manager = CastPlaybackManager::new(tx);
        assert!(!manager.zone_is_radio(99));
    }
}
