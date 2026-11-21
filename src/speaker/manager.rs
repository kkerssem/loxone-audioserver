//! Per-zone speaker polling and command routing.
//!
//! Each bound zone has its own task that polls the speaker and runs Loxone commands in order,
//! so a slow or unreachable speaker never blocks the coordinator or other zones.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use tokio::sync::{mpsc, Notify};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tracing::{debug, info, warn};

use super::binding::ZoneSpeakerTarget;
use super::discovery::{browse_speakers, DiscoveredSpeaker};
use super::{
    connect_backend, SpeakerBackend, SpeakerCommand, SpeakerKind, SpeakerPower, SpeakerStatus,
};

/// Messages from background speaker tasks to the coordinator.
#[derive(Clone, Debug, PartialEq)]
pub enum SpeakerEvent {
    /// A zone's speaker reported a changed status.
    Status {
        player_id: usize,
        /// Boxed: a status is far larger than the other variants.
        status: Box<SpeakerStatus>,
    },
    /// A discovery pass found these speakers.
    Discovered(Vec<DiscoveredSpeaker>),
}

#[derive(Clone, Copy, Debug)]
pub struct PollTiming {
    pub playing: Duration,
    pub idle: Duration,
    /// Poll interval right after a command, while the speaker settles (e.g. paused → playing).
    pub after_command: Duration,
    /// How long the fast `after_command` polling lasts.
    pub after_command_window: Duration,
    /// Budget for one whole poll or command. A backend call can be several HTTP requests in a
    /// row (each with its own shorter timeout), so this must exceed a single request's timeout.
    pub request_timeout: Duration,
    /// Consecutive failed polls before the zone is reported unreachable.
    pub failures_until_unreachable: u32,
    /// After a power-on command, how long to wait for the speaker to report it is on before
    /// running the next queued command (e.g. Play).
    pub wake_timeout: Duration,
}

impl Default for PollTiming {
    fn default() -> Self {
        Self {
            playing: Duration::from_secs(1),
            idle: Duration::from_secs(5),
            after_command: Duration::from_millis(300),
            after_command_window: Duration::from_secs(4),
            request_timeout: Duration::from_secs(4),
            failures_until_unreachable: 3,
            wake_timeout: Duration::from_secs(10),
        }
    }
}

/// Builds the backend for a speaker kind at a host (`None`: that kind is not implemented).
pub type Connector =
    Arc<dyn Fn(SpeakerKind, &str) -> Option<Arc<dyn SpeakerBackend>> + Send + Sync>;

/// Queued commands per zone; Loxone sends a handful at most (e.g. volume steps).
const COMMAND_QUEUE: usize = 16;

/// State shared between the manager and a zone's task.
#[derive(Default)]
struct ZoneShared {
    last: Mutex<Option<SpeakerStatus>>,
    refresh: Notify,
    /// Last volume queued for the speaker and when: taken as current until polls catch up.
    commanded_volume: Mutex<Option<(u8, Instant)>>,
}

/// How long a queued volume counts as the speaker's volume. Speakers report a change a moment
/// late; a quick second step must not start from the old level and get lost.
pub(crate) const VOLUME_SETTLE: Duration = Duration::from_secs(3);

struct ZoneSpeaker {
    target: ZoneSpeakerTarget,
    shared: Arc<ZoneShared>,
    commands: mpsc::Sender<SpeakerCommand>,
    task: JoinHandle<()>,
    /// Same backend the zone task uses, for calls that need an answer (voice messages).
    backend: Arc<dyn SpeakerBackend>,
}

impl Drop for ZoneSpeaker {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub struct SpeakerManager {
    zones: HashMap<usize, ZoneSpeaker>,
    events: mpsc::Sender<SpeakerEvent>,
    connect: Connector,
    timing: PollTiming,
}

impl SpeakerManager {
    pub fn new(events: mpsc::Sender<SpeakerEvent>) -> Self {
        Self::with_connector(events, Arc::new(connect_backend), PollTiming::default())
    }

    pub(crate) fn with_connector(
        events: mpsc::Sender<SpeakerEvent>,
        connect: Connector,
        timing: PollTiming,
    ) -> Self {
        Self {
            zones: HashMap::new(),
            events,
            connect,
            timing,
        }
    }

