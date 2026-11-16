//! Voice messages recorded in the Loxone App (`audio/grouped/playuploadedfile/{file}/{zones}`).
//!
//! Each target zone plays the uploaded WAV once on its Cast device (`cast_device_id`), at the
//! zone's TTS volume or louder, and then gets back what it played:
//! - a Cast radio session of ours is stopped and the zone released in the registry before the
//!   message, and the station restarted afterwards at the volume it had;
//! - live radio started on the speaker itself is kept by its backend
//!   ([`SpeakerBackend::interrupt`]) and replayed afterwards, but only if the message actually
//!   took the speaker over. A track (e.g. Qobuz) stays stopped.
//!
//! [`VoiceMessages`] (owned by the coordinator) tracks the zones with a message in progress: a
//! second message for such a zone waits for the first, the zone is restored once after the last
//! one, and nothing is restored when the user sent the zone a transport command meanwhile.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::{
    cast::{play_announcement, Announcement, CastPlaybackManager},
    config::ZoneConfig,
    player_state::Playerid,
    remote::{IoCommand, PlayOpts},
    speaker::{SpeakerBackend, SpeakerManager},
    voice_upload,
    zone_transport::VolumeStep,
};

/// Cast discovery time when resolving a zone's Cast device.
const CAST_DISCOVERY: Duration = Duration::from_secs(3);
/// Assumed length when the WAV header cannot be read (the App records at most 1 minute).
const UNKNOWN_LENGTH: Duration = Duration::from_secs(60);
/// Extra wait beyond the message length (receiver start-up, buffering).
const END_MARGIN: Duration = Duration::from_secs(20);

/// A zone finished one message.
#[derive(Debug)]
pub(crate) struct VoiceDone {
    pub player_id: Playerid,
    /// Message file it played.
    pub path: PathBuf,
    /// Cast took the speaker over (the media loaded), whether or not it played to the end.
    pub loaded: bool,
}

/// A Cast radio session stopped for a message: its station and the volume it played at.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct StoppedRadio {
    pub station: String,
    /// Restored after the restart, which would otherwise start at the zone's default volume.
    pub volume: Option<u8>,
}

/// What a zone gets back after its last message.
pub(crate) struct Restore {
    pub player_id: Playerid,
    /// Our Cast radio to restart.
    pub radio: Option<StoppedRadio>,
    /// Speaker whose own live radio to replay ([`SpeakerBackend::resume_interrupted`]).
    pub speaker: Option<Arc<dyn SpeakerBackend>>,
}

#[derive(Clone, Debug)]
struct Job {
    url: String,
    path: PathBuf,
    max_wait: Duration,
}

/// A zone with a message in progress.
struct ZoneVoice {
    radio: Option<StoppedRadio>,
    speaker: Option<Arc<dyn SpeakerBackend>>,
    /// Messages that arrived while one was playing.
    queue: VecDeque<Job>,
    /// Some message took the speaker over.
    loaded: bool,
    /// The user sent the zone a transport command during the messages.
    user_took_over: bool,
}

/// Voice messages in progress, per zone.
pub(crate) struct VoiceMessages {
    dir: PathBuf,
    cover_proxy_base: String,
    done: mpsc::Sender<VoiceDone>,
    zones: HashMap<usize, ZoneVoice>,
    /// Zones that still have to play each message file; the file is deleted at zero.
    pending_files: HashMap<PathBuf, usize>,
}

impl VoiceMessages {
    pub(crate) fn new(dir: &Path, cover_proxy_base: &str, done: mpsc::Sender<VoiceDone>) -> Self {
        Self {
            dir: dir.to_path_buf(),
            cover_proxy_base: cover_proxy_base.to_string(),
            done,
            zones: HashMap::new(),
            pending_files: HashMap::new(),
        }
    }

