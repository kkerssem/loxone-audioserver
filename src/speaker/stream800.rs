//! StreamUnlimited ("Stream800") speakers: KEF W2 platform (LS60, LSX II, LS50 Wireless II) and
//! Naim Mu-so 2nd generation.
//!
//! Both expose the same local HTTP API on port 80: `GET /api/getData?path=…&roles=value` and
//! `POST /api/setData`. Power differs by vendor:
//! - KEF: `settings:/kef/play/physicalSource`, which is also the active input. In standby it
//!   reads `standby` (auto-standby) or `powerOn` (standby from the KEF app — verified on an LS60:
//!   the app shows standby while the source is `powerOn`). Waking selects an input.
//! - Naim: its own API on port 15081, `GET /power` (`system`: `on` / `lona`) and
//!   `PUT /power?system=on|lona`. `GET /nowplaying?cmd=play` restarts a stopped radio station.
//!
//! Live radio (vTuner, airable) lists no `pause` control: the toggle is ignored (Naim) or
//! answered with HTTP 500 (KEF LS60). Pause therefore sends `stop`, which clears the player
//! data, so `play` cannot resume it. Play restarts it through the Naim `/nowplaying` resume, or
//! by sending `play` with the stopped stream's `mediaRoles` (both vendors; verified on an LS60).
//! While it can, the stopped station is reported as paused, not as an empty player.
//!
//! The API is unofficial (reverse-engineered; reference: pykefcontrol). Paths were checked
//! against an LS60 and a Mu-so Qb 2 on 2026-11-14.

use std::time::Duration;

use std::sync::Mutex;
use std::time::Instant;

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use tracing::{debug, warn};

use crate::player_state::Mode;

use super::{
    ServiceRows, SpeakerBackend, SpeakerPower, SpeakerStation, SpeakerStatus,
    STATION_AUDIOPATH_PREFIX,
};

const PATH_PLAYER_DATA: &str = "player:player/data";
const PATH_PLAY_TIME: &str = "player:player/data/playTime";
const PATH_PLAYER_CONTROL: &str = "player:player/control";
const PATH_VOLUME: &str = "player:volume";
const PATH_MUTE: &str = "settings:/mediaPlayer/mute";
const PATH_KEF_SOURCE: &str = "settings:/kef/play/physicalSource";
const NAIM_PORT: u16 = 15081;

/// How long a sent play/pause toggle counts as the speaker's state. The speaker reports the
/// new state a moment late; a second command in that gap must not read the old state and
/// toggle back (double tap, or the same command arriving on both 7091 and 7095).
const TOGGLE_SETTLE: Duration = Duration::from_secs(3);
/// Re-reads of a transitional player state (e.g. buffering) before deciding on play/pause.
const STATE_REREADS: u32 = 3;
const STATE_REREAD_DELAY: Duration = Duration::from_millis(300);

/// What play/pause needs to know about the player.
#[derive(Clone, Debug, PartialEq, Eq)]
struct PlayerView {
    state: String,
    /// Stopped with an empty queue (after standby): `play` would fail.
    nothing_to_resume: bool,
    /// `false` for live radio, which has no `pause` control: it is stopped instead.
    can_pause: bool,
    /// Live stream description (`mediaRoles` with a `path`) that `play` can restart after a
    /// stop clears the player.
    replay_media: Option<Value>,
    /// Play session id (`playId.timestamp`), see [`play_session`].
    play_session: String,
}

/// Live stream stopped by pause and kept so play can replay it (see
/// [`Stream800::resume_cleared_player`]).
#[derive(Clone, Debug, PartialEq)]
struct LiveStream {
    media: Value,
    /// Play session it runs under. `None` right after a replay, until a poll adopts the new
    /// session. A poll that sees a different session playing drops the stream.
    session: Option<String>,
}