    pub fn is_bound(&self, player_id: usize) -> bool {
        self.zones.contains_key(&player_id)
    }

    /// The zone's speaker backend, for calls outside the zone's command queue.
    pub fn backend(&self, player_id: usize) -> Option<Arc<dyn SpeakerBackend>> {
        self.zones.get(&player_id).map(|z| Arc::clone(&z.backend))
    }

    /// [`last_status`](Self::last_status) with the volume just queued (if within
    /// [`VOLUME_SETTLE`]), for resolving the next command against.
    pub fn status_for_commands(&self, player_id: usize) -> Option<SpeakerStatus> {
        let zone = self.zones.get(&player_id)?;
        let mut status = zone.shared.last.lock().ok()?.clone()?;
        let commanded = *zone.shared.commanded_volume.lock().ok()?;
        if let Some((level, at)) = commanded {
            if at.elapsed() < VOLUME_SETTLE {
                status.volume = level;
                status.muted = status.muted && level == 0;
            }
        }
        Some(status)
    }

    #[cfg(test)]
    pub fn last_status(&self, player_id: usize) -> Option<SpeakerStatus> {
        self.zones
            .get(&player_id)
            .and_then(|z| z.shared.last.lock().ok().and_then(|g| g.clone()))
    }

    /// Bind zones to `targets`: start polling new or changed bindings, stop removed ones.
    /// Kinds without a backend implementation are skipped.
    pub fn sync(&mut self, targets: &[ZoneSpeakerTarget]) {
        let wanted: HashMap<usize, &ZoneSpeakerTarget> =
            targets.iter().map(|t| (t.player_id, t)).collect();
        self.zones.retain(|player_id, zone| {
            let keep = wanted.get(player_id).is_some_and(|t| **t == zone.target);
            if !keep {
                info!(player_id, speaker_id = %zone.target.speaker_id, "speaker unbound from zone");
            }
            keep
        });
        for target in targets {
            if self.zones.contains_key(&target.player_id) {
                continue;
            }
            match (self.connect)(target.kind, &target.host) {
                Some(backend) => self.bind(target.clone(), backend),
                None => debug!(
                    player_id = target.player_id,
                    speaker_id = %target.speaker_id,
                    kind = target.kind.as_str(),
                    "no backend for speaker kind yet"
                ),
            }
        }
    }

    fn bind(&mut self, target: ZoneSpeakerTarget, backend: Arc<dyn SpeakerBackend>) {
        info!(
            player_id = target.player_id,
            speaker_id = %target.speaker_id,
            kind = target.kind.as_str(),
            host = %target.host,
            "speaker bound to zone"
        );
        let shared = Arc::new(ZoneShared::default());
        let (commands, command_rx) = mpsc::channel(COMMAND_QUEUE);
        let task = tokio::spawn(
            ZoneTask {
                player_id: target.player_id,
                backend: Arc::clone(&backend),
                shared: Arc::clone(&shared),
                events: self.events.clone(),
                timing: self.timing,
                failures: 0,
                fast_until: None,
            }
            .run(command_rx),
        );
        self.zones.insert(
            target.player_id,
            ZoneSpeaker {
                target,
                shared,
                commands,
                task,
                backend,
            },
        );
    }

    /// Queue a command for the zone's speaker. Commands run in order on the zone's own task,
    /// which logs failures; this only fails when no speaker is bound or the queue is full.
    pub fn command(&self, player_id: usize, command: SpeakerCommand) -> Result<()> {
        let zone = self
            .zones
            .get(&player_id)
            .with_context(|| format!("no speaker bound to zone {player_id}"))?;
        let volume = match command {
            SpeakerCommand::SetVolume(level) => Some(level),
            _ => None,
        };
        zone.commands
            .try_send(command)
            .map_err(|e| anyhow!("speaker command queue for zone {player_id}: {e}"))?;
        if let Some(level) = volume {
            if let Ok(mut commanded) = zone.shared.commanded_volume.lock() {
                *commanded = Some((level, Instant::now()));
            }
        }
        Ok(())
    }