    /// Start (or queue) the uploaded `file` on `zones`. Our Cast radio sessions on idle target
    /// zones are stopped first; their ids are returned so the coordinator releases them in the
    /// registry.
    pub(crate) async fn start(
        &mut self,
        file: &str,
        zones: &[Playerid],
        cast_manager: &mut CastPlaybackManager,
        speaker_manager: &SpeakerManager,
    ) -> Vec<Playerid> {
        let message = match voice_upload::claim(&self.dir, file).await {
            Ok(message) => message,
            Err(e) => {
                warn!(file, error = %e, "voice message not played: upload not found");
                return Vec::new();
            }
        };
        let job = Job {
            url: format!(
                "{}{}/{}",
                self.cover_proxy_base,
                voice_upload::SERVE_PREFIX,
                message.file_name
            ),
            max_wait: message.length.unwrap_or(UNKNOWN_LENGTH) + END_MARGIN,
            path: message.path,
        };
        self.pending_files.insert(job.path.clone(), zones.len());

        let (busy, idle): (Vec<Playerid>, Vec<Playerid>) = zones
            .iter()
            .partition(|player_id| self.zones.contains_key(&player_id.0));
        for player_id in busy {
            debug!(
                player_id = player_id.0,
                "voice message queued behind the playing one"
            );
            if let Some(zone) = self.zones.get_mut(&player_id.0) {
                zone.queue.push_back(job.clone());
            }
        }

        let radios: Vec<(Playerid, Option<StoppedRadio>)> = idle
            .iter()
            .map(|&player_id| (player_id, stopped_radio(cast_manager, player_id)))
            .collect();
        let radio_zones: Vec<Playerid> = radios
            .iter()
            .filter(|(_, radio)| radio.is_some())
            .map(|(player_id, _)| *player_id)
            .collect();
        cast_manager.stop_zones(&radio_zones).await;

        for (player_id, radio) in radios {
            // While our Cast radio plays, the speaker's own player shows that stream: nothing
            // of its own to keep.
            let speaker = match radio {
                Some(_) => None,
                None => speaker_manager.backend(player_id.0),
            };
            spawn_job(player_id, job.clone(), speaker.clone(), self.done.clone());
            self.zones.insert(
                player_id.0,
                ZoneVoice {
                    radio,
                    speaker,
                    queue: VecDeque::new(),
                    loaded: false,
                    user_took_over: false,
                },
            );
        }
        radio_zones
    }

    /// The user sent `player_id` a transport command: do not restore it after its messages.
    pub(crate) fn note_transport(&mut self, player_id: Playerid) {
        if let Some(zone) = self.zones.get_mut(&player_id.0) {
            zone.user_took_over = true;
        }
    }

    /// One message on a zone is over. Starts the zone's next queued message, or returns what
    /// to restore after its last one.
    pub(crate) fn finished(&mut self, done: VoiceDone) -> Option<Restore> {
        self.release_file(&done.path);
        let zone = self.zones.get_mut(&done.player_id.0)?;
        zone.loaded |= done.loaded;
        if let Some(job) = zone.queue.pop_front() {
            spawn_job(done.player_id, job, None, self.done.clone());
            return None;
        }
        let zone = self.zones.remove(&done.player_id.0)?;
        if zone.user_took_over {
            info!(
                player_id = done.player_id.0,
                "not restoring playback after voice message: zone was controlled meanwhile"
            );
            return Some(Restore {
                player_id: done.player_id,
                radio: None,
                speaker: None,
            });
        }
        Some(Restore {
            player_id: done.player_id,
            radio: zone.radio,
            // A message that never took the speaker over left its radio playing.
            speaker: zone.speaker.filter(|_| zone.loaded),
        })
    }

    fn release_file(&mut self, path: &Path) {
        let Some(left) = self.pending_files.get_mut(path) else {
            return;
        };
        *left = left.saturating_sub(1);
        if *left == 0 {
            self.pending_files.remove(path);
            let path = path.to_path_buf();
            tokio::spawn(async move {
                if let Err(e) = tokio::fs::remove_file(&path).await {
                    debug!(file = %path.display(), error = %e, "voice message file not removed");
                }
            });
        }
    }
}

fn stopped_radio(cast_manager: &CastPlaybackManager, player_id: Playerid) -> Option<StoppedRadio> {
    cast_manager
        .radio_audiopath(player_id.0)
        .map(|station| StoppedRadio {
            station,
            volume: cast_manager
                .current_volume(player_id)
                .map(|(level, muted)| if muted { 0 } else { level }),
        })
}

/// Commands that bring back a Cast radio stopped for a message: play the station, then set the
/// volume it had (the restart itself starts at the zone's default volume).
pub(crate) fn radio_restart_commands(player_id: Playerid, radio: StoppedRadio) -> Vec<IoCommand> {
    let mut commands = vec![IoCommand::Play(
        player_id,
        Some(PlayOpts {
            id: radio.station,
            session_token: String::new(),
        }),
    )];
    if let Some(level) = radio.volume {
        commands.push(IoCommand::Volume(
            player_id,
            VolumeStep::Absolute(i32::from(level)),
        ));
    }
    commands
}