/// A control sent less than [`TOGGLE_SETTLE`] ago and the state it leads to (`playing`,
/// `paused` or `stopped`).
#[derive(Clone, Copy, Debug)]
struct Pending {
    state: &'static str,
    /// `None` when unknown (after a Naim resume, which may be radio or a pausable track).
    can_pause: Option<bool>,
    sent: Instant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Vendor {
    Kef,
    Naim,
}

pub struct Stream800 {
    host: String,
    vendor: Vendor,
    http: reqwest::Client,
    /// Naim API base, `http://<host>:15081` on real devices.
    naim_base: String,
    /// Last play/pause/stop sent: the player state it leads to.
    pending_toggle: Mutex<Option<Pending>>,
    /// KEF: last real input seen (`wifi`, `tv`, …), selected again to wake the speaker.
    last_input: Mutex<Option<String>>,
    /// Live stream stopped by pause, replayed by the next play on the cleared player. Dropped
    /// when another stream plays, on standby and on power off, so a later wake does not
    /// restart old radio.
    live_stream: Mutex<Option<LiveStream>>,
    /// Live stream that was playing when a voice message took the speaker over (see
    /// [`SpeakerBackend::interrupt`]); replayed from its `mediaRoles` afterwards.
    interrupted: Mutex<Option<Value>>,
    /// Last poll that played a station from the speaker's own app, shown as paused once a
    /// pause has stopped it (see [`stopped_station_as_paused`]).
    last_station: Mutex<Option<SpeakerStatus>>,
}

impl Stream800 {
    pub fn new(host: &str, vendor: Vendor) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_millis(1500))
            .build()
            .unwrap_or_default();
        Self {
            host: host.to_string(),
            vendor,
            http,
            naim_base: format!("http://{host}:{NAIM_PORT}"),
            pending_toggle: Mutex::new(None),
            last_input: Mutex::new(None),
            live_stream: Mutex::new(None),
            interrupted: Mutex::new(None),
            last_station: Mutex::new(None),
        }
    }

    async fn get_data(&self, path: &str) -> Result<Value> {
        let url = format!("http://{}/api/getData", self.host);
        let body: Value = self
            .http
            .get(&url)
            .query(&[("path", path), ("roles", "value")])
            .send()
            .await
            .with_context(|| format!("getData {path}"))?
            .json()
            .await
            .with_context(|| format!("getData {path}: invalid json"))?;
        unwrap_get_data(path, body)
    }

    async fn set_data(&self, path: &str, role: &str, value: Value) -> Result<()> {
        let url = format!("http://{}/api/setData", self.host);
        let response = self
            .http
            .post(&url)
            .json(&json!({ "path": path, "role": role, "value": value }))
            .send()
            .await
            .with_context(|| format!("setData {path}"))?;
        if !response.status().is_success() {
            bail!("setData {path}: http {}", response.status());
        }
        let body: Value = response.json().await.unwrap_or(Value::Null);
        if let Some(err) = body.get("error") {
            bail!("setData {path}: {err}");
        }
        Ok(())
    }

    /// One page of a folder: `getRows` answers `{"rows":[…],"rowsCount":n}`. Its `to` is
    /// inclusive (`from=0&to=3` returns four rows; Mu-so Qb 2, 2026-11-18).
    async fn get_rows(&self, path: &str, start: usize, count: usize) -> Result<ServiceRows> {
        if count == 0 {
            return Ok(ServiceRows::default());
        }
        let url = format!("http://{}/api/getRows", self.host);
        let body: Value = self
            .http
            .get(&url)
            .query(&[
                ("path", path),
                ("roles", "@all"),
                ("from", &start.to_string()),
                ("to", &(start + count - 1).to_string()),
            ])
            .send()
            .await
            .with_context(|| format!("getRows {path}"))?
            .json()
            .await
            .with_context(|| format!("getRows {path}: invalid json"))?;
        parse_rows(path, body)
    }

    async fn naim_power(&self) -> Result<Value> {
        let url = format!("{}/power", self.naim_base);
        Ok(self.http.get(&url).send().await?.json().await?)
    }

    async fn naim_now_playing(&self, url: &str) -> Result<Value> {
        let response = self.http.get(url).send().await?;
        if !response.status().is_success() {
            bail!("http {}", response.status());
        }
        Ok(response.json().await?)
    }

    /// Naim: restart what `/nowplaying` remembers (e.g. a radio station after `stop`, which
    /// clears the port-80 player data). Returns whether a resume was sent.
    async fn naim_resume(&self) -> Result<bool> {
        let url = format!("{}/nowplaying", self.naim_base);
        let now_playing = match self.naim_now_playing(&url).await {
            Ok(now_playing) => now_playing,
            Err(e) => {
                // Same as the power lookup on this port: a hiccup is not a failed command.
                debug!(host = %self.host, error = %e, "naim nowplaying lookup failed");
                return Ok(false);
            }
        };
        if !naim_can_resume(&now_playing) {
            return Ok(false);
        }
        // Verified on a Mu-so Qb 2: `GET` restarts the station; `PUT` answers 200 but does nothing.
        let response = self.http.get(&url).query(&[("cmd", "play")]).send().await?;
        if !response.status().is_success() {
            bail!("naim resume: http {}", response.status());
        }
        Ok(true)
    }

    /// Power and (KEF only) physical input. KEF reads both from one source request. A failed
    /// Naim power lookup (separate port 15081) assumes on, so a hiccup there does not mark a
    /// speaker that answers on port 80 as unreachable.
    async fn power_and_source(&self) -> Result<(SpeakerPower, Option<String>)> {
        match self.vendor {
            Vendor::Kef => {
                let source = self.get_data(PATH_KEF_SOURCE).await?;
                let input = kef_physical_source(&source).map(str::to_string);
                if let Some(input) = input.as_deref().filter(|i| is_kef_input(i)) {
                    *self.last_input.lock().expect("input lock") = Some(input.to_string());
                }
                Ok((kef_power(&source), input))
            }
            Vendor::Naim => match self.naim_power().await {
                Ok(power) => Ok((naim_power(&power), None)),
                Err(e) => {
                    debug!(host = %self.host, error = %e, "naim power lookup failed; assuming on");
                    Ok((SpeakerPower::On, None))
                }
            },
        }
    }

    async fn control(&self, action: &str) -> Result<()> {
        self.set_data(
            PATH_PLAYER_CONTROL,
            "activate",
            json!({ "control": action }),
        )
        .await
    }

    async fn player_view(&self) -> Result<PlayerView> {
        Ok(player_view(&self.get_data(PATH_PLAYER_DATA).await?))
    }

    /// Player view, re-read a few times while the state is transitional (e.g. `buffering`).
    async fn settled_player_view(&self) -> Result<PlayerView> {
        let mut view = self.player_view().await?;
        for _ in 0..STATE_REREADS {
            if is_settled_state(&view.state) {
                break;
            }
            tokio::time::sleep(STATE_REREAD_DELAY).await;
            view = self.player_view().await?;
        }
        Ok(view)
    }

    /// Move to `want_playing` (see [`control_for`]). Within [`TOGGLE_SETTLE`] of the last control
    /// the state it leads to is taken as current instead of the speaker's (possibly stale) one.
    async fn set_playing(&self, want_playing: bool) -> Result<()> {
        let pending = *self.pending_toggle.lock().expect("toggle lock");
        // The player data read for this command, if any (reused by the stop path).
        let mut read = None;
        let (state, can_pause) = match assumed_state(pending, Instant::now()) {
            // Just stopped: the speaker may still read `playing`, but its player is being
            // cleared. Play goes straight to the resume path; a repeated pause does nothing.
            Some(p) if p.state == "stopped" => {
                return if want_playing {
                    self.resume_cleared_player().await
                } else {
                    Ok(())
                };
            }
            Some(Pending {
                state,
                can_pause: Some(can_pause),
                ..
            }) => (state.to_string(), can_pause),
            Some(Pending {
                state,
                can_pause: None,
                ..
            }) => {
                // State known from the last control; whether it pauses comes from the speaker.
                let view = self.player_view().await.ok();
                let can_pause = view.as_ref().is_none_or(|v| v.can_pause);
                read = Some(view);
                (state.to_string(), can_pause)
            }
            None => {
                let view = self.settled_player_view().await?;
                if want_playing && view.nothing_to_resume {
                    return self.resume_cleared_player().await;
                }
                let state = (view.state.clone(), view.can_pause);
                read = Some(Some(view));
                state
            }
        };
        let Some(action) = control_for(&state, want_playing, can_pause) else {
            return Ok(());
        };
        if action == "stop" {
            let view = match read {
                Some(view) => view,
                None => self.player_view().await.ok(),
            };
            self.save_live_stream(view);
        }
        self.control(action).await?;
        // Replaces any earlier pending toggle, so a stop is never followed by a stale target.
        self.remember(state_after(action, want_playing), Some(can_pause));
        Ok(())
    }

    /// Keep the live stream about to be stopped. Without a replayable description (read failed,
    /// or no `path`) a stream already kept (e.g. just replayed) stays; with neither, play can only
    /// restart it through the Naim resume.
    fn save_live_stream(&self, view: Option<PlayerView>) {
        let fresh = view.and_then(|v| {
            v.replay_media.map(|media| LiveStream {
                media,
                session: Some(v.play_session),
            })
        });
        let mut saved = self.live_stream.lock().expect("stream lock");
        match fresh {
            Some(stream) => *saved = Some(stream),
            None if saved.is_some() => {}
            None if self.vendor == Vendor::Naim => {
                debug!(host = %self.host, "stopping live radio without a replayable stream");
            }
            None => {
                warn!(host = %self.host, "stopping live radio without a replayable stream; play cannot restart it");
            }
        }
    }

    fn remember(&self, state: &'static str, can_pause: Option<bool>) {
        *self.pending_toggle.lock().expect("toggle lock") = Some(Pending {
            state,
            can_pause,
            sent: Instant::now(),
        });
    }

    /// Play on a stopped player with no track (after standby, or live radio after `stop`):
    /// port-80 `play` would fail ("Directory is empty"). A Naim restarts what its 15081
    /// `/nowplaying` remembers; otherwise the live stream stopped by the last pause is replayed
    /// from its `mediaRoles` (verified on an LS60 with airable radio, 2026-11-14). With neither,
    /// the speaker can only be woken — music is started from its app.
    async fn resume_cleared_player(&self) -> Result<()> {
        if self.vendor == Vendor::Naim {
            match self.naim_resume().await {
                Ok(true) => {
                    // What resumed (radio or a pausable track) is read from the speaker on
                    // the next pause.
                    self.remember("playing", None);
                    return Ok(());
                }
                Ok(false) => {}
                Err(e) => {
                    debug!(host = %self.host, error = %e, "naim resume failed; trying the kept stream");
                }
            }
        }
        let media = self
            .live_stream
            .lock()
            .expect("stream lock")
            .as_ref()
            .map(|stream| stream.media.clone());
        let Some(media) = media else {
            debug!(host = %self.host, "play ignored: nothing to resume");
            return Ok(());
        };
        // On error the stream stays kept, so another play can retry.
        self.replay_stream(media).await
    }

    /// Play a live stream from its `mediaRoles` and keep it, so a pause (which stops live
    /// radio) and play restart it.
    async fn replay_stream(&self, media: Value) -> Result<()> {
        self.set_data(
            PATH_PLAYER_CONTROL,
            "activate",
            json!({ "control": "play", "mediaRoles": media.clone() }),
        )
        .await?;
        *self.live_stream.lock().expect("stream lock") = Some(LiveStream {
            media,
            session: None,
        });
        // The replayed stream is live radio: a pause right after must stop it, even before the
        // speaker reports it as live.
        self.remember("playing", Some(false));
        Ok(())
    }

    /// Remember the station a poll plays, and show it as paused while the player reads as
    /// stopped and play can restart it: on a Naim through `/nowplaying`, otherwise from the
    /// live stream kept by the pause.
    fn present_stopped_station(&self, status: SpeakerStatus) -> SpeakerStatus {
        let mut last = self.last_station.lock().expect("station lock");
        if status.power != SpeakerPower::On {
            *last = None;
            return status;
        }
        if status.mode == Some(Mode::Play) {
            *last = status.station.is_some().then(|| status.clone());
            return status;
        }
        let can_restart =
            self.vendor == Vendor::Naim || self.live_stream.lock().expect("stream lock").is_some();
        stopped_station_as_paused(&status, last.as_ref(), can_restart).unwrap_or(status)
    }

    /// Track the kept live stream from a poll: standby drops it; while playing, a stream just
    /// replayed adopts the new session and any other session drops it. A late `playing` read of
    /// the stopped session itself keeps it.
    fn observe_live_stream(&self, status: &SpeakerStatus) {
        let mut saved = self.live_stream.lock().expect("stream lock");
        if status.power != SpeakerPower::On {
            *saved = None;
            return;
        }
        if status.mode != Some(Mode::Play) {
            return;
        }
        let Some(stream) = saved.as_mut() else {
            return;
        };
        match &stream.session {
            None if !status.play_session.is_empty() => {
                stream.session = Some(status.play_session.clone());
            }
            Some(session) if *session != status.play_session => *saved = None,
            _ => {}
        }
    }
}

/// Live radio has no pause: Loxone's pause stops it and the player then reads as empty, which
/// the Loxone App shows as "No music selected". While play can restart the station, show it
/// as paused instead, at the speaker's current volume.
fn stopped_station_as_paused(
    status: &SpeakerStatus,
    last: Option<&SpeakerStatus>,
    can_restart: bool,
) -> Option<SpeakerStatus> {
    // A KEF on another input (TV, optical, …) reads that input as its source: not the radio.
    let stopped_empty = status.mode.is_none() && status.title.is_empty() && status.source == "wifi";
    if !(stopped_empty && can_restart) {
        return None;
    }
    let last = last?;
    Some(SpeakerStatus {
        power: status.power,
        mode: Some(Mode::Pause),
        volume: status.volume,
        muted: status.muted,
        position_s: 0,
        play_session: status.play_session.clone(),
        ..last.clone()
    })
}

/// A `getRows` reply as a page of rows. Errors come back as `{"error":{"message":…}}`.
fn parse_rows(path: &str, body: Value) -> Result<ServiceRows> {
    if let Some(err) = body.get("error") {
        let message = err
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        bail!("getRows {path}: {message}");
    }
    let rows = body
        .get("rows")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let total = body
        .get("rowsCount")
        .and_then(Value::as_u64)
        .map(|n| n as usize)
        .unwrap_or(rows.len());
    Ok(ServiceRows { total, rows })
}

/// Player state after sending `action` towards `want_playing`.
fn state_after(action: &str, want_playing: bool) -> &'static str {
    match (action, want_playing) {
        ("stop", _) => "stopped",
        (_, true) => "playing",
        (_, false) => "paused",
    }
}

/// The toggle sent less than [`TOGGLE_SETTLE`] ago, if any.
fn assumed_state(pending: Option<Pending>, now: Instant) -> Option<Pending> {
    pending.filter(|p| now.saturating_duration_since(p.sent) < TOGGLE_SETTLE)
}

/// Naim `/nowplaying` remembers something to restart. Seen as `"1"` on a Mu-so Qb 2;
/// `1` and `true` are accepted too.
fn naim_can_resume(now_playing: &Value) -> bool {
    match now_playing.get("canResume") {
        Some(Value::String(s)) => s == "1" || s.eq_ignore_ascii_case("true"),
        Some(Value::Number(n)) => n.as_u64() == Some(1),
        Some(Value::Bool(b)) => *b,
        _ => false,
    }
}

fn player_view(player_data: &Value) -> PlayerView {
    PlayerView {
        state: player_data
            .get("state")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        nothing_to_resume: nothing_to_resume(player_data),
        can_pause: can_pause(player_data),
        replay_media: replay_media(player_data),
        play_session: play_session(player_data),
    }
}

/// `mediaRoles` of a live stream, if it carries the `path` the speaker needs to play it again.
fn replay_media(player_data: &Value) -> Option<Value> {
    let media = player_data.get("mediaRoles")?;
    let has_path = media
        .get("path")
        .and_then(Value::as_str)
        .is_some_and(|p| !p.is_empty());
    // A Cast stream also has a `path`, but only its Cast sender can start it again: replaying
    // it here is accepted and plays nothing, and `play` fails afterwards (LS60, 2026-11-15).
    let from_cast = media
        .pointer("/mediaData/metaData/serviceID")
        .and_then(Value::as_str)
        == Some("googlecast");
    (is_live(player_data) && has_path && !from_cast).then(|| media.clone())
}

