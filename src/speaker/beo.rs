//! Bang & Olufsen speakers on the BeoLink / ASE platform (Beoplay M3; A6, M5, A9 2nd gen use
//! the same API): the BeoNetRemote HTTP API on port 8080, advertised as `_beocore._tcp`.
//!
//! State comes from the long-poll feed `GET /BeoNotify/Notifications`, a stream of JSON objects
//! (`{"notification":{"type":…,"data":…}}`) that starts with a snapshot of the current source,
//! now playing, progress and volume. A background task keeps a [`BeoState`] from it; polls read
//! that state instead of asking the speaker.
//!
//! Commands (checked on a Beoplay M3, software 6.5, 2026-11-15):
//! - play / pause: `POST /BeoZone/Zone/Stream/{Play,Pause}` then `…/Release`. The speaker's own
//!   radio (`beoradio`) really pauses and resumes. On a Cast stream pause ends the stream.
//! - volume: `PUT /BeoZone/Zone/Sound/Volume/Speaker/Level {"level":n}`, range 0–90 (from the
//!   feed). Ignored while paused (the speaker answers 200 but keeps its level).
//! - power: `PUT /BeoDevice/powerManagement/standby {"standby":{"powerState":"on"|"standby"}}`.
//!   The API stays up in standby; waking resets the volume to the speaker's default level.
//! - The HTTP server sometimes stalls for several seconds (a request then still takes effect),
//!   so commands use a long timeout and run one at a time.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::task::JoinHandle;
use tracing::debug;

use crate::player_state::Mode;

use super::manager::VOLUME_SETTLE;
use super::{SpeakerBackend, SpeakerPower, SpeakerStatus};

/// BeoNetRemote API port.
pub const BEO_PORT: u16 = 8080;

const PATH_NOTIFY: &str = "/BeoNotify/Notifications";
const PATH_STANDBY: &str = "/BeoDevice/powerManagement/standby";
const PATH_LEVEL: &str = "/BeoZone/Zone/Sound/Volume/Speaker/Level";
const PATH_MUTED: &str = "/BeoZone/Zone/Sound/Volume/Speaker/Muted";
const PATH_STREAM: &str = "/BeoZone/Zone/Stream";
const PATH_ACTIVE_SOURCES: &str = "/BeoZone/Zone/ActiveSources";

/// Command timeout: the speaker's server can stall for ~8 s and still apply the request.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(12);
/// Pause before reconnecting the notification feed after it ended or failed.
const FEED_RETRY: Duration = Duration::from_secs(3);
/// The feed counts as down (speaker unreachable) when it has not connected for this long.
const FEED_GRACE: Duration = Duration::from_secs(15);
/// A feed that sends nothing for this long is treated as dead and reconnected. While playing it
/// reports progress every second; a reconnect replays the current state, so an idle speaker
/// that stays silent is only reconnected, not lost. Without it a speaker that drops off Wi-Fi
/// without closing the connection would show its last state forever.
const FEED_IDLE: Duration = Duration::from_secs(90);
/// Standby lookup timeout (kept under the manager's poll timeout).
const STANDBY_TIMEOUT: Duration = Duration::from_secs(3);
/// Sources a voice message can give back by selecting them and pressing play: the speaker's
/// own radio (verified on a Beoplay M3). Sender-driven sources (Cast, AirPlay, Spotify Connect,
/// Bluetooth, line-in) would only switch to a silent input.
const RESUMABLE_SOURCES: &[&str] = &["beoradio", "radio"];
/// Speaker volume range until the feed reports it (Beoplay M3: 0–90).
const DEFAULT_MAX_LEVEL: u8 = 90;

/// Speaker state as reported by the notification feed.
#[derive(Clone, Debug, PartialEq)]
pub struct BeoState {
    pub standby: bool,
    /// Full id of the active source (`beoradio:<jid>`, `googlecast:<jid>`, …); `""` for none.
    pub source_id: String,
    pub mode: Option<Mode>,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub cover_url: String,
    pub duration_s: u32,
    pub position_s: i64,
    pub live: bool,
    pub play_session: String,
    /// Speaker level in the speaker's own range (0..=`max_level`).
    pub level: u8,
    pub max_level: u8,
    pub muted: bool,
}

impl Default for BeoState {
    fn default() -> Self {
        Self {
            standby: false,
            source_id: String::new(),
            mode: None,
            title: String::new(),
            artist: String::new(),
            album: String::new(),
            cover_url: String::new(),
            duration_s: 0,
            position_s: 0,
            live: false,
            play_session: String::new(),
            level: 0,
            max_level: DEFAULT_MAX_LEVEL,
            muted: false,
        }
    }
}

impl BeoState {
    fn clear_now_playing(&mut self) {
        self.title.clear();
        self.artist.clear();
        self.album.clear();
        self.cover_url.clear();
        self.duration_s = 0;
        self.position_s = 0;
        self.live = false;
        self.play_session.clear();
    }

    /// Short source name (`beoradio`, `spotify`, `googlecast`, …): the id before `:`.
    pub fn source(&self) -> &str {
        self.source_id.split(':').next().unwrap_or_default()
    }