    /// Backend of a bound speaker whose kind matches, for calls not tied to a zone (browsing a
    /// music service): one whose last poll found it on, else any that is not known to be
    /// unreachable, else any; the lowest zone id among equals.
    pub fn first_backend(
        &self,
        matches: impl Fn(SpeakerKind) -> bool,
    ) -> Option<Arc<dyn SpeakerBackend>> {
        let rank = |zone: &ZoneSpeaker| {
            let last = zone
                .shared
                .last
                .lock()
                .ok()
                .and_then(|l| l.as_ref().map(|s| s.power));
            match last {
                Some(SpeakerPower::On) => 0,
                Some(SpeakerPower::Unreachable) => 2,
                _ => 1,
            }
        };
        self.zones
            .iter()
            .filter(|(_, zone)| matches(zone.target.kind))
            .min_by_key(|(player_id, zone)| (rank(zone), **player_id))
            .map(|(_, zone)| Arc::clone(&zone.backend))
    }

    /// Speaker API of the zone's bound speaker.
    pub fn kind(&self, player_id: usize) -> Option<SpeakerKind> {
        self.zones.get(&player_id).map(|z| z.target.kind)
    }

    /// Forget the zone's last reported status and poll now, so its current status is
    /// published again — e.g. after a Cast radio session that owned the zone has ended.
    pub fn refresh(&self, player_id: usize) {
        if let Some(zone) = self.zones.get(&player_id) {
            if let Ok(mut last) = zone.shared.last.lock() {
                *last = None;
            }
            zone.shared.refresh.notify_one();
        }
    }
}

/// One bound zone: polls its speaker and runs its queued commands, one at a time.
struct ZoneTask {
    player_id: usize,
    backend: Arc<dyn SpeakerBackend>,
    shared: Arc<ZoneShared>,
    events: mpsc::Sender<SpeakerEvent>,
    timing: PollTiming,
    failures: u32,
    fast_until: Option<Instant>,
}

impl ZoneTask {
    async fn run(mut self, mut commands: mpsc::Receiver<SpeakerCommand>) {
        let shared = Arc::clone(&self.shared);
        loop {
            if !self.poll().await {
                return;
            }
            tokio::select! {
                _ = tokio::time::sleep(self.next_wait()) => {}
                _ = shared.refresh.notified() => {}
                command = commands.recv() => {
                    let Some(command) = command else { return };
                    self.execute(command).await;
                }
            }
        }
    }

    /// Poll once and report a changed status. Returns `false` when the coordinator is gone.
    async fn poll(&mut self) -> bool {
        let polled = tokio::time::timeout(self.timing.request_timeout, self.backend.status()).await;
        let player_id = self.player_id;
        let status = match polled {
            Ok(Ok(status)) => {
                self.failures = 0;
                status
            }
            failed => {
                self.failures += 1;
                match failed {
                    Ok(Err(e)) => {
                        debug!(player_id, failures = self.failures, error = %e, "speaker poll failed")
                    }
                    _ => debug!(
                        player_id,
                        failures = self.failures,
                        "speaker poll timed out"
                    ),
                }
                if self.failures < self.timing.failures_until_unreachable {
                    return true;
                }
                if self.failures == self.timing.failures_until_unreachable {
                    warn!(player_id, "speaker unreachable");
                }
                // Keep reporting unreachable; the change check below sends it once (and again
                // after a refresh).
                SpeakerStatus::unreachable()
            }
        };
        let changed = {
            let mut last = self.shared.last.lock().expect("speaker status lock");
            let changed = last.as_ref() != Some(&status);
            *last = Some(status.clone());
            changed
        };
        !changed
            || self
                .events
                .send(SpeakerEvent::Status {
                    player_id,
                    status: Box::new(status),
                })
                .await
                .is_ok()
    }

