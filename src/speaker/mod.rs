//! Speaker control backends: per-zone play/pause, volume, power and now-playing from a
//! speaker's own LAN API. Content is started in the speaker's app; Loxone controls and mirrors it.
//!
//! Vendor implementations plug in through [`connect_backend`]; the vendor-neutral pieces are the
//! [`SpeakerBackend`] trait, the [`map`] status mapper, mDNS [`discovery`], zone [`binding`] and the
//! polling [`manager`].

pub mod beo;
pub mod binding;
pub mod discovery;
pub mod manager;
pub mod map;
pub mod stream800;

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;

use crate::player_state::Mode;

pub use discovery::DiscoveredSpeaker;
pub use manager::{SpeakerEvent, SpeakerManager};

/// Which local API a speaker speaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SpeakerKind {
    /// KEF W2 platform on StreamUnlimited (LS60, LSX II, LS50 Wireless II): HTTP `getData`/`setData`.
    KefStream800,
    /// Naim on StreamUnlimited (Mu-so Qb 2nd gen): same HTTP `getData`, Naim power API on 15081.
    NaimStream800,
    /// KEF LSX / LS50 Wireless first generation: binary TCP protocol on port 50001.
    KefLegacy,
    /// Bang & Olufsen BeoLink / ASE (Beoplay M3, A6, M5, …): BeoNetRemote HTTP API on 8080.
    Beo,
}

impl std::str::FromStr for SpeakerKind {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "kef_stream800" => Ok(Self::KefStream800),
            "naim_stream800" => Ok(Self::NaimStream800),
            "kef_legacy" => Ok(Self::KefLegacy),
            "beo" => Ok(Self::Beo),
            other => anyhow::bail!("unknown speaker_kind {other:?}"),
        }
    }
}

impl SpeakerKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::KefStream800 => "kef_stream800",
            Self::NaimStream800 => "naim_stream800",
            Self::KefLegacy => "kef_legacy",
            Self::Beo => "beo",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SpeakerPower {
    #[default]
    On,
    Standby,
    /// The speaker did not answer (switched off at the wall, Wi-Fi asleep, wrong address).
    Unreachable,
}

/// One poll of a speaker, in vendor-neutral terms.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SpeakerStatus {
    pub power: SpeakerPower,
    /// `None` when the speaker is on but reports nothing playing.
    pub mode: Option<Mode>,
    /// 0–100.
    pub volume: u8,
    pub muted: bool,
    /// Lowercase source id: `"wifi"`, `"airplay"`, `"spotify"`, `"bluetooth"`, `"optical"`, …
    pub source: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub cover_url: String,
    pub duration_s: u32,
    pub position_s: i64,
    /// Live stream (internet radio): `title` is the station name, there is no track length.
    pub live: bool,
    /// Opaque id of the current play session, new when a stream (re)starts; `""` if unknown.
    /// Lets the Loxone App reload a cover whose URL did not change (see
    /// [`crate::zone_enrich::apply_cover_proxy`]).
    pub play_session: String,
}

impl SpeakerStatus {
    pub fn unreachable() -> Self {
        Self {
            power: SpeakerPower::Unreachable,
            ..Default::default()
        }
    }

    pub fn is_playing(&self) -> bool {
        self.power == SpeakerPower::On && matches!(self.mode, Some(Mode::Play))
    }
}

/// A command Loxone sends to a speaker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpeakerCommand {
    Play,
    Pause,
    SetVolume(u8),
    Power(bool),
}

/// Local control API of one speaker.
#[async_trait]
pub trait SpeakerBackend: Send + Sync {
    async fn status(&self) -> Result<SpeakerStatus>;
    async fn play(&self) -> Result<()>;
    async fn pause(&self) -> Result<()>;
    /// `level` is 0–100.
    async fn set_volume(&self, level: u8) -> Result<()>;
    async fn power(&self, on: bool) -> Result<()>;

    /// How long a command may take; `None` uses the manager's poll timeout.
    fn command_timeout(&self) -> Option<std::time::Duration> {
        None
    }