/// Live radio (vTuner, airable) lists no `pause` control and ignores the toggle. Other
/// players are paused even if `controls` is missing.
fn can_pause(player_data: &Value) -> bool {
    !is_live(player_data)
        || player_data
            .pointer("/controls/pause")
            .and_then(Value::as_bool)
            == Some(true)
}

/// A stopped player with no track: after standby the speaker's queue is empty and `play` fails.
fn nothing_to_resume(player_data: &Value) -> bool {
    player_data.get("state").and_then(Value::as_str) == Some("stopped")
        && str_at(player_data, "/trackRoles/title").is_empty()
}

/// A KEF physical source that is a real input (not a power state).
fn is_kef_input(source: &str) -> bool {
    !matches!(source, "standby" | "powerOn")
}

fn is_settled_state(state: &str) -> bool {
    matches!(state, "playing" | "paused" | "stopped" | "")
}

/// Control action that moves the player from `state` to playing / paused.
///
/// On this API `pause` is a **toggle** and `play` does not resume a paused stream (checked on a
/// Mu-so Qb 2 with Qobuz Connect, 2026-11-14). So: resume = `pause` while paused, start = `play`
/// while stopped, and nothing when already in the wanted state — sending `pause` to a paused
/// speaker would resume it. For a state still unknown after re-reading (transitional), a
/// pause does nothing rather than risk toggling the wrong way.
///
/// Live radio cannot be paused (`can_pause` false): it is stopped, and `play` restarts it.
pub fn control_for(state: &str, want_playing: bool, can_pause: bool) -> Option<&'static str> {
    match (state, want_playing) {
        ("playing", false) if !can_pause => Some("stop"),
        ("paused", true) | ("playing", false) => Some("pause"),
        (_, true) if state != "playing" => Some("play"),
        _ => None,
    }
}

/// `getData` answers a one-element array with the value, or `{ "error": … }`.
fn unwrap_get_data(path: &str, body: Value) -> Result<Value> {
    if let Some(err) = body.get("error") {
        let message = err
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        bail!("getData {path}: {message}");
    }
    match body {
        Value::Array(mut items) if !items.is_empty() => Ok(items.swap_remove(0)),
        other => bail!("getData {path}: unexpected reply {other}"),
    }
}

fn typed<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    value.get(key)
}

fn kef_physical_source(value: &Value) -> Option<&str> {
    typed(value, "kefPhysicalSource").and_then(Value::as_str)
}

/// KEF power from the physical source: `standby` (auto-standby) and `powerOn` (standby from
/// the KEF app, no input selected) are both standby.
pub fn kef_power(source: &Value) -> SpeakerPower {
    match kef_physical_source(source) {
        Some(s) if !is_kef_input(s) => SpeakerPower::Standby,
        _ => SpeakerPower::On,
    }
}

/// Naim `/power` → `system` is `on`, or `lona` / `standby` when asleep.
pub fn naim_power(power: &Value) -> SpeakerPower {
    match power.get("system").and_then(Value::as_str) {
        Some("on") => SpeakerPower::On,
        Some(_) => SpeakerPower::Standby,
        None => SpeakerPower::On,
    }
}

/// Map a KEF physical input to the vendor-neutral source ids used by [`super::map`].
fn normalize_kef_source(source: &str) -> &str {
    match source {
        "optic" => "optical",
        "analog" => "analog",
        other => other,
    }
}

/// Streaming service behind the current track (`mediaRoles…serviceID`), if any.
fn service_id(player_data: &Value) -> Option<&str> {
    player_data
        .pointer("/mediaRoles/mediaData/metaData/serviceID")
        .and_then(Value::as_str)
}

fn source_for(physical: Option<&str>, player_data: &Value) -> String {
    // `powerOn` is reported briefly while a KEF wakes up; it is not an input.
    if let Some(input) = physical.filter(|s| !matches!(*s, "wifi" | "standby" | "powerOn")) {
        return normalize_kef_source(input).to_string();
    }
    match service_id(player_data)
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("airplay") => "airplay".to_string(),
        Some("spotify") => "spotify".to_string(),
        Some("googlecast") => "cast".to_string(),
        // Built-in internet radio: KEF uses airable, Naim uses vTuner.
        Some("airableradios" | "vtuner" | "tunein") => "radio".to_string(),
        Some(other) if !other.is_empty() => other.to_string(),
        _ => "wifi".to_string(),
    }
}

fn str_at<'a>(value: &'a Value, pointer: &str) -> &'a str {
    value.pointer(pointer).and_then(Value::as_str).unwrap_or("")
}