/// Replay the speaker's own live radio kept before the first message (in the background: it
/// is speaker HTTP I/O).
pub(crate) fn spawn_speaker_resume(player_id: Playerid, speaker: Arc<dyn SpeakerBackend>) {
    tokio::spawn(async move {
        match speaker.resume_interrupted().await {
            Ok(true) => info!(
                player_id = player_id.0,
                "speaker radio restarted after voice message"
            ),
            Ok(false) => {}
            Err(e) => warn!(
                player_id = player_id.0,
                error = %e,
                "speaker radio restart after voice message failed"
            ),
        }
    });
}

/// The zone's TTS volume from Loxone Config (`audio/cfg/volumes`), used as the minimum.
fn tts_minimum(player_id: Playerid) -> Option<u8> {
    ZoneConfig::get()
        .players
        .iter()
        .find(|p| p.player_id == player_id.0)
        .and_then(|p| p.loxone_tts)
}

/// Play one message on a zone in the background. `interrupt` is the speaker to keep the
/// playback of first (the zone's first message only).
fn spawn_job(
    player_id: Playerid,
    job: Job,
    interrupt: Option<Arc<dyn SpeakerBackend>>,
    done: mpsc::Sender<VoiceDone>,
) {
    tokio::spawn(async move {
        let id = player_id.0;
        if let Some(speaker) = interrupt {
            match speaker.interrupt().await {
                Ok(resumable) => debug!(
                    player_id = id,
                    resumable, "speaker playback kept for voice message"
                ),
                Err(e) => {
                    debug!(player_id = id, error = %e, "speaker state before voice message unknown")
                }
            }
        }
        let outcome = play_announcement(
            id,
            CAST_DISCOVERY,
            &Announcement {
                url: &job.url,
                content_type: "audio/wav",
                title: "Voice message",
                min_volume: tts_minimum(player_id),
                max_wait: job.max_wait,
            },
        )
        .await;
        match &outcome.error {
            None => info!(player_id = id, "voice message played"),
            Some(e) => {
                warn!(player_id = id, loaded = outcome.loaded, error = %e, "voice message failed")
            }
        }
        let _ = done
            .send(VoiceDone {
                player_id,
                path: job.path,
                loaded: outcome.loaded,
            })
            .await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AudioserverConfigFile;
    use crate::speaker::binding::ZoneSpeakerTarget;
    use crate::speaker::fake::{self, FakeSpeaker};
    use crate::speaker::manager::PollTiming;
    use crate::speaker::{SpeakerKind, SpeakerStatus};

    fn speaker_on_zone_4() -> (Arc<FakeSpeaker>, SpeakerManager) {
        let fake_speaker = FakeSpeaker::with_status(SpeakerStatus::default());
        let (tx, _rx) = mpsc::channel(8);
        let timing = PollTiming {
            idle: Duration::from_secs(60),
            ..PollTiming::default()
        };
        let mut manager =
            SpeakerManager::with_connector(tx, fake::connector(&fake_speaker), timing);
        manager.sync(&[ZoneSpeakerTarget {
            player_id: 4,
            speaker_id: "ls60".into(),
            kind: SpeakerKind::KefStream800,
            host: "10.0.0.4".into(),
        }]);
        (fake_speaker, manager)
    }

    struct Harness {
        dir: PathBuf,
        voice: VoiceMessages,
        done_rx: mpsc::Receiver<VoiceDone>,
        cast_manager: CastPlaybackManager,
        fake_speaker: Arc<FakeSpeaker>,
        speaker_manager: SpeakerManager,
    }

    impl Harness {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("loxone-voice-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            let (done_tx, done_rx) = mpsc::channel(8);
            let (cast_tx, _cast_rx) = mpsc::channel(8);
            let (fake_speaker, speaker_manager) = speaker_on_zone_4();
            Self {
                voice: VoiceMessages::new(&dir, "http://10.0.0.1:7091", done_tx),
                dir,
                done_rx,
                cast_manager: CastPlaybackManager::new(cast_tx),
                fake_speaker,
                speaker_manager,
            }
        }

        async fn send(&mut self, zones: &[Playerid]) -> Vec<Playerid> {
            voice_upload::store(&self.dir, "audio_recording.wav", b"RIFF....WAVE")
                .await
                .unwrap();
            self.voice
                .start(
                    "audio_recording.wav",
                    zones,
                    &mut self.cast_manager,
                    &self.speaker_manager,
                )
                .await
        }

        async fn next_done(&mut self) -> VoiceDone {
            tokio::time::timeout(Duration::from_secs(5), self.done_rx.recv())
                .await
                .expect("zone reports done")
                .unwrap()
        }

        fn interruptions(&self) -> Vec<&'static str> {
            self.fake_speaker.interruptions.lock().unwrap().clone()
        }
    }

    impl Drop for Harness {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[tokio::test]
    async fn a_message_that_never_took_over_does_not_replay_the_radio() {
        let _config = AudioserverConfigFile::lock_for_test();
        let mut h = Harness::new("no-cast");
        assert!(
            h.send(&[Playerid(4)]).await.is_empty(),
            "no Cast radio to stop"
        );

        // No cast_device_id configured: Cast never took the speaker over.
        let done = h.next_done().await;
        assert_eq!(done.player_id, Playerid(4));
        assert!(!done.loaded);
        let path = done.path.clone();
        let restore = h.voice.finished(done).expect("last message on the zone");
        assert!(restore.radio.is_none());
        assert!(restore.speaker.is_none(), "its radio is still playing");
        assert_eq!(h.interruptions(), vec!["interrupt"]);

        // The message file is deleted once the zone is done.
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn a_second_message_waits_and_the_zone_is_restored_once() {
        let _config = AudioserverConfigFile::lock_for_test();
        let mut h = Harness::new("queue");
        h.send(&[Playerid(4)]).await;
        h.send(&[Playerid(4)]).await;

        let first = h.next_done().await;
        let first_path = first.path.clone();
        // Pretend the first one played: Cast took over.
        let first = VoiceDone {
            loaded: true,
            ..first
        };
        assert!(
            h.voice.finished(first).is_none(),
            "the queued message starts"
        );
        let second = h.next_done().await;
        assert_ne!(second.path, first_path, "each message has its own file");
        let restore = h.voice.finished(second).expect("restored after the last");
        assert!(restore.speaker.is_some(), "an earlier message took over");
        assert_eq!(
            h.interruptions(),
            vec!["interrupt"],
            "kept once, before the first"
        );
    }

    #[tokio::test]
    async fn a_zone_controlled_during_the_message_is_not_restored() {
        let _config = AudioserverConfigFile::lock_for_test();
        let mut h = Harness::new("takeover");
        h.send(&[Playerid(4)]).await;
        h.voice.note_transport(Playerid(4));
        h.voice.note_transport(Playerid(3)); // no message there: ignored

        let done = VoiceDone {
            loaded: true,
            ..h.next_done().await
        };
        let restore = h.voice.finished(done).unwrap();
        assert!(restore.radio.is_none() && restore.speaker.is_none());
    }

    #[tokio::test]
    async fn missing_upload_plays_nothing() {
        let mut h = Harness::new("missing");
        let released = h
            .voice
            .start(
                "../nothing.wav",
                &[Playerid(4)],
                &mut h.cast_manager,
                &h.speaker_manager,
            )
            .await;
        assert!(released.is_empty());
        drop(h.voice.done.clone());
        assert!(
            tokio::time::timeout(Duration::from_millis(200), h.done_rx.recv())
                .await
                .is_err(),
            "no zone plays"
        );
        assert!(h.interruptions().is_empty());
    }

    #[test]
    fn stopped_radio_restarts_at_its_volume() {
        let radio = StoppedRadio {
            station: "custom:station:1".into(),
            volume: Some(30),
        };
        let commands = radio_restart_commands(Playerid(4), radio);
        assert!(matches!(
            &commands[..],
            [
                IoCommand::Play(Playerid(4), Some(opts)),
                IoCommand::Volume(Playerid(4), VolumeStep::Absolute(30)),
            ] if opts.id == "custom:station:1"
        ));

        let unknown_volume = StoppedRadio {
            station: "custom:station:1".into(),
            volume: None,
        };
        assert_eq!(radio_restart_commands(Playerid(4), unknown_volume).len(), 1);
    }
}
