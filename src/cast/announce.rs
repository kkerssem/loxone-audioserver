//! One-off Cast playback of an audio file (a Loxone App voice message) on a zone's Cast device.
//!
//! The receiver volume is raised to the message volume (and a muted receiver unmuted) for the
//! duration of the file and put back afterwards; on these speakers the Cast volume is the
//! speaker volume.

use std::time::Duration;

use anyhow::{bail, Context, Result};
use oxicast::{
    CastApp, CastClient, MediaInfo, MediaMetadata, MediaStatus, PlayerState as CastPlayerState,
    StreamType,
};
use tracing::{debug, info, warn};

use super::session::{cast_set_volume, connect_zone_cast};
use super::sync::cast_volume_to_loxone;

/// Message to play once.
pub struct Announcement<'a> {
    pub url: &'a str,
    pub content_type: &'a str,
    pub title: &'a str,
    /// Lowest volume to play it at (0–100); a louder current volume is kept.
    pub min_volume: Option<u8>,
    /// Give up waiting for the end after this long.
    pub max_wait: Duration,
}

/// How a message went.
#[derive(Debug)]
pub struct AnnounceOutcome {
    /// The receiver accepted the media, i.e. Cast took the speaker over. `false` means the
    /// speaker kept playing what it played (no device, connect or launch failed).
    pub loaded: bool,
    /// Why the message did not play to the end.
    pub error: Option<anyhow::Error>,
}

/// Play `announcement` on the zone's Cast device and return once it finished (or failed /
/// timed out). Volume and mute are restored in every case.
pub async fn play_announcement(
    player_id: usize,
    discovery: Duration,
    announcement: &Announcement<'_>,
) -> AnnounceOutcome {
    let (device, client) = match connect_zone_cast(player_id, discovery).await {
        Ok(connected) => connected,
        Err(error) => {
            return AnnounceOutcome {
                loaded: false,
                error: Some(error),
            }
        }
    };
    info!(player_id, cast = %device.friendly_name, "playing voice message");
    let mut loaded = false;
    let result = play_and_wait(&client, announcement, &mut loaded).await;
    if let Err(e) = client.disconnect().await {
        debug!(cast = %device.friendly_name, error = %e, "cast disconnect after message failed");
    }
    AnnounceOutcome {
        loaded,
        error: result.err(),
    }
}

/// Receiver volume and mute to use for the message: unmuted, at the message volume.
fn message_level(previous: u8, muted: bool, min_volume: Option<u8>) -> (u8, bool) {
    let target = crate::voice_upload::message_volume(previous, min_volume);
    (target, muted)
}

async fn play_and_wait(
    client: &CastClient,
    announcement: &Announcement<'_>,
    loaded: &mut bool,
) -> Result<()> {
    let started = std::time::Instant::now();
    let receiver = client
        .receiver_status()
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))
        .context("Cast receiver status")?;
    let (previous, _) = cast_volume_to_loxone(receiver.volume.level, receiver.volume.muted);
    let (target, unmute) = message_level(previous, receiver.volume.muted, announcement.min_volume);

    client
        .launch_app(&CastApp::DefaultMediaReceiver)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))
        .context("Cast launch default media receiver")?;
    debug!(
        ms = started.elapsed().as_millis() as u64,
        "voice message: receiver app ready"
    );
    if target != previous {
        cast_set_volume(client, target).await?;
    }
    if unmute {
        set_muted(client, false).await?;
    }

    let outcome = load_and_wait(client, announcement, loaded, started).await;

    if unmute {
        if let Err(e) = set_muted(client, true).await {
            warn!(error = %e, "failed to mute again after voice message");
        }
    }
    if target != previous {
        if let Err(e) = cast_set_volume(client, previous).await {
            warn!(error = %e, level = previous, "failed to restore volume after voice message");
        }
    }
    outcome
}

async fn set_muted(client: &CastClient, muted: bool) -> Result<()> {
    client
        .set_muted(muted)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))
        .context("Cast set muted")?;
    Ok(())
}