/// Build a [`SpeakerStatus`] from the raw `getData` values (pure, fixture-tested).
pub fn parse_status(
    power: SpeakerPower,
    player_data: &Value,
    volume: &Value,
    mute: Option<&Value>,
    play_time_ms: Option<&Value>,
    physical_source: Option<&str>,
) -> SpeakerStatus {
    let mode = match player_data.get("state").and_then(Value::as_str) {
        Some("playing") => Some(Mode::Play),
        Some("paused") => Some(Mode::Pause),
        _ => None,
    };
    let cover_url = cover_url(player_data);
    let duration_ms = player_data
        .pointer("/status/duration")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let position_ms = play_time_ms
        .and_then(|v| v.get("i64_"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    SpeakerStatus {
        power,
        mode,
        volume: volume
            .get("i32_")
            .and_then(Value::as_i64)
            .unwrap_or(0)
            .clamp(0, 100) as u8,
        muted: mute
            .and_then(|m| m.get("bool_"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        source: source_for(physical_source, player_data),
        title: str_at(player_data, "/trackRoles/title").to_string(),
        artist: str_at(player_data, "/trackRoles/mediaData/metaData/artist").to_string(),
        album: str_at(player_data, "/trackRoles/mediaData/metaData/album").to_string(),
        cover_url,
        duration_s: (duration_ms / 1000) as u32,
        position_s: position_ms / 1000,
        live: is_live(player_data),
        station_name: station_name(player_data),
        play_session: play_session(player_data),
        station: station(player_data),
        track_path: str_at(player_data, "/trackRoles/path").to_string(),
    }
}

/// A live stream from the speaker's own app that its `mediaRoles` can start again (see
/// [`replay_media`]), keyed by its `path` so the same station keeps the same audiopath.
fn station(player_data: &Value) -> Option<std::sync::Arc<SpeakerStation>> {
    let media = replay_media(player_data)?;
    let path = media.get("path").and_then(Value::as_str)?;
    Some(std::sync::Arc::new(SpeakerStation {
        audiopath: format!(
            "{STATION_AUDIOPATH_PREFIX}{}",
            crate::config::crc32_hex(path.as_bytes())
        ),
        media,
    }))
}

/// Name of the live station that plays (`mediaRoles.title`); the track title of a radio stream
/// is often the song (`"The The - Cognitive Dissident"`, airable on an LS60, 2026-11-19).
fn station_name(player_data: &Value) -> String {
    if is_live(player_data) {
        str_at(player_data, "/mediaRoles/title").to_string()
    } else {
        String::new()
    }
}

/// Track icon, if it is an http(s) URL.
fn cover_url(player_data: &Value) -> String {
    let cover = str_at(player_data, "/trackRoles/icon");
    if cover.starts_with("http://") || cover.starts_with("https://") {
        cover.to_string()
    } else {
        String::new()
    }
}

/// Play session id (`playId.timestamp`): constant while one stream plays, new on restart.
fn play_session(player_data: &Value) -> String {
    player_data
        .pointer("/playId/timestamp")
        .and_then(Value::as_u64)
        .map(|t| t.to_string())
        .unwrap_or_default()
}

fn is_live(player_data: &Value) -> bool {
    [
        "/trackRoles/mediaData/metaData/live",
        "/mediaRoles/mediaData/metaData/live",
    ]
    .iter()
    .any(|p| player_data.pointer(p).and_then(Value::as_bool) == Some(true))
}

#[async_trait]
impl SpeakerBackend for Stream800 {
    async fn status(&self) -> Result<SpeakerStatus> {
        let (power, source) = self.power_and_source().await?;
        if power == SpeakerPower::Standby {
            let status = SpeakerStatus {
                power,
                ..Default::default()
            };
            self.observe_live_stream(&status);
            return Ok(self.present_stopped_station(status));
        }
        let (player_data, volume, mute, play_time) = tokio::join!(
            self.get_data(PATH_PLAYER_DATA),
            self.get_data(PATH_VOLUME),
            self.get_data(PATH_MUTE),
            self.get_data(PATH_PLAY_TIME),
        );
        let status = parse_status(
            power,
            &player_data?,
            &volume?,
            mute.ok().as_ref(),
            play_time.ok().as_ref(),
            source.as_deref(),
        );
        self.observe_live_stream(&status);
        Ok(self.present_stopped_station(status))
    }

    async fn play(&self) -> Result<()> {
        self.set_playing(true).await
    }

    /// Keep the live stream that plays now (radio has a replayable `mediaRoles`). A pausable
    /// track (e.g. Qobuz) is not kept: once another source took over, `play` cannot resume it.
    async fn interrupt(&self) -> Result<bool> {
        let view = self.player_view().await?;
        let media = (view.state == "playing")
            .then_some(view.replay_media)
            .flatten();
        let resumable = media.is_some();
        *self.interrupted.lock().expect("interrupted lock") = media;
        Ok(resumable)
    }

    /// Replay the live stream kept by [`interrupt`](SpeakerBackend::interrupt), the same way
    /// play restarts stopped radio (`play` with its `mediaRoles`). Verified after a Cast voice
    /// message on an LS60 (airable) and a Mu-so Qb 2 (vTuner), 2026-11-15.
    async fn resume_interrupted(&self) -> Result<bool> {
        let Some(media) = self.interrupted.lock().expect("interrupted lock").take() else {
            return Ok(false);
        };
        self.set_data(
            PATH_PLAYER_CONTROL,
            "activate",
            json!({ "control": "play", "mediaRoles": media.clone() }),
        )
        .await?;
        // Kept like a stream replayed by play, so a pause right after (which stops live radio,
        // perhaps reading the message's player data) can still restart it; the next poll
        // adopts the new session.
        *self.live_stream.lock().expect("stream lock") = Some(LiveStream {
            media,
            session: None,
        });
        // Live radio again: a pause right after must stop it.
        self.remember("playing", Some(false));
        Ok(true)
    }

    async fn pause(&self) -> Result<()> {
        self.set_playing(false).await
    }

    /// Set the level and, for a non-zero level on a muted speaker, unmute it: Loxone shows a
    /// muted speaker as volume 0, so raising the volume must make it audible.
    async fn play_station(&self, media: &Value) -> Result<()> {
        self.replay_stream(media.clone()).await
    }

    /// Play an item (a `getRows` row or a description built from its path). Whether it pauses
    /// is read from the speaker on the next pause.
    async fn play_item(&self, media: &Value) -> Result<()> {
        self.set_data(
            PATH_PLAYER_CONTROL,
            "activate",
            json!({ "control": "play", "mediaRoles": media }),
        )
        .await?;
        self.remember("playing", None);
        Ok(())
    }

    /// Naim: `/nowplaying?cmd=next|prev` (port 80 `next_` / `previous` answer but do nothing on
    /// a Qobuz album, Mu-so 2026-11-18). Not checked on KEF yet.
    async fn skip(&self, forward: bool) -> Result<()> {
        if self.vendor != Vendor::Naim {
            bail!("skip not supported on KEF yet");
        }
        let url = format!("{}/nowplaying", self.naim_base);
        let cmd = if forward { "next" } else { "prev" };
        let response = self.http.get(&url).query(&[("cmd", cmd)]).send().await?;
        if !response.status().is_success() {
            bail!("naim {cmd}: http {}", response.status());
        }
        Ok(())
    }

    async fn browse(&self, path: &str, start: usize, count: usize) -> Result<ServiceRows> {
        self.get_rows(path, start, count).await
    }

    async fn seek(&self, seconds: u32) -> Result<()> {
        self.set_data(
            PATH_PLAYER_CONTROL,
            "activate",
            json!({ "control": "seekTime", "time": u64::from(seconds) * 1000 }),
        )
        .await
    }

    async fn set_volume(&self, level: u8) -> Result<()> {
        let level = level.min(100);
        self.set_data(
            PATH_VOLUME,
            "value",
            json!({ "type": "i32_", "i32_": i32::from(level) }),
        )
        .await?;
        if level > 0 {
            let muted = self
                .get_data(PATH_MUTE)
                .await
                .ok()
                .and_then(|m| m.get("bool_").and_then(Value::as_bool))
                .unwrap_or(false);
            if muted {
                self.set_data(
                    PATH_MUTE,
                    "value",
                    json!({ "type": "bool_", "bool_": false }),
                )
                .await?;
            }
        }
        Ok(())
    }

    async fn power(&self, on: bool) -> Result<()> {
        if !on {
            // Switched off: a stream kept from before must not restart on the next wake.
            *self.live_stream.lock().expect("stream lock") = None;
            *self.last_station.lock().expect("station lock") = None;
        }
        match self.vendor {
            Vendor::Kef => {
                // Writing `powerOn` leaves an LS60 in its app-standby state; selecting an input
                // wakes it. Use the last input seen, or Wi-Fi.
                let target = if on {
                    self.last_input
                        .lock()
                        .expect("input lock")
                        .clone()
                        .unwrap_or_else(|| "wifi".to_string())
                } else {
                    "standby".to_string()
                };
                self.set_data(
                    PATH_KEF_SOURCE,
                    "value",
                    json!({ "type": "kefPhysicalSource", "kefPhysicalSource": target }),
                )
                .await
            }
            Vendor::Naim => {
                let system = if on { "on" } else { "lona" };
                let url = format!("{}/power", self.naim_base);
                let response = self
                    .http
                    .put(&url)
                    .query(&[("system", system)])
                    .send()
                    .await
                    .context("naim power")?;
                if !response.status().is_success() {
                    bail!("naim power: http {}", response.status());
                }
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Value {
        let text = match name {
            "ls60_radio" => include_str!(
                "../../tests/fixtures/speakers/ls60_player_data_cast_radio_paused.json"
            ),
            "muso_radio" => include_str!(
                "../../tests/fixtures/speakers/muso_player_data_cast_radio_paused.json"
            ),
            "muso_power" => include_str!("../../tests/fixtures/speakers/muso_15081_power_on.json"),
            "ls60_airable" => include_str!(
                "../../tests/fixtures/speakers/ls60_player_data_airable_radio_playing.json"
            ),
            "muso_vtuner" => include_str!(
                "../../tests/fixtures/speakers/muso_player_data_vtuner_radio_playing.json"
            ),
            "ls60_qobuz" => include_str!(
                "../../tests/fixtures/speakers/ls60_player_data_qobuz_connect_playing.json"
            ),
            "muso_qobuz" => include_str!(
                "../../tests/fixtures/speakers/muso_player_data_qobuz_connect_playing.json"
            ),
            other => panic!("unknown fixture {other}"),
        };
        serde_json::from_str(text).unwrap()
    }

    fn first(v: Value) -> Value {
        unwrap_get_data("test", v).unwrap()
    }

    #[test]
    fn ls60_cast_radio_paused() {
        let status = parse_status(
            SpeakerPower::On,
            &first(fixture("ls60_radio")),
            &json!({"type":"i32_","i32_":31}),
            Some(&json!({"bool_":false,"type":"bool_"})),
            Some(&json!({"type":"i64_","i64_":104148})),
            Some("wifi"),
        );
        assert_eq!(status.mode, Some(Mode::Pause));
        assert_eq!(status.title, "NPO Radio 2");
        assert_eq!(status.volume, 31);
        assert!(!status.muted);
        assert_eq!(status.source, "cast");
        assert!(status.live);
        assert_eq!(status.position_s, 104);
        assert_eq!(status.duration_s, 0);
        assert!(status.cover_url.is_empty(), "skin: icons are not URLs");
    }

    #[test]
    fn muso_cast_radio_paused() {
        let status = parse_status(
            naim_power(&fixture("muso_power")),
            &first(fixture("muso_radio")),
            &json!({"type":"i32_","i32_":23}),
            None,
            None,
            None,
        );
        assert_eq!(status.power, SpeakerPower::On);
        assert_eq!(status.mode, Some(Mode::Pause));
        assert_eq!(status.title, "538 Non-Stop");
        assert_eq!(status.volume, 23);
        assert_eq!(status.source, "cast");
    }

    #[test]
    fn native_app_radio_on_both_speakers() {
        let ls60 = parse_status(
            SpeakerPower::On,
            &first(fixture("ls60_airable")),
            &json!({"i32_": 31}),
            None,
            Some(&json!({"i64_": 78000})),
            Some("wifi"),
        );
        assert_eq!(ls60.mode, Some(Mode::Play));
        assert_eq!(ls60.title, "Radio Paradise Main Mix");
        assert_eq!(ls60.source, "radio");
        assert!(ls60.live);
        assert_eq!(ls60.cover_url, "http://static.airable.io/45/99/143921.png");
        assert_eq!(ls60.duration_s, 0);

        let muso = parse_status(
            SpeakerPower::On,
            &first(fixture("muso_vtuner")),
            &json!({"i32_": 23}),
            None,
            None,
            None,
        );
        assert_eq!(muso.mode, Some(Mode::Play));
        assert_eq!(muso.title, "Radio Paradise - Main Mix");
        assert_eq!(muso.source, "radio");
        assert!(muso.live);
        assert_eq!(
            muso.cover_url,
            "https://media.focal-naim.com/app/images/stations/2822.png"
        );
        assert_eq!(muso.duration_s, 0, "duration -1 means unknown");
    }

    #[test]
    fn live_radio_keeps_the_station_name_apart_from_the_song() {
        let read = |data: &Value| {
            parse_status(
                SpeakerPower::On,
                data,
                &json!({"i32_": 40}),
                None,
                None,
                Some("wifi"),
            )
        };
        let mut song = first(fixture("ls60_airable"));
        song["trackRoles"]["title"] = json!("The The - Cognitive Dissident");
        let status = read(&song);
        assert_eq!(status.title, "The The - Cognitive Dissident");
        assert_eq!(status.station_name, "Radio Paradise Main Mix");

        let mut track = song.clone();
        track["trackRoles"]["mediaData"]["metaData"]["live"] = json!(false);
        track["mediaRoles"]["mediaData"]["metaData"]["live"] = json!(false);
        assert!(read(&track).station_name.is_empty(), "not radio");
    }

    #[test]
    fn native_app_radio_is_a_replayable_station() {
        let status = |data: &Value| {
            parse_status(
                SpeakerPower::On,
                data,
                &json!({"i32_": 23}),
                None,
                None,
                None,
            )
        };
        // The capture has its stream path stripped; put a short one back.
        let mut vtuner = first(fixture("muso_vtuner"));
        vtuner["mediaRoles"]["path"] = json!("vtuner:c3RhdGlvbg==");
        let station = status(&vtuner).station.expect("vtuner station");
        assert!(station.audiopath.starts_with(STATION_AUDIOPATH_PREFIX));
        assert_eq!(station.media, vtuner["mediaRoles"]);
        assert_eq!(
            status(&vtuner).station.unwrap().audiopath,
            station.audiopath,
            "same station, same audiopath"
        );
        let mut other = vtuner.clone();
        other["mediaRoles"]["path"] = json!("vtuner:b3RoZXI=");
        assert_ne!(status(&other).station.unwrap().audiopath, station.audiopath);

        assert!(
            status(&first(fixture("muso_vtuner"))).station.is_none(),
            "no path"
        );
        assert!(
            status(&first(fixture("muso_radio"))).station.is_none(),
            "cast radio"
        );
        assert!(
            status(&first(fixture("muso_qobuz"))).station.is_none(),
            "qobuz connect"
        );
        assert!(
            status(&first(fixture("ls60_qobuz"))).station.is_none(),
            "qobuz connect"
        );
    }

    #[test]
    fn qobuz_connect_track_on_both_speakers() {
        for name in ["ls60_qobuz", "muso_qobuz"] {
            let status = parse_status(
                SpeakerPower::On,
                &first(fixture(name)),
                &json!({"i32_": 30}),
                None,
                Some(&json!({"i64_": 12000})),
                None,
            );
            assert_eq!(status.mode, Some(Mode::Play), "{name}");
            assert_eq!(status.title, "The Black Crow", "{name}");
            assert_eq!(status.artist, "Songs: Ohia", "{name}");
            assert_eq!(status.album, "The Lioness", "{name}");
            assert_eq!(status.duration_s, 436, "{name}");
            assert_eq!(status.position_s, 12, "{name}");
            assert_eq!(
                status.cover_url,
                "https://static.qobuz.com/images/covers/61/30/0656605003061_600.jpg",
                "{name}"
            );
            assert!(!status.play_session.is_empty(), "{name}");
            assert_eq!(status.source, "qobuzconnect", "{name}");
            assert!(!status.live, "{name}: a track is not a live stream");
        }
    }

    #[test]
    fn music_track_metadata_and_cover() {
        let data = json!({
            "state": "playing",
            "status": {"duration": 431000},
            "trackRoles": {
                "title": "Hey Jude",
                "icon": "https://static.qobuz.com/images/covers/x.jpg",
                "mediaData": {"metaData": {"artist": "The Beatles", "album": "1"}}
            },
            "mediaRoles": {"mediaData": {"metaData": {"serviceID": "qobuz"}}}
        });
        let status = parse_status(
            SpeakerPower::On,
            &data,
            &json!({"i32_": 140}),
            Some(&json!({"bool_": true})),
            Some(&json!({"i64_": 61500})),
            Some("wifi"),
        );
        assert_eq!(status.mode, Some(Mode::Play));
        assert_eq!(status.artist, "The Beatles");
        assert_eq!(status.album, "1");
        assert_eq!(
            status.cover_url,
            "https://static.qobuz.com/images/covers/x.jpg"
        );
        assert_eq!(status.duration_s, 431);
        assert_eq!(status.position_s, 61);
        assert_eq!(status.volume, 100, "clamped");
        assert!(status.muted);
        assert_eq!(status.source, "qobuz");
    }

    #[test]
    fn physical_inputs_and_services_map_to_sources() {
        let empty = json!({});
        assert_eq!(source_for(Some("optic"), &empty), "optical");
        assert_eq!(source_for(Some("bluetooth"), &empty), "bluetooth");
        assert_eq!(source_for(Some("wifi"), &empty), "wifi");
        assert_eq!(
            source_for(Some("powerOn"), &empty),
            "wifi",
            "transient wake value is not an input"
        );
        assert_eq!(source_for(None, &empty), "wifi");
        let airplay = json!({"mediaRoles": {"mediaData": {"metaData": {"serviceID": "airplay"}}}});
        assert_eq!(source_for(Some("wifi"), &airplay), "airplay");
        let spotify = json!({"mediaRoles": {"mediaData": {"metaData": {"serviceID": "spotify"}}}});
        assert_eq!(source_for(None, &spotify), "spotify");
    }

    #[test]
    fn stopped_or_unknown_state_has_no_mode() {
        let status = parse_status(
            SpeakerPower::On,
            &json!({"state": "stopped", "trackRoles": {}}),
            &json!({"i32_": 10}),
            None,
            None,
            None,
        );
        assert_eq!(status.mode, None);
        assert!(status.title.is_empty());
    }

    #[test]
    fn play_pause_use_the_toggle_only_when_needed() {
        assert_eq!(control_for("paused", true, true), Some("pause"));
        assert_eq!(control_for("playing", false, true), Some("pause"));
        assert_eq!(control_for("playing", true, true), None);
        assert_eq!(control_for("paused", false, true), None, "must not resume");
        assert_eq!(control_for("stopped", false, true), None);
        assert_eq!(control_for("stopped", true, true), Some("play"));
        assert_eq!(control_for("", true, true), Some("play"));
        assert_eq!(
            control_for("buffering", false, true),
            None,
            "unknown state: do not toggle"
        );
        assert_eq!(control_for("buffering", true, true), Some("play"));
    }

    #[test]
    fn play_session_comes_from_play_id() {
        let track = |play: u64| json!({"playId": {"timestamp": play}});
        assert_eq!(play_session(&track(545638249)), "545638249");
        assert_ne!(play_session(&track(1)), play_session(&track(2)));
        assert_eq!(play_session(&json!({})), "");
        assert_eq!(
            cover_url(&json!({"trackRoles": {"icon": "skin:radio"}})),
            "",
            "not an http(s) URL"
        );
    }

    #[test]
    fn live_radio_is_stopped_and_restarted() {
        assert_eq!(control_for("playing", false, false), Some("stop"));
        assert_eq!(control_for("stopped", false, false), None);
        assert_eq!(control_for("stopped", true, false), Some("play"));
        for yes in [json!("1"), json!(1), json!(true), json!("true")] {
            assert!(naim_can_resume(&json!({"canResume": yes})), "{yes}");
        }
        for no in [json!("0"), json!(0), json!(false), json!(null)] {
            assert!(!naim_can_resume(&json!({"canResume": no})), "{no}");
        }
        assert!(!naim_can_resume(&json!({})));
    }

    #[test]
    fn only_live_streams_without_a_pause_control_cannot_pause() {
        assert!(!can_pause(&first(fixture("muso_vtuner"))));
        assert!(!can_pause(&first(fixture("ls60_airable"))));
        assert!(can_pause(&first(fixture("ls60_qobuz"))));
        assert!(can_pause(&first(fixture("muso_qobuz"))));
        assert!(
            can_pause(&json!({"state": "playing"})),
            "no controls listed"
        );
        assert!(can_pause(&json!({
            "trackRoles": {"mediaData": {"metaData": {"live": true}}},
            "controls": {"pause": true}
        })));
    }

    #[test]
    fn empty_stopped_player_has_nothing_to_resume() {
        // Captured from an LS60 woken from app-standby.
        let after_standby = json!({
            "trackRoles": {}, "mediaRoles": {}, "state": "stopped",
            "error": "Directory is empty. No playable items found."
        });
        assert!(nothing_to_resume(&after_standby));
        assert!(!nothing_to_resume(&first(fixture("ls60_qobuz"))));
        assert!(!nothing_to_resume(
            &json!({"state": "stopped", "trackRoles": {"title": "Song"}})
        ));
    }

    #[test]
    fn toggle_target_is_assumed_only_inside_the_settle_window() {
        let now = Instant::now();
        let pending = |state| Pending {
            state,
            can_pause: Some(true),
            sent: now,
        };
        assert!(assumed_state(None, now).is_none());
        assert_eq!(
            assumed_state(Some(pending("paused")), now).map(|p| p.state),
            Some("paused")
        );
        assert!(
            assumed_state(Some(pending("playing")), now + TOGGLE_SETTLE).is_none(),
            "expired"
        );
    }

    #[test]
    fn power_parsing_per_vendor() {
        assert_eq!(
            kef_power(&json!({"kefPhysicalSource": "standby"})),
            SpeakerPower::Standby
        );
        assert_eq!(
            kef_power(&json!({"kefPhysicalSource": "wifi"})),
            SpeakerPower::On
        );
        // Standby set from the KEF app reads as source `powerOn` (LS60, 2026-11-14).
        assert_eq!(
            kef_power(&json!({"kefPhysicalSource": "powerOn"})),
            SpeakerPower::Standby
        );
        assert_eq!(naim_power(&json!({"system": "on"})), SpeakerPower::On);
        assert_eq!(
            naim_power(&json!({"system": "lona"})),
            SpeakerPower::Standby
        );
        assert_eq!(naim_power(&json!({})), SpeakerPower::On);
    }

    #[test]
    fn get_data_unwraps_array_and_errors() {
        assert_eq!(
            unwrap_get_data("p", json!([{"i32_": 5}])).unwrap(),
            json!({"i32_": 5})
        );
        let err = unwrap_get_data(
            "p",
            json!({"error": {"message": "Node at path 'p' does not exist"}}),
        )
        .unwrap_err();
        assert!(err.to_string().contains("does not exist"));
        assert!(unwrap_get_data("p", json!([])).is_err());
    }

    /// Local stand-in for the speaker HTTP API: serves fixtures and records write requests.
    mod http {
        use super::*;
        use axum::{
            extract::{Query, State},
            routing::{get, post},
            Json, Router,
        };
        use std::collections::HashMap;
        use std::sync::{Arc, Mutex};

        #[derive(Clone, Default)]
        struct Fake {
            data: Arc<HashMap<String, Value>>,
            writes: Arc<Mutex<Vec<Value>>>,
            reads: Arc<Mutex<Vec<String>>>,
        }

        fn fake_with(entries: &[(&str, Value)]) -> Fake {
            Fake {
                data: Arc::new(
                    entries
                        .iter()
                        .map(|(k, v)| (k.to_string(), v.clone()))
                        .collect(),
                ),
                ..Default::default()
            }
        }

        fn getdata_router(fake: &Fake) -> (Fake, Router<Fake>) {
            (
                fake.clone(),
                Router::new()
                    .route("/api/getData", get(get_data))
                    .route("/api/setData", post(set_data)),
            )
        }

        async fn get_data(
            State(fake): State<Fake>,
            Query(q): Query<HashMap<String, String>>,
        ) -> Json<Value> {
            let path = q.get("path").cloned().unwrap_or_default();
            fake.reads.lock().unwrap().push(path.clone());
            Json(match fake.data.get(&path) {
                Some(v) => json!([v]),
                None => {
                    json!({"error": {"message": format!("Node at path '{path}' does not exist")}})
                }
            })
        }

        async fn set_data(State(fake): State<Fake>, Json(body): Json<Value>) -> Json<Value> {
            fake.writes.lock().unwrap().push(body);
            Json(json!({}))
        }

        async fn naim_power_get() -> Json<Value> {
            Json(json!({"system": "lona"}))
        }

        async fn naim_power_put(
            State(fake): State<Fake>,
            Query(q): Query<HashMap<String, String>>,
        ) -> Json<Value> {
            fake.writes
                .lock()
                .unwrap()
                .push(json!({"naim_power": q.get("system")}));
            Json(json!({}))
        }

        async fn serve(fake: Fake, router: Router<Fake>) -> String {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let app = router.with_state(fake);
            tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            addr.to_string()
        }

        /// A Naim whose port-80 and 15081 APIs are both the fake at `addr`.
        fn naim_on(addr: &str) -> Stream800 {
            let mut speaker = Stream800::new(addr, Vendor::Naim);
            speaker.naim_base = format!("http://{addr}");
            speaker
        }

        async fn naim_now_playing(
            State(fake): State<Fake>,
            Query(q): Query<HashMap<String, String>>,
        ) -> Json<Value> {
            if let Some(cmd) = q.get("cmd") {
                fake.writes.lock().unwrap().push(json!({"naim_cmd": cmd}));
                return Json(json!({}));
            }
            let can_resume = fake.data.get("naim:canResume").cloned();
            Json(json!({"canResume": can_resume.unwrap_or(json!("0"))}))
        }

        fn kef_fake() -> Fake {
            let mut data = HashMap::new();
            data.insert(PATH_PLAYER_DATA.to_string(), first(fixture("ls60_radio")));
            data.insert(PATH_VOLUME.to_string(), json!({"type":"i32_","i32_":31}));
            data.insert(PATH_MUTE.to_string(), json!({"type":"bool_","bool_":false}));
            data.insert(
                PATH_PLAY_TIME.to_string(),
                json!({"type":"i64_","i64_":104148}),
            );
            data.insert(
                PATH_KEF_SOURCE.to_string(),
                json!({"type":"kefPhysicalSource","kefPhysicalSource":"wifi"}),
            );
            Fake {
                data: Arc::new(data),
                ..Default::default()
            }
        }

        #[tokio::test]
        async fn kef_status_and_commands_over_http() {
            let fake = kef_fake();
            let router = Router::new()
                .route("/api/getData", get(get_data))
                .route("/api/setData", post(set_data));
            let host = serve(fake.clone(), router).await;
            let speaker = Stream800::new(&host, Vendor::Kef);

            let status = speaker.status().await.unwrap();
            assert_eq!(status.title, "NPO Radio 2");
            assert_eq!(status.volume, 31);
            assert_eq!(status.source, "cast");
            assert_eq!(status.position_s, 104);

            // Fixture is paused: pause is a no-op, play resumes via the `pause` toggle.
            speaker.pause().await.unwrap();
            speaker.play().await.unwrap();
            speaker.set_volume(42).await.unwrap();
            speaker.power(false).await.unwrap();
            let writes = fake.writes.lock().unwrap().clone();
            assert_eq!(
                writes,
                vec![
                    json!({"path": "player:player/control", "role": "activate", "value": {"control": "pause"}}),
                    json!({"path": "player:volume", "role": "value", "value": {"type": "i32_", "i32_": 42}}),
                    json!({"path": "settings:/kef/play/physicalSource", "role": "value",
                           "value": {"type": "kefPhysicalSource", "kefPhysicalSource": "standby"}}),
                ]
            );
        }

        #[tokio::test]
        async fn naim_standby_short_circuits_status_and_power_uses_15081_api() {
            let fake = Fake::default();
            let router = Router::new()
                .route("/power", get(naim_power_get).put(naim_power_put))
                .route("/api/getData", get(get_data));
            let addr = serve(fake.clone(), router).await;
            // getData is never reached: a speaker in standby returns early.
            let speaker = naim_on(&addr);

            let status = speaker.status().await.unwrap();
            assert_eq!(status.power, SpeakerPower::Standby);
            speaker.power(true).await.unwrap();
            speaker.power(false).await.unwrap();
            assert_eq!(
                fake.writes.lock().unwrap().clone(),
                vec![json!({"naim_power": "on"}), json!({"naim_power": "lona"})]
            );
        }

        #[tokio::test]
        async fn double_pause_sends_one_toggle_and_play_toggles_back() {
            // The speaker keeps reporting "playing" (it applies the toggle late).
            let (fake, router) = getdata_router(&fake_with(&[(
                PATH_PLAYER_DATA,
                json!({"state": "playing"}),
            )]));
            let host = serve(fake.clone(), router).await;
            let speaker = Stream800::new(&host, Vendor::Kef);

            speaker.pause().await.unwrap();
            speaker.pause().await.unwrap();
            assert_eq!(
                fake.writes.lock().unwrap().len(),
                1,
                "second pause inside the settle window must not toggle back"
            );
            speaker.play().await.unwrap();
            let controls: Vec<Value> = fake
                .writes
                .lock()
                .unwrap()
                .iter()
                .map(|w| w["value"]["control"].clone())
                .collect();
            assert_eq!(controls, vec![json!("pause"), json!("pause")]);
        }

        #[tokio::test]
        async fn live_radio_pause_sends_stop() {
            let (fake, router) = getdata_router(&fake_with(&[(
                PATH_PLAYER_DATA,
                first(fixture("muso_vtuner")),
            )]));
            let host = serve(fake.clone(), router).await;
            Stream800::new(&host, Vendor::Naim).pause().await.unwrap();
            let controls: Vec<Value> = fake
                .writes
                .lock()
                .unwrap()
                .iter()
                .map(|w| w["value"]["control"].clone())
                .collect();
            assert_eq!(controls, vec![json!("stop")]);
        }

        /// Captured from a Mu-so Qb 2 right after `stop` on vTuner radio.
        fn stopped_radio() -> Value {
            json!({"state": "stopped", "error": "", "keepActive": false, "streamerError": {}})
        }

        #[tokio::test]
        async fn naim_restarts_stopped_radio_via_now_playing() {
            for (can_resume, expected) in [("1", vec![json!({"naim_cmd": "play"})]), ("0", vec![])]
            {
                let (fake, router) = getdata_router(&fake_with(&[
                    (PATH_PLAYER_DATA, stopped_radio()),
                    ("naim:canResume", json!(can_resume)),
                ]));
                let addr = serve(
                    fake.clone(),
                    router.route("/nowplaying", get(naim_now_playing)),
                )
                .await;
                naim_on(&addr).play().await.unwrap();
                assert_eq!(
                    *fake.writes.lock().unwrap(),
                    expected,
                    "canResume {can_resume}"
                );
            }
        }

        /// `getRows`: the `rows:<path>` entry, paged by `from` / `to` (inclusive, like the
        /// speaker), or the speaker's error.
        async fn get_rows(
            State(fake): State<Fake>,
            Query(q): Query<HashMap<String, String>>,
        ) -> Json<Value> {
            let path = q.get("path").cloned().unwrap_or_default();
            let from: usize = q.get("from").and_then(|v| v.parse().ok()).unwrap_or(0);
            let to: usize = q.get("to").and_then(|v| v.parse().ok()).unwrap_or(0);
            Json(match fake.data.get(&format!("rows:{path}")) {
                Some(Value::Array(rows)) => json!({
                    "rows": rows.iter().skip(from).take(to + 1 - from).collect::<Vec<_>>(),
                    "rowsCount": rows.len(),
                }),
                _ => json!({"error": {"message": format!("Node at path '{path}' does not exist")}}),
            })
        }

        #[tokio::test]
        async fn qobuz_browse_play_and_skip_over_http() {
            let tracks: Vec<Value> = (1..=5)
                .map(|i| json!({"type": "audio", "path": format!("qobuz:/track?id={i}"), "title": format!("T{i}")}))
                .collect();
            let (fake, router) = getdata_router(&fake_with(&[(
                "rows:qobuz:/user/favourites/tracks",
                json!(tracks),
            )]));
            let addr = serve(
                fake.clone(),
                router
                    .route("/api/getRows", get(get_rows))
                    .route("/nowplaying", get(naim_now_playing)),
            )
            .await;
            let speaker = naim_on(&addr);

            let page = speaker
                .browse("qobuz:/user/favourites/tracks", 1, 2)
                .await
                .unwrap();
            assert_eq!(page.total, 5);
            let titles: Vec<&str> = page
                .rows
                .iter()
                .map(|r| r["title"].as_str().unwrap())
                .collect();
            assert_eq!(titles, ["T2", "T3"]);
            let missing = speaker.browse("qobuz:/nope", 0, 5).await.unwrap_err();
            assert!(missing.to_string().contains("does not exist"));

            let album = json!({"type": "container", "path": "qobuz:/album?id=A1", "containerPlayable": true});
            speaker.play_item(&album).await.unwrap();
            speaker.skip(true).await.unwrap();
            speaker.skip(false).await.unwrap();
            let writes = fake.writes.lock().unwrap().clone();
            assert_eq!(
                writes[0]["value"],
                json!({"control": "play", "mediaRoles": album})
            );
            assert_eq!(writes[1], json!({"naim_cmd": "next"}));
            assert_eq!(writes[2], json!({"naim_cmd": "prev"}));

            let kef = Stream800::new(&addr, Vendor::Kef);
            assert!(kef.skip(true).await.is_err(), "not checked on KEF yet");
        }

        fn controls_sent(fake: &Fake) -> Vec<Value> {
            fake.writes
                .lock()
                .unwrap()
                .iter()
                .map(|w| {
                    w.get("naim_cmd")
                        .map(|c| json!(format!("naim:{}", c.as_str().unwrap())))
                        .unwrap_or_else(|| w["value"]["control"].clone())
                })
                .collect()
        }

        /// A Naim that keeps reporting `player_data`, with `/nowplaying` able to resume.
        async fn naim_reporting(player_data: Value) -> (Fake, Stream800) {
            let (fake, router) = getdata_router(&fake_with(&[
                (PATH_PLAYER_DATA, player_data),
                ("naim:canResume", json!("1")),
            ]));
            let addr = serve(
                fake.clone(),
                router.route("/nowplaying", get(naim_now_playing)),
            )
            .await;
            (fake, naim_on(&addr))
        }

        /// A live stream with the replayable `mediaRoles.path` (fixtures have it stripped).
        fn with_replay_path(mut player_data: Value) -> Value {
            player_data["mediaRoles"]["path"] =
                json!("airable:https://example.airable.io/id/radio/1");
            player_data
        }

        #[tokio::test]
        async fn interrupted_live_radio_is_replayed_and_a_track_is_not() {
            let radio = with_replay_path(first(fixture("ls60_airable")));
            assert_eq!(radio["state"], json!("playing"), "fixture plays");
            let (fake, router) = getdata_router(&fake_with(&[(PATH_PLAYER_DATA, radio.clone())]));
            let host = serve(fake.clone(), router).await;
            let speaker = Stream800::new(&host, Vendor::Kef);

            assert!(speaker.interrupt().await.unwrap());
            assert!(speaker.resume_interrupted().await.unwrap());
            assert!(
                !speaker.resume_interrupted().await.unwrap(),
                "the kept stream is replayed once"
            );
            // A pause right after the replay stops the radio and keeps it, so play restarts it.
            speaker.pause().await.unwrap();
            speaker.play().await.unwrap();
            let writes = fake.writes.lock().unwrap().clone();
            assert_eq!(
                controls_sent(&fake),
                vec![json!("play"), json!("stop"), json!("play")]
            );
            assert_eq!(writes[0]["value"]["mediaRoles"], radio["mediaRoles"]);
            assert_eq!(writes[2]["value"]["mediaRoles"], radio["mediaRoles"]);

            let mut track = first(fixture("ls60_qobuz"));
            track["state"] = json!("playing");
            let (fake, router) = getdata_router(&fake_with(&[(PATH_PLAYER_DATA, track)]));
            let host = serve(fake.clone(), router).await;
            let speaker = Stream800::new(&host, Vendor::Kef);
            assert!(!speaker.interrupt().await.unwrap());
            assert!(!speaker.resume_interrupted().await.unwrap());
            assert!(fake.writes.lock().unwrap().is_empty());
        }

        #[tokio::test]
        async fn kef_live_radio_pause_stops_and_play_replays_the_station() {
            // LS60 (2026-11-14): `pause` on airable radio is HTTP 500 and `play` after `stop`
            // fails; `play` with the stopped stream's `mediaRoles` restarts it.
            let radio = with_replay_path(first(fixture("ls60_airable")));
            let (fake, router) = getdata_router(&fake_with(&[(PATH_PLAYER_DATA, radio.clone())]));
            let host = serve(fake.clone(), router).await;
            let speaker = Stream800::new(&host, Vendor::Kef);
            speaker.pause().await.unwrap();
            speaker.play().await.unwrap();
            let writes = fake.writes.lock().unwrap().clone();
            assert_eq!(controls_sent(&fake), vec![json!("stop"), json!("play")]);
            assert_eq!(writes[1]["value"]["mediaRoles"], radio["mediaRoles"]);
        }

        #[tokio::test]
        async fn naim_replays_the_station_when_now_playing_cannot_resume() {
            let radio = with_replay_path(first(fixture("muso_vtuner")));
            let (fake, router) = getdata_router(&fake_with(&[
                (PATH_PLAYER_DATA, radio.clone()),
                ("naim:canResume", json!("0")),
            ]));
            let addr = serve(
                fake.clone(),
                router.route("/nowplaying", get(naim_now_playing)),
            )
            .await;
            let speaker = naim_on(&addr);
            speaker.pause().await.unwrap();
            speaker.play().await.unwrap();
            assert_eq!(controls_sent(&fake), vec![json!("stop"), json!("play")]);
            assert_eq!(
                fake.writes.lock().unwrap()[1]["value"]["mediaRoles"],
                radio["mediaRoles"]
            );
        }

        fn kept(speaker: &Stream800) -> Option<LiveStream> {
            speaker.live_stream.lock().unwrap().clone()
        }

        fn keep(speaker: &Stream800, session: Option<&str>) {
            *speaker.live_stream.lock().unwrap() = Some(LiveStream {
                media: json!({"path": "airable:https://example.airable.io/id/radio/1"}),
                session: session.map(str::to_string),
            });
        }

        /// A KEF poll answering `player_data` (on, Wi-Fi input).
        async fn kef_reporting(player_data: Value) -> (Fake, Stream800) {
            let mut fake = kef_fake();
            Arc::get_mut(&mut fake.data)
                .unwrap()
                .insert(PATH_PLAYER_DATA.to_string(), player_data);
            let (fake, router) = getdata_router(&fake);
            let host = serve(fake.clone(), router).await;
            (fake, Stream800::new(&host, Vendor::Kef))
        }

        #[tokio::test]
        async fn station_stopped_by_pause_shows_as_paused_until_another_stream_plays() {
            let radio = with_replay_path(first(fixture("ls60_airable")));
            let (_, playing) = kef_reporting(radio).await;
            let before = playing.status().await.unwrap();
            assert!(before.station.is_some(), "fixture is a replayable station");
            let remembered = playing.last_station.lock().unwrap().clone();
            assert_eq!(remembered.as_ref(), Some(&before));

            let (_, stopped) = kef_reporting(stopped_radio()).await;
            *stopped.last_station.lock().unwrap() = remembered;
            assert_eq!(
                stopped.status().await.unwrap().mode,
                None,
                "KEF without a kept stream cannot restart it"
            );
            keep(&stopped, Some("5058108944"));
            let shown = stopped.status().await.unwrap();
            assert_eq!(shown.mode, Some(Mode::Pause));
            assert_eq!(shown.title, before.title);
            assert_eq!(shown.cover_url, before.cover_url);
            assert_eq!(shown.station, before.station);
            assert_eq!(shown.position_s, 0);

            // A track playing replaces the remembered station.
            let mut track = first(fixture("ls60_qobuz"));
            track["state"] = json!("playing");
            let (_, other) = kef_reporting(track).await;
            *other.last_station.lock().unwrap() = Some(before);
            other.status().await.unwrap();
            assert!(other.last_station.lock().unwrap().is_none());
        }

        #[test]
        fn only_an_empty_stopped_player_on_wifi_shows_the_station() {
            let last = SpeakerStatus {
                mode: Some(Mode::Play),
                source: "radio".into(),
                title: "Radio Paradise".into(),
                volume: 20,
                position_s: 300,
                live: true,
                ..Default::default()
            };
            let stopped = SpeakerStatus {
                volume: 35,
                muted: true,
                source: "wifi".into(),
                ..Default::default()
            };
            let shown = stopped_station_as_paused(&stopped, Some(&last), true).unwrap();
            assert_eq!(shown.mode, Some(Mode::Pause));
            assert_eq!((shown.volume, shown.muted), (35, true), "current volume");
            assert_eq!(shown.source, "radio");
            assert_eq!(shown.position_s, 0);
            assert!(shown.live);

            assert!(stopped_station_as_paused(&stopped, Some(&last), false).is_none());
            assert!(stopped_station_as_paused(&stopped, None, true).is_none());
            let optical = SpeakerStatus {
                source: "optical".into(),
                ..stopped.clone()
            };
            assert!(stopped_station_as_paused(&optical, Some(&last), true).is_none());
            let paused_track = SpeakerStatus {
                mode: Some(Mode::Pause),
                title: "Song".into(),
                ..stopped.clone()
            };
            assert!(stopped_station_as_paused(&paused_track, Some(&last), true).is_none());
        }

        #[tokio::test]
        async fn kept_stream_follows_play_sessions() {
            // ls60_airable plays under playId.timestamp 5058108944.
            let (_, speaker) = kef_reporting(first(fixture("ls60_airable"))).await;

            keep(&speaker, Some("5058108944"));
            speaker.status().await.unwrap();
            assert!(
                kept(&speaker).is_some(),
                "late `playing` read of the stopped session"
            );

            keep(&speaker, None);
            speaker.status().await.unwrap();
            assert_eq!(
                kept(&speaker).unwrap().session.as_deref(),
                Some("5058108944"),
                "a replayed stream adopts the new session"
            );

            keep(&speaker, Some("old"));
            speaker.status().await.unwrap();
            assert!(kept(&speaker).is_none(), "another session plays");
        }

        #[tokio::test]
        async fn standby_and_power_off_drop_the_kept_stream() {
            let (_, speaker) = kef_reporting(first(fixture("ls60_airable"))).await;
            keep(&speaker, Some("5058108944"));
            speaker.power(false).await.unwrap();
            assert!(kept(&speaker).is_none(), "power off");

            let fake = fake_with(&[(
                PATH_KEF_SOURCE,
                json!({"type": "kefPhysicalSource", "kefPhysicalSource": "standby"}),
            )]);
            let (_, router) = getdata_router(&fake);
            let host = serve(fake, router).await;
            let speaker = Stream800::new(&host, Vendor::Kef);
            keep(&speaker, Some("5058108944"));
            assert_eq!(speaker.status().await.unwrap().power, SpeakerPower::Standby);
            assert!(kept(&speaker).is_none(), "standby");
        }

        #[tokio::test]
        async fn pause_right_after_a_replay_stops_instead_of_toggling() {
            // The speaker still reports the cleared player after the replay.
            let (fake, router) = getdata_router(&fake_with(&[(PATH_PLAYER_DATA, stopped_radio())]));
            let host = serve(fake.clone(), router).await;
            let speaker = Stream800::new(&host, Vendor::Kef);
            keep(&speaker, Some("5058108944"));
            speaker.play().await.unwrap();
            speaker.pause().await.unwrap();
            assert_eq!(controls_sent(&fake), vec![json!("play"), json!("stop")]);
            assert!(kept(&speaker).is_some(), "the replayed stream stays kept");
        }

        #[tokio::test]
        async fn failed_replay_keeps_the_stream_for_a_retry() {
            async fn set_data_fails() -> axum::http::StatusCode {
                axum::http::StatusCode::INTERNAL_SERVER_ERROR
            }
            let fake = fake_with(&[(PATH_PLAYER_DATA, stopped_radio())]);
            let router = Router::new()
                .route("/api/getData", get(get_data))
                .route("/api/setData", post(set_data_fails));
            let host = serve(fake, router).await;
            let speaker = Stream800::new(&host, Vendor::Kef);
            keep(&speaker, Some("5058108944"));
            assert!(speaker.play().await.is_err());
            assert!(kept(&speaker).is_some());
        }

        #[tokio::test]
        async fn failed_naim_resume_falls_back_to_the_kept_stream() {
            async fn now_playing_resume_fails(
                Query(q): Query<HashMap<String, String>>,
            ) -> axum::response::Response {
                use axum::response::IntoResponse;
                if q.contains_key("cmd") {
                    return axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response();
                }
                Json(json!({"canResume": "1"})).into_response()
            }
            let (fake, router) = getdata_router(&fake_with(&[(PATH_PLAYER_DATA, stopped_radio())]));
            let addr = serve(
                fake.clone(),
                router.route("/nowplaying", get(now_playing_resume_fails)),
            )
            .await;
            let speaker = naim_on(&addr);
            keep(&speaker, Some("1"));
            speaker.play().await.unwrap();
            assert_eq!(controls_sent(&fake), vec![json!("play")]);
        }

        #[tokio::test]
        async fn kef_stops_live_radio_without_a_replayable_stream() {
            // No `mediaRoles.path`: pause still stops (the toggle is HTTP 500 on an LS60);
            // nothing is kept, so play cannot restart it (logged as a warning).
            let (fake, router) = getdata_router(&fake_with(&[(
                PATH_PLAYER_DATA,
                first(fixture("ls60_airable")),
            )]));
            let host = serve(fake.clone(), router).await;
            let speaker = Stream800::new(&host, Vendor::Kef);
            speaker.pause().await.unwrap();
            assert_eq!(controls_sent(&fake), vec![json!("stop")]);
            assert!(kept(&speaker).is_none());
        }

        #[tokio::test]
        async fn stop_reuses_the_player_data_it_read() {
            let (fake, router) = getdata_router(&fake_with(&[(
                PATH_PLAYER_DATA,
                first(fixture("ls60_airable")),
            )]));
            let host = serve(fake.clone(), router).await;
            Stream800::new(&host, Vendor::Kef).pause().await.unwrap();
            let reads = fake
                .reads
                .lock()
                .unwrap()
                .iter()
                .filter(|p| p.as_str() == PATH_PLAYER_DATA)
                .count();
            assert_eq!(reads, 1);
        }

        #[test]
        fn only_live_streams_with_a_path_are_replayable() {
            assert!(replay_media(&with_replay_path(first(fixture("ls60_airable")))).is_some());
            assert!(
                replay_media(&first(fixture("ls60_airable"))).is_none(),
                "no path"
            );
            assert!(
                replay_media(&with_replay_path(first(fixture("ls60_qobuz")))).is_none(),
                "not live"
            );
            for cast in ["ls60_radio", "muso_radio"] {
                let mut stream = with_replay_path(first(fixture(cast)));
                stream["mediaRoles"]["mediaData"]["metaData"]["live"] = json!(true);
                assert!(replay_media(&stream).is_none(), "{cast}: a Cast stream");
            }
        }

        #[tokio::test]
        async fn play_right_after_a_stop_resumes_even_on_a_stale_read() {
            // The speaker still reports the pre-stop `playing` for every read.
            let (fake, speaker) = naim_reporting(first(fixture("muso_vtuner"))).await;
            speaker.pause().await.unwrap();
            speaker.pause().await.unwrap();
            speaker.play().await.unwrap();
            assert_eq!(
                controls_sent(&fake),
                vec![json!("stop"), json!("naim:play")],
                "second pause is a no-op; play takes the resume path"
            );
        }

        #[tokio::test]
        async fn quick_pause_play_after_a_resume_stops_and_resumes_again() {
            let (fake, speaker) = naim_reporting(first(fixture("muso_vtuner"))).await;
            speaker.pause().await.unwrap();
            speaker.play().await.unwrap();
            speaker.pause().await.unwrap();
            speaker.play().await.unwrap();
            assert_eq!(
                controls_sent(&fake),
                vec![
                    json!("stop"),
                    json!("naim:play"),
                    json!("stop"),
                    json!("naim:play")
                ]
            );
        }

        #[tokio::test]
        async fn pause_after_resuming_a_track_pauses_instead_of_stopping() {
            // `/nowplaying` resumed a Qobuz session: pausable, so it must not be stopped.
            let (fake, speaker) = naim_reporting(first(fixture("muso_qobuz"))).await;
            speaker.remember("playing", None);
            speaker.pause().await.unwrap();
            assert_eq!(controls_sent(&fake), vec![json!("pause")]);
        }

        #[tokio::test]
        async fn naim_play_without_now_playing_api_is_ignored() {
            // No `/nowplaying` route: the lookup fails, play is skipped rather than an error.
            let (fake, router) = getdata_router(&fake_with(&[(PATH_PLAYER_DATA, stopped_radio())]));
            let addr = serve(fake.clone(), router).await;
            naim_on(&addr).play().await.unwrap();
            assert!(fake.writes.lock().unwrap().is_empty());
        }

        #[tokio::test]
        async fn kef_ignores_play_on_a_cleared_player() {
            let (fake, router) = getdata_router(&fake_with(&[(PATH_PLAYER_DATA, stopped_radio())]));
            let host = serve(fake.clone(), router).await;
            Stream800::new(&host, Vendor::Kef).play().await.unwrap();
            assert!(fake.writes.lock().unwrap().is_empty());
        }

        #[tokio::test]
        async fn volume_up_on_a_muted_speaker_unmutes_it() {
            let (fake, router) = getdata_router(&fake_with(&[(
                PATH_MUTE,
                json!({"type": "bool_", "bool_": true}),
            )]));
            let host = serve(fake.clone(), router).await;
            let speaker = Stream800::new(&host, Vendor::Kef);

            speaker.set_volume(0).await.unwrap();
            speaker.set_volume(30).await.unwrap();
            let paths: Vec<Value> = fake
                .writes
                .lock()
                .unwrap()
                .iter()
                .map(|w| w["path"].clone())
                .collect();
            assert_eq!(
                paths,
                vec![json!(PATH_VOLUME), json!(PATH_VOLUME), json!(PATH_MUTE)],
                "volume 0 leaves mute alone; a non-zero level unmutes"
            );
            assert_eq!(
                fake.writes.lock().unwrap()[2]["value"]["bool_"],
                json!(false)
            );
        }

        #[tokio::test]
        async fn naim_without_power_api_still_reports_on() {
            let (fake, router) = getdata_router(&fake_with(&[
                (PATH_PLAYER_DATA, first(fixture("muso_vtuner"))),
                (PATH_VOLUME, json!({"type": "i32_", "i32_": 23})),
            ]));
            let addr = serve(fake, router).await;
            // The fake has no `/power` route, like a Naim whose port 15081 does not answer.
            // The poll must still succeed.
            let speaker = naim_on(&addr);
            let status = speaker.status().await.unwrap();
            assert_eq!(status.power, SpeakerPower::On);
            assert_eq!(status.title, "Radio Paradise - Main Mix");
        }

        #[tokio::test]
        async fn kef_poll_reads_the_source_once() {
            let fake = kef_fake();
            let (fake, router) = getdata_router(&fake);
            let host = serve(fake.clone(), router).await;
            Stream800::new(&host, Vendor::Kef).status().await.unwrap();
            let source_reads = fake
                .reads
                .lock()
                .unwrap()
                .iter()
                .filter(|p| p.as_str() == PATH_KEF_SOURCE)
                .count();
            assert_eq!(source_reads, 1);
        }

        #[tokio::test]
        async fn kef_wakes_to_the_last_input_or_wifi() {
            let (fake, router) = getdata_router(&fake_with(&[
                (
                    PATH_KEF_SOURCE,
                    json!({"type": "kefPhysicalSource", "kefPhysicalSource": "tv"}),
                ),
                (
                    PATH_PLAYER_DATA,
                    json!({"state": "playing", "trackRoles": {"title": "TV"}}),
                ),
                (PATH_VOLUME, json!({"type": "i32_", "i32_": 20})),
            ]));
            let host = serve(fake.clone(), router).await;

            let fresh = Stream800::new(&host, Vendor::Kef);
            fresh.power(true).await.unwrap();
            let seen_tv = Stream800::new(&host, Vendor::Kef);
            seen_tv.status().await.unwrap();
            seen_tv.power(true).await.unwrap();

            let targets: Vec<Value> = fake
                .writes
                .lock()
                .unwrap()
                .iter()
                .map(|w| w["value"]["kefPhysicalSource"].clone())
                .collect();
            assert_eq!(targets, vec![json!("wifi"), json!("tv")]);
        }

        #[tokio::test]
        async fn play_after_standby_with_nothing_to_resume_sends_nothing() {
            let (fake, router) = getdata_router(&fake_with(&[(
                PATH_PLAYER_DATA,
                json!({"state": "stopped", "trackRoles": {},
                       "error": "Directory is empty. No playable items found."}),
            )]));
            let host = serve(fake.clone(), router).await;
            Stream800::new(&host, Vendor::Kef).play().await.unwrap();
            assert!(fake.writes.lock().unwrap().is_empty());
        }

        #[tokio::test]
        async fn missing_path_is_an_error() {
            let router = Router::new().route("/api/getData", get(get_data));
            let host = serve(Fake::default(), router).await;
            let speaker = Stream800::new(&host, Vendor::Kef);
            assert!(speaker.status().await.is_err());
        }
    }

    /// Read-only check against a real speaker:
    /// `STREAM800_LIVE=192.168.50.209:kef cargo test live_status -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "needs a real speaker on the LAN"]
    async fn live_status() {
        let spec = std::env::var("STREAM800_LIVE").expect("STREAM800_LIVE=host:kef|naim");
        let (host, vendor) = spec.split_once(':').expect("host:vendor");
        let vendor = if vendor == "naim" {
            Vendor::Naim
        } else {
            Vendor::Kef
        };
        let status = Stream800::new(host, vendor).status().await.unwrap();
        println!("{status:#?}");
    }
}