    /// Remember what plays now, before another source (a Cast voice message) takes over the
    /// speaker. Returns whether there is something [`resume_interrupted`] can restart.
    ///
    /// [`resume_interrupted`]: SpeakerBackend::resume_interrupted
    async fn interrupt(&self) -> Result<bool> {
        Ok(false)
    }

    /// Restart what [`interrupt`] remembered. Returns whether anything was restarted.
    ///
    /// [`interrupt`]: SpeakerBackend::interrupt
    async fn resume_interrupted(&self) -> Result<bool> {
        Ok(false)
    }

    async fn run(&self, command: SpeakerCommand) -> Result<()> {
        match command {
            SpeakerCommand::Play => self.play().await,
            SpeakerCommand::Pause => self.pause().await,
            SpeakerCommand::SetVolume(level) => self.set_volume(level.min(100)).await,
            SpeakerCommand::Power(on) => self.power(on).await,
        }
    }
}

/// Build the backend for a speaker kind at `host`, or `None` when that kind has no
/// implementation yet (first-generation KEF follows separately).
pub fn connect_backend(kind: SpeakerKind, host: &str) -> Option<Arc<dyn SpeakerBackend>> {
    use stream800::{Stream800, Vendor};
    match kind {
        SpeakerKind::KefStream800 => Some(Arc::new(Stream800::new(host, Vendor::Kef))),
        SpeakerKind::NaimStream800 => Some(Arc::new(Stream800::new(host, Vendor::Naim))),
        SpeakerKind::KefLegacy => None,
        SpeakerKind::Beo => Some(Arc::new(beo::Beo::new(host))),
    }
}

#[cfg(test)]
pub(crate) mod fake {
    //! In-memory speaker for manager and coordinator tests.

    use super::*;
    use std::sync::Mutex;

    /// A connector that always hands out this fake (each test owns its own).
    pub fn connector(fake: &Arc<FakeSpeaker>) -> super::manager::Connector {
        let fake = Arc::clone(fake);
        Arc::new(move |_, _| Some(Arc::clone(&fake) as Arc<dyn SpeakerBackend>))
    }

    #[derive(Default)]
    pub struct FakeSpeaker {
        pub status: Mutex<SpeakerStatus>,
        pub commands: Mutex<Vec<SpeakerCommand>>,
        pub fail: Mutex<bool>,
        /// Record commands without applying them (a speaker that reacts late).
        pub inert: Mutex<bool>,
        /// After `Power(true)`, keep reporting standby for this many status polls.
        pub wake_polls: Mutex<u32>,
        pending_wake: Mutex<u32>,
        /// Set when `Play` arrives while the speaker is not on yet.
        pub played_while_off: Mutex<bool>,
        /// Commands fail while polls still work.
        pub fail_commands: Mutex<bool>,
        /// `interrupt` / `resume` calls (voice messages), in order.
        pub interruptions: Mutex<Vec<&'static str>>,
    }

    impl FakeSpeaker {
        pub fn with_status(status: SpeakerStatus) -> Arc<Self> {
            Arc::new(Self {
                status: Mutex::new(status),
                ..Default::default()
            })
        }

        pub fn set_failing(&self, fail: bool) {
            *self.fail.lock().unwrap() = fail;
        }

        pub fn commands(&self) -> Vec<SpeakerCommand> {
            self.commands.lock().unwrap().clone()
        }

        fn check(&self) -> Result<()> {
            if *self.fail.lock().unwrap() {
                anyhow::bail!("fake speaker offline");
            }
            Ok(())
        }