async fn load_and_wait(
    client: &CastClient,
    announcement: &Announcement<'_>,
    loaded: &mut bool,
    started: std::time::Instant,
) -> Result<()> {
    let mut media_status = client.watch_media_status();
    let media = MediaInfo::new(announcement.url, announcement.content_type)
        .stream_type(StreamType::Buffered)
        .metadata(MediaMetadata::Generic {
            title: Some(announcement.title.to_string()),
            subtitle: None,
            images: vec![],
        });
    let media_loaded = client
        .load_media(&media, true, 0.0, None)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))
        .context("Cast load voice message")?;
    *loaded = true;
    let session = media_loaded.media_session_id;
    debug!(
        ms = started.elapsed().as_millis() as u64,
        "voice message: media loaded"
    );

    let wait = async {
        let mut playing_logged = false;
        loop {
            if let Some(status) = media_status.borrow_and_update().as_ref() {
                if !playing_logged && status.player_state == CastPlayerState::Playing {
                    playing_logged = true;
                    debug!(
                        ms = started.elapsed().as_millis() as u64,
                        "voice message: playing"
                    );
                }
                if let Some(done) = message_outcome(status, session) {
                    return done;
                }
            }
            if media_status.changed().await.is_err() {
                bail!("Cast connection closed during voice message");
            }
        }
    };
    match tokio::time::timeout(announcement.max_wait, wait).await {
        Ok(result) => result,
        Err(_) => bail!(
            "voice message did not finish within {}s",
            announcement.max_wait.as_secs()
        ),
    }
}

/// `Some` once the message's media session is over: `Ok` when it played to the end (or was
/// replaced), `Err` when the receiver reports an error.
fn message_outcome(status: &MediaStatus, session: i32) -> Option<Result<()>> {
    if status.player_state != CastPlayerState::Idle {
        return None;
    }
    if status.media_session_id != session && status.media_session_id != 0 {
        return None;
    }
    let reason = status.idle_reason?;
    Some(match reason {
        oxicast::IdleReason::Error => {
            Err(anyhow::anyhow!("Cast receiver could not play the message"))
        }
        _ => Ok(()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxicast::{IdleReason, Volume};

    #[test]
    fn message_level_unmutes_and_raises_to_the_tts_minimum() {
        // Muted at 30 with TTS 20: same level, but unmuted for the message.
        assert_eq!(message_level(30, true, Some(20)), (30, true));
        assert_eq!(message_level(10, false, Some(20)), (20, false));
        assert_eq!(message_level(10, true, None), (10, true));
    }

    fn status(state: CastPlayerState, session: i32, reason: Option<IdleReason>) -> MediaStatus {
        MediaStatus {
            media_session_id: session,
            player_state: state,
            idle_reason: reason,
            current_time: 0.0,
            duration: None,
            volume: Volume::new(0.2),
            media: None,
        }
    }

    #[test]
    fn message_outcome_waits_for_its_own_session_to_end() {
        assert!(message_outcome(&status(CastPlayerState::Playing, 7, None), 7).is_none());
        assert!(message_outcome(&status(CastPlayerState::Buffering, 7, None), 7).is_none());
        // Idle before the load took effect (no reason yet).
        assert!(message_outcome(&status(CastPlayerState::Idle, 7, None), 7).is_none());
        // An older session ending is not this message.
        assert!(message_outcome(
            &status(CastPlayerState::Idle, 6, Some(IdleReason::Finished)),
            7
        )
        .is_none());

        let finished = message_outcome(
            &status(CastPlayerState::Idle, 7, Some(IdleReason::Finished)),
            7,
        );
        assert!(matches!(finished, Some(Ok(()))));
        let interrupted = message_outcome(
            &status(CastPlayerState::Idle, 0, Some(IdleReason::Interrupted)),
            7,
        );
        assert!(matches!(interrupted, Some(Ok(()))));
        let failed = message_outcome(
            &status(CastPlayerState::Idle, 7, Some(IdleReason::Error)),
            7,
        );
        assert!(matches!(failed, Some(Err(_))));
    }
}