    async fn execute(&mut self, command: SpeakerCommand) {
        // A backend may need longer than a poll (e.g. a speaker whose server stalls); cancelling
        // a command early can leave it half done on the speaker.
        let timeout = self
            .backend
            .command_timeout()
            .map_or(self.timing.request_timeout, |t| {
                t.max(self.timing.request_timeout)
            });
        let result = tokio::time::timeout(timeout, self.backend.run(command.clone()))
            .await
            .map_err(|_| anyhow!("timed out"))
            .and_then(|r| r);
        if result.is_err() && matches!(command, SpeakerCommand::SetVolume(_)) {
            // Not applied: the next step must start from what the speaker reports.
            if let Ok(mut commanded) = self.shared.commanded_volume.lock() {
                *commanded = None;
            }
        }
        match result {
            Err(e) => {
                warn!(player_id = self.player_id, ?command, error = %e, "speaker command failed")
            }
            // A volume change or seek needs no settle polling (a slider drag would otherwise
            // keep the speaker busy); one re-poll shows the new level or position.
            Ok(())
                if matches!(
                    command,
                    SpeakerCommand::SetVolume(_) | SpeakerCommand::Seek(_)
                ) =>
            {
                self.poll().await;
            }
            Ok(()) => {
                // Play / pause / power: the speaker reports the new state a moment late.
                self.fast_until = Some(Instant::now() + self.timing.after_command_window);
                if command == SpeakerCommand::Power(true) {
                    self.wait_until_on().await;
                }
            }
        }
    }

    /// After power-on, poll until the speaker reports it is on (or `wake_timeout` passes), so
    /// a following Play is not sent to a speaker that is still waking up.
    async fn wait_until_on(&mut self) {
        let deadline = Instant::now() + self.timing.wake_timeout;
        while Instant::now() < deadline {
            if !self.poll().await || self.last_power() == Some(SpeakerPower::On) {
                return;
            }
            tokio::time::sleep(self.timing.after_command).await;
        }
        debug!(
            player_id = self.player_id,
            "speaker not on after wake timeout"
        );
    }

    fn last_power(&self) -> Option<SpeakerPower> {
        self.shared.last.lock().ok()?.as_ref().map(|s| s.power)
    }