        fn record(&self, command: SpeakerCommand) -> Result<()> {
            self.check()?;
            if *self.fail_commands.lock().unwrap() {
                anyhow::bail!("fake speaker refused {command:?}");
            }
            self.commands.lock().unwrap().push(command);
            if *self.inert.lock().unwrap() {
                return Ok(());
            }
            let mut status = self.status.lock().unwrap();
            match command {
                SpeakerCommand::Play => {
                    if status.power != SpeakerPower::On {
                        *self.played_while_off.lock().unwrap() = true;
                    }
                    status.mode = Some(Mode::Play)
                }
                SpeakerCommand::Pause => status.mode = Some(Mode::Pause),
                SpeakerCommand::SetVolume(level) => status.volume = level,
                SpeakerCommand::Power(false) => status.power = SpeakerPower::Standby,
                SpeakerCommand::Power(true) => {
                    let delay = *self.wake_polls.lock().unwrap();
                    if delay == 0 {
                        status.power = SpeakerPower::On;
                    } else {
                        *self.pending_wake.lock().unwrap() = delay;
                    }
                }
            }
            Ok(())
        }
    }

    #[async_trait]
    impl SpeakerBackend for FakeSpeaker {
        async fn status(&self) -> Result<SpeakerStatus> {
            self.check()?;
            let mut pending = self.pending_wake.lock().unwrap();
            let mut status = self.status.lock().unwrap();
            if *pending > 0 {
                *pending -= 1;
                if *pending == 0 {
                    status.power = SpeakerPower::On;
                }
            }
            Ok(status.clone())
        }
        async fn play(&self) -> Result<()> {
            self.record(SpeakerCommand::Play)
        }
        async fn pause(&self) -> Result<()> {
            self.record(SpeakerCommand::Pause)
        }
        async fn set_volume(&self, level: u8) -> Result<()> {
            self.record(SpeakerCommand::SetVolume(level))
        }
        async fn power(&self, on: bool) -> Result<()> {
            self.record(SpeakerCommand::Power(on))
        }
        async fn interrupt(&self) -> Result<bool> {
            self.check()?;
            self.interruptions.lock().unwrap().push("interrupt");
            Ok(true)
        }
        async fn resume_interrupted(&self) -> Result<bool> {
            self.check()?;
            self.interruptions.lock().unwrap().push("resume");
            Ok(true)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::FakeSpeaker;
    use super::*;

    #[test]
    fn kind_ids_round_trip() {
        for kind in [
            SpeakerKind::KefStream800,
            SpeakerKind::NaimStream800,
            SpeakerKind::KefLegacy,
            SpeakerKind::Beo,
        ] {
            assert_eq!(kind.as_str().parse::<SpeakerKind>().unwrap(), kind);
        }
        assert_eq!(
            " KEF_Legacy ".parse::<SpeakerKind>().unwrap(),
            SpeakerKind::KefLegacy
        );
        assert!("sonos".parse::<SpeakerKind>().is_err());
    }

    #[test]
    fn stream800_kinds_have_a_backend_legacy_not_yet() {
        assert!(connect_backend(SpeakerKind::KefStream800, "192.168.1.2").is_some());
        assert!(connect_backend(SpeakerKind::NaimStream800, "192.168.1.3").is_some());
        assert!(connect_backend(SpeakerKind::KefLegacy, "192.168.1.4").is_none());
    }

    #[test]
    fn is_playing_requires_power_and_play_mode() {
        let mut status = SpeakerStatus {
            mode: Some(Mode::Play),
            ..Default::default()
        };
        assert!(status.is_playing());
        status.power = SpeakerPower::Standby;
        assert!(!status.is_playing());
        assert!(!SpeakerStatus::unreachable().is_playing());
    }

    #[tokio::test]
    async fn run_dispatches_and_clamps_volume() {
        let speaker = FakeSpeaker::with_status(SpeakerStatus::default());
        speaker.run(SpeakerCommand::SetVolume(150)).await.unwrap();
        speaker.run(SpeakerCommand::Power(false)).await.unwrap();
        speaker.run(SpeakerCommand::Play).await.unwrap();
        assert_eq!(
            speaker.commands(),
            vec![
                SpeakerCommand::SetVolume(100),
                SpeakerCommand::Power(false),
                SpeakerCommand::Play
            ]
        );
        let status = speaker.status().await.unwrap();
        assert_eq!(status.volume, 100);
        assert_eq!(status.power, SpeakerPower::Standby);
        speaker.set_failing(true);
        assert!(speaker.pause().await.is_err());
    }
}