    pub fn to_status(&self) -> SpeakerStatus {
        if self.standby {
            return SpeakerStatus {
                power: SpeakerPower::Standby,
                ..Default::default()
            };
        }
        SpeakerStatus {
            power: SpeakerPower::On,
            mode: self.mode.clone(),
            volume: to_loxone_level(self.level, self.max_level),
            muted: self.muted,
            source: loxone_source(self.source()).to_string(),
            title: self.title.clone(),
            artist: self.artist.clone(),
            album: self.album.clone(),
            cover_url: self.cover_url.clone(),
            duration_s: self.duration_s,
            position_s: self.position_s,
            live: self.live,
            station_name: String::new(),
            play_session: self.play_session.clone(),
            // B&O radio is not replayed from a description yet.
            station: None,
            track_path: String::new(),
        }
    }
}

/// Source id as the vendor-neutral mapper knows it ([`super::map`]).
fn loxone_source(source: &str) -> &str {
    match source {
        "beoradio" | "radio" | "tunein" => "radio",
        "googlecast" => "cast",
        "linein" => "aux",
        other => other,
    }
}

/// Speaker level (0..=`max`) → Loxone 0–100.
pub fn to_loxone_level(level: u8, max: u8) -> u8 {
    if max == 0 {
        return 0;
    }
    ((f32::from(level.min(max)) * 100.0 / f32::from(max)).round() as u8).min(100)
}

/// Loxone 0–100 → speaker level (0..=`max`).
pub fn to_speaker_level(level: u8, max: u8) -> u8 {
    (f32::from(level.min(100)) * f32::from(max) / 100.0).round() as u8
}

/// Speaker level for Loxone `level`, moving at least one speaker step away from `current` when
/// Loxone asked for a level other than `reference` (the Loxone level `current` stands for). The
/// ranges differ (0–100 vs 0–90), so one Loxone step can round back to the current speaker
/// level (36 → 35 is 32 → 32) and the volume would stick.
pub fn step_level(level: u8, current: u8, reference: u8, max: u8) -> u8 {
    let raw = to_speaker_level(level, max);
    if raw != current || level == reference {
        return raw;
    }
    if level < reference {
        current.saturating_sub(1)
    } else {
        current.saturating_add(1).min(max)
    }
}

fn mode_from(state: &str) -> Option<Mode> {
    match state {
        "play" | "playing" => Some(Mode::Play),
        "pause" | "paused" => Some(Mode::Pause),
        "stop" | "stopped" => Some(Mode::Stop),
        _ => None,
    }
}