    fn next_wait(&self) -> Duration {
        let settling = self.fast_until.is_some_and(|until| Instant::now() < until);
        let playing = self
            .shared
            .last
            .lock()
            .ok()
            .and_then(|g| g.as_ref().map(SpeakerStatus::is_playing))
            .unwrap_or(false);
        if settling {
            self.timing.after_command
        } else if playing {
            self.timing.playing
        } else {
            self.timing.idle
        }
    }
}

/// Browse for speakers every `interval` and report each successful pass. A failed browse is
/// logged and reports nothing, so known speakers are never dropped because of it.
pub fn spawn_discovery(events: mpsc::Sender<SpeakerEvent>, interval: Duration) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut previous: Vec<DiscoveredSpeaker> = Vec::new();
        loop {
            match browse_speakers(Duration::from_secs(3)).await {
                Ok(found) => {
                    for speaker in found.iter().filter(|s| !previous.contains(s)) {
                        info!(
                            speaker_id = %speaker.id,
                            kind = speaker.kind.as_str(),
                            name = %speaker.name,
                            host = %speaker.host,
                            "speaker discovered"
                        );
                    }
                    if found != previous {
                        previous = found.clone();
                        if events.send(SpeakerEvent::Discovered(found)).await.is_err() {
                            return;
                        }
                    }
                }
                Err(e) => warn!(error = %e, "speaker discovery failed"),
            }
            tokio::time::sleep(interval).await;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::super::fake::{self, FakeSpeaker};
    use super::super::SpeakerPower;
    use super::*;
    use crate::player_state::Mode;

    fn fast() -> PollTiming {
        PollTiming {
            playing: Duration::from_millis(20),
            idle: Duration::from_millis(20),
            after_command: Duration::from_millis(20),
            after_command_window: Duration::from_millis(100),
            request_timeout: Duration::from_millis(200),
            failures_until_unreachable: 2,
            wake_timeout: Duration::from_secs(2),
        }
    }

    fn target(player_id: usize, id: &str) -> ZoneSpeakerTarget {
        ZoneSpeakerTarget {
            player_id,
            speaker_id: id.into(),
            kind: SpeakerKind::KefStream800,
            host: "10.0.0.1".into(),
        }
    }

    fn manager_with(
        fake: &Arc<FakeSpeaker>,
        timing: PollTiming,
    ) -> (SpeakerManager, mpsc::Receiver<SpeakerEvent>) {
        let (tx, rx) = mpsc::channel(32);
        (
            SpeakerManager::with_connector(tx, fake::connector(fake), timing),
            rx,
        )
    }

    async fn next_status(rx: &mut mpsc::Receiver<SpeakerEvent>) -> SpeakerStatus {
        match tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("speaker event")
            .expect("channel open")
        {
            SpeakerEvent::Status { status, .. } => *status,
            other => panic!("unexpected {other:?}"),
        }
    }

    async fn status_matching(
        rx: &mut mpsc::Receiver<SpeakerEvent>,
        wanted: impl Fn(&SpeakerStatus) -> bool,
    ) -> SpeakerStatus {
        loop {
            let status = next_status(rx).await;
            if wanted(&status) {
                return status;
            }
        }
    }

    #[tokio::test]
    async fn the_zone_free_backend_prefers_a_speaker_that_is_on() {
        let asleep = FakeSpeaker::with_status(SpeakerStatus {
            power: SpeakerPower::Standby,
            ..Default::default()
        });
        let awake = FakeSpeaker::with_status(SpeakerStatus::default());
        let (tx, mut rx) = mpsc::channel(32);
        let (a, b) = (Arc::clone(&asleep), Arc::clone(&awake));
        let connector: Connector = Arc::new(move |_, host: &str| {
            let fake = if host == "asleep" { &a } else { &b };
            Some(Arc::clone(fake) as Arc<dyn SpeakerBackend>)
        });
        let mut manager = SpeakerManager::with_connector(tx, connector, fast());
        let naim = |player_id, host: &str| ZoneSpeakerTarget {
            player_id,
            speaker_id: host.into(),
            kind: SpeakerKind::NaimStream800,
            host: host.into(),
        };
        manager.sync(&[naim(1, "asleep"), naim(2, "awake")]);
        next_status(&mut rx).await;
        next_status(&mut rx).await;

        let backend = manager.first_backend(SpeakerKind::plays_qobuz).unwrap();
        backend.status().await.unwrap();
        assert!(Arc::ptr_eq(
            &backend,
            &(Arc::clone(&awake) as Arc<dyn SpeakerBackend>)
        ));
        assert!(manager.first_backend(|_| false).is_none());
    }

    #[tokio::test]
    async fn a_seek_reports_the_new_position() {
        let fake = FakeSpeaker::with_status(SpeakerStatus {
            mode: Some(Mode::Play),
            duration_s: 240,
            position_s: 10,
            ..Default::default()
        });
        let (mut manager, mut rx) = manager_with(&fake, fast());
        manager.sync(&[target(3, "naim")]);
        next_status(&mut rx).await;

        manager.command(3, SpeakerCommand::Seek(95)).unwrap();
        status_matching(&mut rx, |s| s.position_s == 95).await;
        assert_eq!(fake.commands(), vec![SpeakerCommand::Seek(95)]);
    }

    #[tokio::test]
    async fn polls_reports_changes_and_runs_queued_commands_in_order() {
        let fake = FakeSpeaker::with_status(SpeakerStatus {
            mode: Some(Mode::Pause),
            volume: 20,
            title: "Song".into(),
            ..Default::default()
        });
        let (mut manager, mut rx) = manager_with(&fake, fast());
        manager.sync(&[target(3, "ls60")]);
        assert!(manager.is_bound(3));

        assert_eq!(next_status(&mut rx).await.title, "Song");
        assert_eq!(manager.last_status(3).unwrap().volume, 20);

        // Queuing returns at once; the zone task runs the commands in order.
        manager.command(3, SpeakerCommand::Play).unwrap();
        manager.command(3, SpeakerCommand::SetVolume(35)).unwrap();
        let latest = status_matching(&mut rx, |s| s.volume == 35).await;
        assert_eq!(latest.mode, Some(Mode::Play));
        assert_eq!(
            fake.commands(),
            vec![SpeakerCommand::Play, SpeakerCommand::SetVolume(35)]
        );
        assert!(manager.command(9, SpeakerCommand::Pause).is_err());
    }

    #[tokio::test]
    async fn a_queued_volume_counts_until_the_speaker_reports_it() {
        let fake = FakeSpeaker::with_status(SpeakerStatus {
            mode: Some(Mode::Play),
            volume: 40,
            ..Default::default()
        });
        // A slow speaker: it takes the command but keeps reporting the old level.
        *fake.inert.lock().unwrap() = true;
        let (mut manager, mut rx) = manager_with(&fake, fast());
        manager.sync(&[target(3, "m3")]);
        next_status(&mut rx).await;

        assert_eq!(manager.status_for_commands(3).unwrap().volume, 40);
        manager.command(3, SpeakerCommand::SetVolume(39)).unwrap();
        assert_eq!(manager.last_status(3).unwrap().volume, 40);
        assert_eq!(
            manager.status_for_commands(3).unwrap().volume,
            39,
            "the next step starts from the level just queued"
        );
        assert!(manager.status_for_commands(9).is_none());
    }

    #[tokio::test]
    async fn a_failed_volume_is_not_counted_as_applied() {
        let fake = FakeSpeaker::with_status(SpeakerStatus {
            mode: Some(Mode::Play),
            volume: 40,
            ..Default::default()
        });
        *fake.fail_commands.lock().unwrap() = true;
        let (mut manager, mut rx) = manager_with(&fake, fast());
        manager.sync(&[target(3, "m3")]);
        next_status(&mut rx).await;

        manager.command(3, SpeakerCommand::SetVolume(39)).unwrap();
        assert_eq!(manager.status_for_commands(3).unwrap().volume, 39);
        // Once the zone task has run (and failed) it, steps start from the speaker again.
        for _ in 0..50 {
            if manager.status_for_commands(3).unwrap().volume == 40 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("failed volume still counted");
    }

    #[tokio::test]
    async fn repeated_failures_report_unreachable_once() {
        let fake = FakeSpeaker::with_status(SpeakerStatus::default());
        let (mut manager, mut rx) = manager_with(&fake, fast());
        manager.sync(&[target(1, "lsx")]);
        assert_eq!(next_status(&mut rx).await.power, SpeakerPower::On);

        fake.set_failing(true);
        assert_eq!(next_status(&mut rx).await.power, SpeakerPower::Unreachable);
        assert!(
            tokio::time::timeout(Duration::from_millis(150), rx.recv())
                .await
                .is_err(),
            "unreachable must not be re-sent every poll"
        );

        fake.set_failing(false);
        assert_eq!(next_status(&mut rx).await.power, SpeakerPower::On);
    }

    #[tokio::test]
    async fn refresh_republishes_an_unchanged_status() {
        let fake = FakeSpeaker::with_status(SpeakerStatus {
            power: SpeakerPower::Standby,
            ..Default::default()
        });
        let timing = PollTiming {
            idle: Duration::from_secs(60),
            ..fast()
        };
        let (mut manager, mut rx) = manager_with(&fake, timing);
        manager.sync(&[target(4, "ls60")]);
        assert_eq!(next_status(&mut rx).await.power, SpeakerPower::Standby);

        manager.refresh(4);
        assert_eq!(
            next_status(&mut rx).await.power,
            SpeakerPower::Standby,
            "same status sent again after refresh"
        );
    }

    #[tokio::test]
    async fn sync_rebinds_changed_targets_and_drops_removed() {
        let fake = FakeSpeaker::with_status(SpeakerStatus::default());
        let (mut manager, _rx) = manager_with(&fake, fast());
        manager.sync(&[target(1, "a"), target(2, "b")]);
        assert!(manager.is_bound(1) && manager.is_bound(2));

        let mut moved = target(1, "a");
        moved.host = "10.0.0.99".into();
        manager.sync(&[moved.clone()]);
        assert!(manager.is_bound(1));
        assert!(!manager.is_bound(2));
        assert_eq!(manager.zones[&1].target, moved);

        manager.sync(&[]);
        assert!(!manager.is_bound(1));
    }

    /// A speaker that reports the new state a moment after the command (like the Mu-so going
    /// paused → playing) still reaches Loxone quickly: polling speeds up after a command even
    /// though the idle interval is long.
    #[tokio::test]
    async fn commands_trigger_fast_polling_while_the_speaker_settles() {
        let fake = FakeSpeaker::with_status(SpeakerStatus {
            mode: Some(Mode::Pause),
            ..Default::default()
        });
        *fake.inert.lock().unwrap() = true;
        let timing = PollTiming {
            playing: Duration::from_secs(60),
            idle: Duration::from_secs(60),
            after_command: Duration::from_millis(30),
            after_command_window: Duration::from_secs(2),
            ..fast()
        };
        let (mut manager, mut rx) = manager_with(&fake, timing);
        manager.sync(&[target(3, "muso")]);
        assert_eq!(next_status(&mut rx).await.mode, Some(Mode::Pause));

        manager.command(3, SpeakerCommand::Play).unwrap();
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert_eq!(fake.commands(), vec![SpeakerCommand::Play]);
        // The speaker only now reports playing.
        fake.status.lock().unwrap().mode = Some(Mode::Play);

        let started = Instant::now();
        status_matching(&mut rx, |s| s.mode == Some(Mode::Play)).await;
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "playing state must arrive within the fast-poll window, not the 60 s idle interval"
        );
    }

    #[tokio::test]
    async fn play_after_power_on_waits_until_the_speaker_is_on() {
        let fake = FakeSpeaker::with_status(SpeakerStatus {
            power: SpeakerPower::Standby,
            ..Default::default()
        });
        *fake.wake_polls.lock().unwrap() = 3;
        let (mut manager, mut rx) = manager_with(&fake, fast());
        manager.sync(&[target(4, "ls60")]);
        assert_eq!(next_status(&mut rx).await.power, SpeakerPower::Standby);

        manager.command(4, SpeakerCommand::Power(true)).unwrap();
        manager.command(4, SpeakerCommand::Play).unwrap();
        status_matching(&mut rx, |s| s.mode == Some(Mode::Play)).await;
        assert_eq!(
            fake.commands(),
            vec![SpeakerCommand::Power(true), SpeakerCommand::Play]
        );
        assert!(
            !*fake.played_while_off.lock().unwrap(),
            "Play must wait until the speaker reports on"
        );
    }

    #[tokio::test]
    async fn a_failing_command_does_not_stop_the_zone() {
        let fake = FakeSpeaker::with_status(SpeakerStatus::default());
        let (mut manager, mut rx) = manager_with(&fake, fast());
        manager.sync(&[target(1, "a")]);
        next_status(&mut rx).await;

        fake.set_failing(true);
        manager.command(1, SpeakerCommand::Pause).unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        fake.set_failing(false);
        manager.command(1, SpeakerCommand::SetVolume(9)).unwrap();
        status_matching(&mut rx, |s| s.volume == 9).await;
    }

    #[test]
    fn default_command_budget_exceeds_one_request() {
        // Stream800 requests time out after 1.5 s each; a poll or toggle is 2–3 of them.
        assert!(PollTiming::default().request_timeout >= Duration::from_secs(3));
    }

    #[tokio::test]
    async fn only_play_pause_power_open_the_fast_poll_window() {
        let fake = FakeSpeaker::with_status(SpeakerStatus::default());
        let timing = PollTiming {
            playing: Duration::from_secs(60),
            idle: Duration::from_secs(60),
            after_command: Duration::from_millis(20),
            after_command_window: Duration::from_secs(5),
            ..fast()
        };
        let (mut manager, mut rx) = manager_with(&fake, timing);
        manager.sync(&[target(1, "ls60")]);
        next_status(&mut rx).await;

        // A volume step re-polls once, then waits the (long) idle interval again.
        manager.command(1, SpeakerCommand::SetVolume(12)).unwrap();
        assert_eq!(next_status(&mut rx).await.volume, 12);
        fake.status.lock().unwrap().title = "changed later".into();
        assert!(
            tokio::time::timeout(Duration::from_millis(300), rx.recv())
                .await
                .is_err(),
            "no fast polling after a volume change"
        );

        // A pause does open the window: the later change shows up quickly.
        manager.command(1, SpeakerCommand::Pause).unwrap();
        status_matching(&mut rx, |s| s.title == "changed later").await;
    }

    #[tokio::test]
    async fn kinds_without_backend_are_not_bound() {
        let (tx, _rx) = mpsc::channel(4);
        let mut manager = SpeakerManager::with_connector(tx, Arc::new(|_, _| None), fast());
        manager.sync(&[target(1, "a")]);
        assert!(!manager.is_bound(1));
        assert!(manager.last_status(1).is_none());
    }
}