fn text(value: &Value, pointer: &str) -> String {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// First image URL, preferring a large one.
fn image_url(images: Option<&Value>) -> String {
    let Some(images) = images.and_then(Value::as_array) else {
        return String::new();
    };
    images
        .iter()
        .find(|i| i.get("size").and_then(Value::as_str) == Some("large"))
        .or_else(|| images.first())
        .and_then(|i| i.get("url"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Apply one feed object (`{"notification":{…}}`) to `state`.
pub fn apply_notification(state: &mut BeoState, object: &Value) {
    let Some(n) = object.get("notification") else {
        return;
    };
    let kind = n.get("type").and_then(Value::as_str).unwrap_or_default();
    let data = n.get("data").unwrap_or(&Value::Null);
    match kind {
        "SOURCE" | "SOURCE_EXPERIENCE_CHANGED" => {
            let experience = data.get("primaryExperience");
            let source_id = experience
                .map(|e| text(e, "/source/id"))
                .filter(|id| !id.is_empty())
                .unwrap_or_else(|| text(data, "/primary"));
            // Only SOURCE (an empty one, at standby) says nothing is selected; the experience
            // change has not been captured, so one without a source id is not taken as that.
            if source_id.is_empty() && kind == "SOURCE_EXPERIENCE_CHANGED" {
                return;
            }
            if source_id != state.source_id {
                state.clear_now_playing();
            }
            if source_id.is_empty() {
                state.mode = None;
            } else {
                state.standby = false;
            }
            state.source_id = source_id;
            if let Some(mode) = experience
                .and_then(|e| e.get("state"))
                .and_then(Value::as_str)
                .and_then(mode_from)
            {
                state.mode = Some(mode);
            }
        }
        "NOW_PLAYING_NET_RADIO" => {
            state.title = text(data, "/name");
            state.artist = text(data, "/liveDescription");
            state.album.clear();
            state.cover_url = image_url(data.get("image"));
            state.duration_s = 0;
            state.position_s = 0;
            state.live = true;
            state.play_session = text(data, "/stationId");
        }
        "NOW_PLAYING_STORED_MUSIC" => {
            state.title = text(data, "/name");
            state.artist = text(data, "/artist");
            state.album = text(data, "/album");
            state.cover_url = image_url(data.get("trackImage"));
            if state.cover_url.is_empty() {
                state.cover_url = image_url(data.get("albumImage"));
            }
            state.duration_s = data
                .get("duration")
                .and_then(Value::as_u64)
                .and_then(|d| u32::try_from(d).ok())
                .unwrap_or(0);
            state.live = state.duration_s == 0;
            state.play_session =
                format!("{}/{}", text(data, "/playQueueItemId"), text(data, "/name"));
        }
        "NOW_PLAYING_ENDED" => {
            state.clear_now_playing();
            state.mode = Some(Mode::Stop);
        }
        "PROGRESS_INFORMATION" => {
            if let Some(mode) = data
                .get("state")
                .and_then(Value::as_str)
                .and_then(mode_from)
            {
                state.mode = Some(mode);
            }
            if let Some(position) = data.get("position").and_then(Value::as_i64) {
                state.position_s = progress_seconds(position, state.duration_s, state.live);
            }
        }
        "VOLUME" => {
            let speaker = data.get("speaker").unwrap_or(&Value::Null);
            if let Some(max) = speaker
                .pointer("/range/maximum")
                .and_then(Value::as_u64)
                .and_then(|m| u8::try_from(m).ok())
                .filter(|&m| m > 0)
            {
                state.max_level = max;
            }
            if let Some(level) = speaker
                .get("level")
                .and_then(Value::as_u64)
                .and_then(|l| u8::try_from(l).ok())
            {
                state.level = level;
            }
            if let Some(muted) = speaker.get("muted").and_then(Value::as_bool) {
                state.muted = muted;
            }
        }
        "SHUTDOWN" => {
            state.standby = true;
            state.source_id.clear();
            state.mode = None;
            state.clear_now_playing();
        }
        _ => {}
    }
}

/// Feed `position` → seconds. The M3 counts seconds for its own radio but milliseconds for a
/// Cast stream; live streams have no meaningful position.
fn progress_seconds(position: i64, duration_s: u32, live: bool) -> i64 {
    if live {
        return 0;
    }
    if duration_s > 0 && position > i64::from(duration_s) + 5 {
        return position / 1000;
    }
    position
}

/// Split complete JSON objects off the front of `buffer`, leaving any partial object. Bytes,
/// not text: a multi-byte character split across network chunks stays intact.
pub fn drain_objects(buffer: &mut Vec<u8>) -> Vec<Value> {
    let mut objects = Vec::new();
    let mut consumed = 0;
    let mut stream = serde_json::Deserializer::from_slice(buffer).into_iter::<Value>();
    while let Some(next) = stream.next() {
        match next {
            Ok(value) => {
                objects.push(value);
                consumed = stream.byte_offset();
            }
            // An object cut off at the end of the received bytes: wait for the rest.
            Err(e) if e.is_eof() => break,
            Err(_) => {
                // Not JSON (or corrupt): drop what was read so far and resync on new data.
                consumed = buffer.len();
                break;
            }
        }
    }
    buffer.drain(..consumed);
    objects
}

/// Feed state shared with the background task.
#[derive(Default)]
struct Shared {
    state: BeoState,
    /// The feed delivered its first snapshot; until then there is no state to report.
    snapshot: bool,
    /// The feed is connected (or connected less than [`FEED_GRACE`] ago).
    last_connected: Option<std::time::Instant>,
    connected: bool,
}

pub struct Beo {
    base: String,
    http: reqwest::Client,
    shared: Arc<Mutex<Shared>>,
    feed: Mutex<Option<JoinHandle<()>>>,
    /// Commands run one at a time (the speaker's server handles concurrent requests badly).
    command_lock: tokio::sync::Mutex<()>,
    /// Source playing when a voice message took the speaker over.
    interrupted: Mutex<Option<String>>,
    /// Last speaker level sent and when; the feed reports it a moment later.
    sent_level: Mutex<Option<SentLevel>>,
}

/// What to step from: the speaker level just sent and the Loxone level it was asked for, while
/// the feed has not caught up; else the reported level and the Loxone level it shows as.
fn step_base(reported: u8, max: u8, sent: Option<SentLevel>) -> (u8, u8) {
    match sent {
        Some(sent) if sent.at.elapsed() < VOLUME_SETTLE => (sent.raw, sent.requested),
        _ => (reported, to_loxone_level(reported, max)),
    }
}

/// A level sent to the speaker.
#[derive(Clone, Copy, Debug)]
struct SentLevel {
    raw: u8,
    /// Loxone level (0–100) it was sent for.
    requested: u8,
    at: std::time::Instant,
}

impl Beo {
    pub fn new(host: &str) -> Self {
        Self::with_base(format!("http://{host}:{BEO_PORT}"))
    }

    fn with_base(base: String) -> Self {
        let http = reqwest::Client::builder()
            .timeout(COMMAND_TIMEOUT)
            .build()
            .unwrap_or_default();
        Self {
            base,
            http,
            shared: Arc::new(Mutex::new(Shared::default())),
            feed: Mutex::new(None),
            command_lock: tokio::sync::Mutex::new(()),
            interrupted: Mutex::new(None),
            sent_level: Mutex::new(None),
        }
    }

    /// Start the notification feed task on first use (needs a running Tokio runtime).
    fn ensure_feed(&self) {
        let mut feed = self.feed.lock().expect("feed lock");
        if feed.as_ref().is_some_and(|task| !task.is_finished()) {
            return;
        }
        let shared = Arc::downgrade(&self.shared);
        let base = self.base.clone();
        *feed = Some(tokio::spawn(run_feed(base, shared)));
    }

    fn state(&self) -> BeoState {
        self.shared.lock().expect("state lock").state.clone()
    }

    async fn send(&self, method: reqwest::Method, path: &str, body: Option<Value>) -> Result<()> {
        let _one_at_a_time = self.command_lock.lock().await;
        let mut request = self.http.request(method, format!("{}{path}", self.base));
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request
            .send()
            .await
            .with_context(|| format!("beo {path}"))?;
        if !response.status().is_success() {
            let status = response.status();
            let detail = response.text().await.unwrap_or_default();
            bail!("beo {path}: http {status} {detail}");
        }
        Ok(())
    }

    /// `POST …/Stream/{action}` then its `Release`, like a remote button press.
    async fn press(&self, action: &str) -> Result<()> {
        let path = format!("{PATH_STREAM}/{action}");
        self.send(reqwest::Method::POST, &path, None).await?;
        self.send(reqwest::Method::POST, &format!("{path}/Release"), None)
            .await
    }

    async fn read_standby(&self) -> Result<bool> {
        read_standby(&self.http, &self.base).await
    }
}

/// Whether the speaker is in standby (`powerManagement/standby` is not `on`).
async fn read_standby(http: &reqwest::Client, base: &str) -> Result<bool> {
    let body: Value = http
        .get(format!("{base}{PATH_STANDBY}"))
        .timeout(STANDBY_TIMEOUT)
        .send()
        .await?
        .json()
        .await?;
    Ok(body.pointer("/standby/powerState").and_then(Value::as_str) != Some("on"))
}

impl Drop for Beo {
    fn drop(&mut self) {
        if let Ok(mut feed) = self.feed.lock() {
            if let Some(task) = feed.take() {
                task.abort();
            }
        }
    }
}

/// Keep `shared` up to date from the notification feed, reconnecting until the backend is gone.
async fn run_feed(base: String, shared: std::sync::Weak<Mutex<Shared>>) {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .build()
        .unwrap_or_default();
    loop {
        let Some(live) = shared.upgrade() else { return };
        drop(live);
        let response = client.get(format!("{base}{PATH_NOTIFY}")).send().await;
        match response {
            Ok(mut response) if response.status().is_success() => {
                // The feed reports going to standby but not being in it: a speaker asleep when
                // the feed (re)connects would otherwise show as on.
                let standby = read_standby(&client, &base).await;
                if let Some(live) = shared.upgrade() {
                    let mut s = live.lock().expect("state lock");
                    s.connected = true;
                    s.last_connected = Some(std::time::Instant::now());
                    if let Ok(standby) = standby {
                        s.state.standby = standby;
                    }
                }
                let mut buffer = Vec::new();
                loop {
                    let chunk = match tokio::time::timeout(FEED_IDLE, response.chunk()).await {
                        Ok(Ok(Some(chunk))) => chunk,
                        Ok(Ok(None)) | Ok(Err(_)) => break,
                        Err(_) => {
                            debug!(base, "beo notification feed silent; reconnecting");
                            break;
                        }
                    };
                    buffer.extend_from_slice(&chunk);
                    let objects = drain_objects(&mut buffer);
                    let Some(live) = shared.upgrade() else { return };
                    let mut s = live.lock().expect("state lock");
                    s.last_connected = Some(std::time::Instant::now());
                    for object in &objects {
                        apply_notification(&mut s.state, object);
                    }
                    s.snapshot |= !objects.is_empty();
                }
            }
            Ok(response) => {
                debug!(base, status = %response.status(), "beo notification feed refused")
            }
            Err(e) => debug!(base, error = %e, "beo notification feed failed"),
        }
        if let Some(live) = shared.upgrade() {
            live.lock().expect("state lock").connected = false;
        }
        tokio::time::sleep(FEED_RETRY).await;
    }
}

#[async_trait]
impl SpeakerBackend for Beo {
    async fn status(&self) -> Result<SpeakerStatus> {
        self.ensure_feed();
        let (state, reachable, snapshot) = {
            let s = self.shared.lock().expect("state lock");
            let recent = s.last_connected.is_some_and(|at| at.elapsed() < FEED_GRACE);
            (s.state.clone(), s.connected || recent, s.snapshot)
        };
        if !reachable {
            bail!("beo notification feed not connected");
        }
        // Before the first snapshot the state is a placeholder (on, volume 0): not reported, so
        // nothing (e.g. a volume step) is computed from it.
        if !snapshot {
            bail!("beo notification feed has not reported yet");
        }
        // The feed reports going to standby (SHUTDOWN) but not every wake: while in standby,
        // ask the speaker.
        if state.standby && !self.read_standby().await? {
            self.shared.lock().expect("state lock").state.standby = false;
            return Ok(BeoState {
                standby: false,
                ..state
            }
            .to_status());
        }
        Ok(state.to_status())
    }

    /// A press is two requests that may each stall up to [`COMMAND_TIMEOUT`]; cancelling
    /// earlier would drop the `Release` while the speaker still applies the press.
    fn command_timeout(&self) -> Option<Duration> {
        Some(COMMAND_TIMEOUT * 2 + Duration::from_secs(1))
    }

    async fn play(&self) -> Result<()> {
        self.press("Play").await
    }

    async fn pause(&self) -> Result<()> {
        self.press("Pause").await
    }

    /// Set the level (scaled to the speaker's range) and, for a non-zero level on a muted
    /// speaker, unmute it. The speaker ignores level changes while paused.
    async fn set_volume(&self, level: u8) -> Result<()> {
        let state = self.state();
        // The M3 answers 200 but keeps its level while paused: report it, so the manager does
        // not take the level as applied and resolve the next step from it.
        if state.mode == Some(Mode::Pause) {
            bail!("beo ignores volume changes while paused");
        }
        let sent = *self.sent_level.lock().expect("level lock");
        let in_effect = sent.filter(|s| s.at.elapsed() < VOLUME_SETTLE);
        // The same level again (a resend, or the command on both 7091 and 7095): keep what was
        // sent. Recomputing would undo a nudge (see `step_level`).
        if in_effect.is_some_and(|s| s.requested == level) {
            return Ok(());
        }
        let (current, reference) = step_base(state.level, state.max_level, sent);
        let raw = step_level(level, current, reference, state.max_level);
        *self.sent_level.lock().expect("level lock") = Some(SentLevel {
            raw,
            requested: level,
            at: std::time::Instant::now(),
        });
        let result = self
            .send(
                reqwest::Method::PUT,
                PATH_LEVEL,
                Some(json!({ "level": raw })),
            )
            .await;
        if result.is_err() {
            *self.sent_level.lock().expect("level lock") = None;
        }
        result?;
        if level > 0 && state.muted {
            self.send(
                reqwest::Method::PUT,
                PATH_MUTED,
                Some(json!({ "muted": false })),
            )
            .await?;
        }
        Ok(())
    }

    async fn power(&self, on: bool) -> Result<()> {
        let power_state = if on { "on" } else { "standby" };
        self.send(
            reqwest::Method::PUT,
            PATH_STANDBY,
            Some(json!({ "standby": { "powerState": power_state } })),
        )
        .await?;
        if !on {
            *self.interrupted.lock().expect("interrupted lock") = None;
        }
        Ok(())
    }

    /// Keep the source that plays now. A Cast stream is not kept: only its sender can start it.
    async fn interrupt(&self) -> Result<bool> {
        let state = self.state();
        let keep = (state.mode == Some(Mode::Play) && RESUMABLE_SOURCES.contains(&state.source()))
            .then(|| state.source_id.clone());
        let resumable = keep.is_some();
        *self.interrupted.lock().expect("interrupted lock") = keep;
        Ok(resumable)
    }

    /// Select the kept source again and press play.
    async fn resume_interrupted(&self) -> Result<bool> {
        let Some(source_id) = self.interrupted.lock().expect("interrupted lock").take() else {
            return Ok(false);
        };
        self.send(
            reqwest::Method::POST,
            PATH_ACTIVE_SOURCES,
            Some(json!({ "primaryExperience": { "source": { "id": source_id } } })),
        )
        .await?;
        self.press("Play").await?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Vec<Value> {
        let text = match name {
            "beoradio" => {
                include_str!("../../tests/fixtures/speakers/beoplay_m3_notify_beoradio.jsonl")
            }
            "cast" => include_str!("../../tests/fixtures/speakers/beoplay_m3_notify_cast.jsonl"),
            "standby" => {
                include_str!("../../tests/fixtures/speakers/beoplay_m3_notify_standby_wake.jsonl")
            }
            other => panic!("unknown fixture {other}"),
        };
        let mut buffer = text.as_bytes().to_vec();
        drain_objects(&mut buffer)
    }

    fn state_after(objects: &[Value]) -> BeoState {
        let mut state = BeoState::default();
        for object in objects {
            apply_notification(&mut state, object);
        }
        state
    }

    #[test]
    fn own_radio_from_the_feed() {
        let objects = fixture("beoradio");
        // The snapshot: playing B&O Radio.
        let state = state_after(&objects[..4]);
        assert_eq!(state.source(), "beoradio");
        assert_eq!(state.mode, Some(Mode::Play));
        let status = state.to_status();
        assert_eq!(status.power, SpeakerPower::On);
        assert_eq!(status.source, "radio");
        assert_eq!(status.title, "NPO Radio 2");
        assert_eq!(status.artist, "Madness - Our House");
        assert_eq!(
            status.cover_url,
            "http://static.airable.io/38/29/804488.png"
        );
        assert!(status.live);
        assert_eq!(status.position_s, 0, "live radio has no position");
        assert_eq!(status.volume, 38, "34 of 90");
        assert!(!status.play_session.is_empty());
        // Then a pause.
        let paused = state_after(&objects);
        assert_eq!(paused.mode, Some(Mode::Pause));
    }

    #[test]
    fn cast_stream_from_the_feed() {
        let state = state_after(&fixture("cast"));
        assert_eq!(state.source(), "googlecast");
        let status = state.to_status();
        assert_eq!(status.source, "cast");
        assert_eq!(status.title, "Radio Paradise Main Mix");
        assert_eq!(status.mode, Some(Mode::Play));
        assert!(status.live, "no duration");
        assert_eq!(status.volume, 36, "32 of 90");
    }

    #[test]
    fn standby_from_the_feed() {
        let mut state = state_after(&fixture("beoradio"));
        let shutdown = &fixture("standby")[0];
        apply_notification(&mut state, shutdown);
        assert!(state.standby);
        assert_eq!(state.to_status().power, SpeakerPower::Standby);
        assert!(state.title.is_empty());
        // The empty SOURCE and the reset volume that follow keep it in standby.
        for object in &fixture("standby")[1..] {
            apply_notification(&mut state, object);
        }
        assert!(state.standby);
        assert_eq!(state.level, 34);
    }

    #[test]
    fn levels_scale_between_loxone_and_the_speaker_range() {
        assert_eq!(to_loxone_level(90, 90), 100);
        assert_eq!(to_loxone_level(45, 90), 50);
        assert_eq!(to_loxone_level(0, 90), 0);
        assert_eq!(to_loxone_level(95, 90), 100);
        assert_eq!(to_loxone_level(10, 0), 0);
        assert_eq!(to_speaker_level(100, 90), 90);
        assert_eq!(to_speaker_level(50, 90), 45);
        assert_eq!(to_speaker_level(36, 90), 32);
        for level in 0..=100u8 {
            let back = to_loxone_level(to_speaker_level(level, 90), 90);
            assert!(back.abs_diff(level) <= 1, "{level} -> {back}");
        }
    }

    #[test]
    fn a_loxone_step_always_moves_the_speaker() {
        // Shown as 36 (speaker 32): one step down asked for 35, which rounds back to 32.
        assert_eq!(to_speaker_level(35, 90), 32);
        assert_eq!(step_level(35, 32, 36, 90), 31);
        assert_eq!(step_level(37, 32, 36, 90), 33);
        // Walking down from 100 by Loxone steps reaches 0 without sticking.
        let mut current = 90;
        for _ in 0..200 {
            let shown = to_loxone_level(current, 90);
            if shown == 0 {
                break;
            }
            let next = step_level(shown - 1, current, shown, 90);
            assert!(next < current, "stuck at speaker level {current}");
            current = next;
        }
        assert_eq!(current, 0);
        // Same level, edges and an absolute jump.
        assert_eq!(step_level(36, 32, 36, 90), 32);
        assert_eq!(step_level(0, 0, 0, 90), 0);
        assert_eq!(step_level(100, 90, 100, 90), 90);
        assert_eq!(step_level(50, 10, 11, 90), 45);
    }

    #[test]
    fn quick_steps_each_move_the_speaker() {
        let now = std::time::Instant::now();
        let sent = |raw, requested| {
            Some(SentLevel {
                raw,
                requested,
                at: now,
            })
        };
        assert_eq!(step_base(32, 90, None), (32, 36));
        assert_eq!(step_base(32, 90, sent(30, 34)), (30, 34));
        let stale = Some(SentLevel {
            raw: 30,
            requested: 34,
            at: now - Duration::from_secs(10),
        });
        assert_eq!(step_base(32, 90, stale), (32, 36), "feed caught up");

        // Quick presses down from 36 while the feed still reports 32: every press moves.
        let mut last = None;
        let mut requested = 36;
        let mut levels = Vec::new();
        for _ in 0..5 {
            requested -= 1;
            let (current, reference) = step_base(32, 90, last);
            let raw = step_level(requested, current, reference, 90);
            levels.push(raw);
            last = sent(raw, requested);
        }
        assert_eq!(levels, vec![31, 30, 29, 28, 27]);
    }

    #[test]
    fn progress_counts_seconds_or_milliseconds() {
        assert_eq!(progress_seconds(42, 200, false), 42);
        assert_eq!(progress_seconds(42_000, 200, false), 42);
        assert_eq!(progress_seconds(1_060_267, 0, true), 0);
    }

    #[test]
    fn stored_music_track() {
        let mut state = BeoState::default();
        apply_notification(
            &mut state,
            &json!({"notification":{"type":"NOW_PLAYING_STORED_MUSIC","data":{
                "name":"Our House","artist":"Madness","album":"The Rise & Fall","duration":203,
                "trackImage":[{"url":"http://img/small","size":"small"},{"url":"http://img/large","size":"large"}],
                "playQueueItemId":"spotify"}}}),
        );
        apply_notification(
            &mut state,
            &json!({"notification":{"type":"PROGRESS_INFORMATION","data":{"state":"play","position":61}}}),
        );
        let status = state.to_status();
        assert_eq!(
            (
                status.title.as_str(),
                status.artist.as_str(),
                status.album.as_str()
            ),
            ("Our House", "Madness", "The Rise & Fall")
        );
        assert_eq!(status.cover_url, "http://img/large");
        assert_eq!(status.duration_s, 203);
        assert_eq!(status.position_s, 61);
        assert!(!status.live);
        apply_notification(
            &mut state,
            &json!({"notification":{"type":"NOW_PLAYING_ENDED","data":{}}}),
        );
        assert!(state.title.is_empty());
        assert_eq!(state.mode, Some(Mode::Stop));
    }

    #[test]
    fn drain_keeps_a_partial_object_for_the_next_chunk() {
        let mut buffer =
            b"{\"notification\":{\"type\":\"VOLUME\"}}\n\n{\"notification\":{\"type\":\"SHU"
                .to_vec();
        let objects = drain_objects(&mut buffer);
        assert_eq!(objects.len(), 1);
        assert_eq!(buffer, b"\n\n{\"notification\":{\"type\":\"SHU");
        buffer.extend_from_slice(b"TDOWN\"}}\n");
        assert_eq!(drain_objects(&mut buffer).len(), 1);
        assert!(buffer.iter().all(u8::is_ascii_whitespace));

        let mut junk = b"<html>oops".to_vec();
        assert!(drain_objects(&mut junk).is_empty());
        assert!(junk.is_empty(), "unparsable data is dropped");
    }

    #[test]
    fn a_character_split_across_chunks_survives() {
        let object =
            r#"{"notification":{"type":"NOW_PLAYING_NET_RADIO","data":{"name":"Beyoncé - Halo"}}}"#;
        let bytes = object.as_bytes();
        let split = object.find('é').unwrap() + 1; // inside the two-byte é
        let mut buffer = bytes[..split].to_vec();
        assert!(drain_objects(&mut buffer).is_empty(), "waits for the rest");
        buffer.extend_from_slice(&bytes[split..]);
        let objects = drain_objects(&mut buffer);
        let mut state = BeoState::default();
        apply_notification(&mut state, &objects[0]);
        assert_eq!(state.title, "Beyoncé - Halo");
    }

    #[test]
    fn an_experience_change_without_a_source_keeps_the_source() {
        let mut state = state_after(&fixture("beoradio")[..4]);
        apply_notification(
            &mut state,
            &json!({"notification":{"type":"SOURCE_EXPERIENCE_CHANGED","data":{}}}),
        );
        assert_eq!(state.source(), "beoradio");
        assert_eq!(state.title, "NPO Radio 2");
        // A real "nothing selected" comes as an empty SOURCE.
        apply_notification(
            &mut state,
            &json!({"notification":{"type":"SOURCE","data":{}}}),
        );
        assert_eq!(state.source(), "");
    }

    mod http {
        use super::*;
        use axum::{
            extract::State,
            routing::{get, post, put},
            Json, Router,
        };

        #[derive(Clone, Default)]
        struct Fake {
            feed: Arc<String>,
            standby: Arc<Mutex<String>>,
            writes: Arc<Mutex<Vec<String>>>,
        }

        async fn notifications(State(fake): State<Fake>) -> String {
            fake.feed.as_ref().clone()
        }

        async fn standby_get(State(fake): State<Fake>) -> Json<Value> {
            let state = fake.standby.lock().unwrap().clone();
            Json(json!({"standby":{"powerState": state}}))
        }

        async fn record(
            State(fake): State<Fake>,
            uri: axum::http::Uri,
            body: String,
        ) -> Json<Value> {
            fake.writes
                .lock()
                .unwrap()
                .push(format!("{} {body}", uri.path()));
            Json(json!({}))
        }

        async fn serve(fake: Fake) -> String {
            let router = Router::new()
                .route(PATH_NOTIFY, get(notifications))
                .route(PATH_STANDBY, get(standby_get).put(record))
                .route(PATH_LEVEL, put(record))
                .route(PATH_MUTED, put(record))
                .route(PATH_ACTIVE_SOURCES, post(record))
                .route("/BeoZone/Zone/Stream/{*action}", post(record))
                .with_state(fake);
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
            format!("http://{addr}")
        }

        /// The own-radio capture without its trailing pause: playing.
        fn playing_feed() -> String {
            let mut bytes =
                include_bytes!("../../tests/fixtures/speakers/beoplay_m3_notify_beoradio.jsonl")
                    .to_vec();
            drain_objects(&mut bytes)[..4]
                .iter()
                .map(|o| format!("{o}\n\n"))
                .collect()
        }

        fn fake_with(feed: String, power: &str) -> Fake {
            Fake {
                feed: Arc::new(feed),
                standby: Arc::new(Mutex::new(power.into())),
                ..Default::default()
            }
        }

        fn writes(fake: &Fake) -> Vec<String> {
            fake.writes.lock().unwrap().clone()
        }

        async fn status_once_fed(speaker: &Beo) -> SpeakerStatus {
            for _ in 0..50 {
                if let Ok(status) = speaker.status().await {
                    if !status.title.is_empty() || status.power != SpeakerPower::On {
                        return status;
                    }
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            panic!("feed never applied");
        }

        #[tokio::test]
        async fn status_comes_from_the_feed_and_commands_hit_the_api() {
            let fake = fake_with(playing_feed(), "on");
            let speaker = Beo::with_base(serve(fake.clone()).await);
            let status = status_once_fed(&speaker).await;
            assert_eq!(status.title, "NPO Radio 2");
            assert_eq!(status.mode, Some(Mode::Play));

            speaker.play().await.unwrap();
            speaker.pause().await.unwrap();
            speaker.set_volume(50).await.unwrap();
            speaker.power(false).await.unwrap();
            assert_eq!(
                writes(&fake),
                vec![
                    "/BeoZone/Zone/Stream/Play ",
                    "/BeoZone/Zone/Stream/Play/Release ",
                    "/BeoZone/Zone/Stream/Pause ",
                    "/BeoZone/Zone/Stream/Pause/Release ",
                    "/BeoZone/Zone/Sound/Volume/Speaker/Level {\"level\":45}",
                    "/BeoDevice/powerManagement/standby {\"standby\":{\"powerState\":\"standby\"}}",
                ]
            );
            assert!(speaker.command_timeout().unwrap() > COMMAND_TIMEOUT * 2);
        }

        #[tokio::test]
        async fn volume_is_refused_while_paused_and_a_repeat_is_kept() {
            let paused =
                include_str!("../../tests/fixtures/speakers/beoplay_m3_notify_beoradio.jsonl");
            let fake = fake_with(paused.to_string(), "on");
            let speaker = Beo::with_base(serve(fake.clone()).await);
            assert_eq!(status_once_fed(&speaker).await.mode, Some(Mode::Pause));
            assert!(
                speaker.set_volume(30).await.is_err(),
                "ignored by the speaker"
            );
            assert!(writes(&fake).is_empty());

            // Playing at speaker level 34 (shown 38): 37 sends 33; the same 37 again within
            // the settle window sends nothing.
            let fake = fake_with(playing_feed(), "on");
            let speaker = Beo::with_base(serve(fake.clone()).await);
            status_once_fed(&speaker).await;
            speaker.set_volume(37).await.unwrap();
            speaker.set_volume(37).await.unwrap();
            assert_eq!(
                writes(&fake),
                vec!["/BeoZone/Zone/Sound/Volume/Speaker/Level {\"level\":33}"]
            );
        }

        #[tokio::test]
        async fn own_radio_is_selected_again_after_a_message() {
            let feed = playing_feed();
            let fake = Fake {
                feed: Arc::new(feed),
                standby: Arc::new(Mutex::new("on".into())),
                ..Default::default()
            };
            let speaker = Beo::with_base(serve(fake.clone()).await);
            status_once_fed(&speaker).await;

            assert!(speaker.interrupt().await.unwrap());
            assert!(speaker.resume_interrupted().await.unwrap());
            assert!(!speaker.resume_interrupted().await.unwrap());
            let writes = writes(&fake);
            assert!(writes[0].starts_with("/BeoZone/Zone/ActiveSources "));
            assert!(writes[0].contains("beoradio:2730.1200316.00000000@products.bang-olufsen.com"));
            assert_eq!(writes[1], "/BeoZone/Zone/Stream/Play ");
        }

        #[tokio::test]
        async fn only_the_own_radio_is_kept_for_a_message() {
            let cast = include_str!("../../tests/fixtures/speakers/beoplay_m3_notify_cast.jsonl");
            let spotify = playing_feed().replace("beoradio:", "spotify:");
            for feed in [cast.to_string(), spotify] {
                let speaker = Beo::with_base(serve(fake_with(feed, "on")).await);
                status_once_fed(&speaker).await;
                assert!(!speaker.interrupt().await.unwrap());
            }
        }

        #[tokio::test]
        async fn a_speaker_asleep_when_the_feed_connects_is_in_standby() {
            // The snapshot of a sleeping M3: no source, its volume. No SHUTDOWN.
            let mut bytes = include_bytes!(
                "../../tests/fixtures/speakers/beoplay_m3_notify_standby_wake.jsonl"
            )
            .to_vec();
            let feed: String = drain_objects(&mut bytes)[1..]
                .iter()
                .map(|o| format!("{o}\n\n"))
                .collect();
            let speaker = Beo::with_base(serve(fake_with(feed, "standby")).await);
            assert_eq!(status_once_fed(&speaker).await.power, SpeakerPower::Standby);
        }

        #[tokio::test]
        async fn standby_is_left_when_the_speaker_reports_on() {
            let fake = Fake {
                feed: Arc::new(
                    include_str!(
                        "../../tests/fixtures/speakers/beoplay_m3_notify_standby_wake.jsonl"
                    )
                    .to_string(),
                ),
                standby: Arc::new(Mutex::new("standby".into())),
                ..Default::default()
            };
            let speaker = Beo::with_base(serve(fake.clone()).await);
            assert_eq!(status_once_fed(&speaker).await.power, SpeakerPower::Standby);
            *fake.standby.lock().unwrap() = "on".into();
            assert_eq!(speaker.status().await.unwrap().power, SpeakerPower::On);
        }

        #[tokio::test]
        async fn unreachable_speaker_fails_the_poll() {
            let speaker = Beo::with_base("http://127.0.0.1:9".into());
            // Nothing reported yet: no placeholder status (on, volume 0).
            assert!(speaker.status().await.is_err());
        }
    }
}
