//! Shared zone transport commands (`audio/{playerid}/play`, etc.) for 7091 and 7095.

use crate::{
    player_state::Playerid,
    remote::{IoCommand, Repeat},
    wire_response::build_wire_result_response,
    zone_audio_prefs::{get_loudness, set_loudness},
};
use anyhow::Context;
use matchit::Params;
use serde_json::json;
use tokio::sync::mpsc;
use tracing::{debug, warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeStep {
    Delta(i32),
    Absolute(i32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoneTransportCommand {
    Pause,
    Play,
    Resume,
    Position,
    QueueMinus,
    QueuePlus,
    QueuePlay,
    QueueRemove,
    QueueMove,
    Repeat,
    Shuffle,
    ShuffleOff,
    ShuffleOn,
    Loudness,
    LoudnessOff,
    LoudnessOn,
    Status,
    Volume,
    VolumeUp,
    VolumeDown,
}

pub fn zone_transport_router() -> matchit::Router<ZoneTransportCommand> {
    let mut router = matchit::Router::new();
    router
        .insert("audio/{playerid}/pause", ZoneTransportCommand::Pause)
        .unwrap();
    router
        .insert("audio/{playerid}/play", ZoneTransportCommand::Play)
        .unwrap();
    router
        .insert("audio/{playerid}/resume", ZoneTransportCommand::Resume)
        .unwrap();
    router
        .insert(
            "audio/{playerid}/position/{position}",
            ZoneTransportCommand::Position,
        )
        .unwrap();
    router
        .insert(
            "audio/{playerid}/queueminus",
            ZoneTransportCommand::QueueMinus,
        )
        .unwrap();
    router
        .insert(
            "audio/{playerid}/queueplus",
            ZoneTransportCommand::QueuePlus,
        )
        .unwrap();
    router
        .insert(
            "audio/{playerid}/queue/play/{unique_id}",
            ZoneTransportCommand::QueuePlay,
        )
        .unwrap();
    router
        .insert(
            "audio/{playerid}/queue/remove/{unique_id}",
            ZoneTransportCommand::QueueRemove,
        )
        .unwrap();
    router
        .insert(
            "audio/{playerid}/queue/move/{src_uid}/before/{before_uid}",
            ZoneTransportCommand::QueueMove,
        )
        .unwrap();
    router
        .insert(
            "audio/{playerid}/repeat/{repeat}",
            ZoneTransportCommand::Repeat,
        )
        .unwrap();
    router
        .insert(
            "audio/{playerid}/shuffle/0",
            ZoneTransportCommand::ShuffleOff,
        )
        .unwrap();
    router
        .insert(
            "audio/{playerid}/shuffle/1",
            ZoneTransportCommand::ShuffleOn,
        )
        .unwrap();
    router
        .insert("audio/{playerid}/shuffle", ZoneTransportCommand::Shuffle)
        .unwrap();
    router
        .insert(
            "audio/{playerid}/loudness/0",
            ZoneTransportCommand::LoudnessOff,
        )
        .unwrap();
    router
        .insert(
            "audio/{playerid}/loudness/1",
            ZoneTransportCommand::LoudnessOn,
        )
        .unwrap();
    router
        .insert("audio/{playerid}/loudness", ZoneTransportCommand::Loudness)
        .unwrap();
    router
        .insert("audio/{playerid}/status", ZoneTransportCommand::Status)
        .unwrap();
    router
        .insert(
            "audio/{playerid}/volume/{step}",
            ZoneTransportCommand::Volume,
        )
        .unwrap();
    router
        .insert("audio/{playerid}/volume/+", ZoneTransportCommand::Volume)
        .unwrap();
    router
        .insert("audio/{playerid}/volume/-", ZoneTransportCommand::Volume)
        .unwrap();
    router
        .insert("audio/{playerid}/volup", ZoneTransportCommand::VolumeUp)
        .unwrap();
    router
        .insert("audio/{playerid}/voldown", ZoneTransportCommand::VolumeDown)
        .unwrap();
    router
}

/// Returns `true` if `command` is a zone transport route.
pub fn is_zone_transport(command: &str) -> bool {
    zone_transport_router().at(command).is_ok()
}

pub fn infer_result_name(command: &str) -> String {
    command
        .split('/')
        .filter(|s| !s.is_empty())
        .rev()
        .find(|s| s.chars().next().is_some_and(|c| c.is_ascii_alphabetic()))
        .unwrap_or("response")
        .to_string()
}

#[cfg(test)]
pub fn parse_volume_step(step: &str) -> anyhow::Result<i32> {
    match parse_volume_command("", step)? {
        VolumeStep::Delta(d) => Ok(d),
        VolumeStep::Absolute(v) => Ok(v),
    }
}

/// Relative steps (`+`, `-`, `+1`, `-3`) vs absolute level (`50`) for Loxone volume paths.
pub fn parse_volume_command(command: &str, step: &str) -> anyhow::Result<VolumeStep> {
    if command.ends_with("/volume/+") {
        return Ok(VolumeStep::Delta(1));
    }
    if command.ends_with("/volume/-") {
        return Ok(VolumeStep::Delta(-1));
    }
    match step {
        "+" | "+1" => Ok(VolumeStep::Delta(1)),
        "-" | "-1" => Ok(VolumeStep::Delta(-1)),
        s if s.starts_with('+') => Ok(VolumeStep::Delta(
            s[1..].parse::<i32>().context("invalid volume step")?,
        )),
        s if s.starts_with('-') => Ok(VolumeStep::Delta(
            s.parse::<i32>().context("invalid volume step")?,
        )),
        s => Ok(VolumeStep::Absolute(
            s.parse::<i32>().context("invalid volume level")?,
        )),
    }
}

/// Handle zone transport; returns JSON response body (not a WebSocket message).
pub async fn dispatch(command: &str, sender: &mpsc::Sender<IoCommand>) -> anyhow::Result<String> {
    let router = zone_transport_router();
    let matched = router.at(command).map_err(|_| {
        warn!(command, "zone transport route not found");
        anyhow::anyhow!("zone transport route not found: {command}")
    })?;
    let result_name = infer_result_name(command);

    match matched.value {
        ZoneTransportCommand::Pause => {
            let playerid = parse_playerid(&matched.params)?;
            sender.send(IoCommand::Pause(playerid)).await?;
            empty_result(command, &result_name)
        }
        ZoneTransportCommand::Play | ZoneTransportCommand::Resume => {
            let playerid = parse_playerid(&matched.params)?;
            sender.send(IoCommand::Play(playerid, None)).await?;
            empty_result(command, &result_name)
        }
        ZoneTransportCommand::Position => {
            let playerid = parse_playerid(&matched.params)?;
            let position: i32 = matched
                .params
                .get("position")
                .context("Missing parameter position")?
                .parse()?;
            sender.send(IoCommand::Position(playerid, position)).await?;
            empty_result(command, &result_name)
        }
        ZoneTransportCommand::QueueMinus => {
            let playerid = parse_playerid(&matched.params)?;
            sender.send(IoCommand::QueueMinus(playerid)).await?;
            empty_result(command, &result_name)
        }
        ZoneTransportCommand::QueuePlus => {
            let playerid = parse_playerid(&matched.params)?;
            sender.send(IoCommand::QueuePlus(playerid)).await?;
            empty_result(command, &result_name)
        }
        ZoneTransportCommand::QueuePlay => {
            let playerid = parse_playerid(&matched.params)?;
            let unique_id = matched
                .params
                .get("unique_id")
                .context("Missing parameter unique_id")?
                .to_string();
            debug!(
                player_id = playerid.0,
                "queue play ignored: no audioserver queue"
            );
            build_wire_result_response("queue", json!({ "play": unique_id }), Some(command), &[])
        }
        ZoneTransportCommand::QueueRemove => {
            let playerid = parse_playerid(&matched.params)?;
            let unique_id = matched
                .params
                .get("unique_id")
                .context("Missing parameter unique_id")?
                .to_string();
            debug!(
                player_id = playerid.0,
                "queue remove ignored: no audioserver queue"
            );
            build_wire_result_response(
                "remove",
                json!({ "deleted": unique_id }),
                Some(command),
                &[],
            )
        }
        ZoneTransportCommand::QueueMove => {
            let playerid = parse_playerid(&matched.params)?;
            let src_uid = matched
                .params
                .get("src_uid")
                .context("Missing parameter src_uid")?
                .to_string();
            let before_uid = matched
                .params
                .get("before_uid")
                .context("Missing parameter before_uid")?
                .to_string();
            debug!(
                player_id = playerid.0,
                src_uid, before_uid, "queue move ignored: no audioserver queue"
            );
            build_wire_result_response("move", json!("ok"), Some(command), &[])
        }
        ZoneTransportCommand::Repeat => {
            let playerid = parse_playerid(&matched.params)?;
            let repeat: Repeat = matched
                .params
                .get("repeat")
                .context("Missing parameter repeat")?
                .parse()?;
            sender.send(IoCommand::Repeat(playerid, repeat)).await?;
            empty_result(command, &result_name)
        }
        ZoneTransportCommand::Shuffle => {
            let playerid = parse_playerid(&matched.params)?;
            sender.send(IoCommand::Shuffle(playerid, None)).await?;
            empty_result(command, &result_name)
        }
        ZoneTransportCommand::ShuffleOff => {
            let playerid = parse_playerid(&matched.params)?;
            sender
                .send(IoCommand::Shuffle(playerid, Some(false)))
                .await?;
            empty_result(command, &result_name)
        }
        ZoneTransportCommand::ShuffleOn => {
            let playerid = parse_playerid(&matched.params)?;
            sender
                .send(IoCommand::Shuffle(playerid, Some(true)))
                .await?;
            empty_result(command, &result_name)
        }
        ZoneTransportCommand::Loudness => {
            let playerid = parse_playerid(&matched.params)?;
            loudness_result(command, get_loudness(playerid))
        }
        ZoneTransportCommand::LoudnessOff => {
            let playerid = parse_playerid(&matched.params)?;
            set_loudness(playerid, false);
            loudness_result(command, false)
        }
        ZoneTransportCommand::LoudnessOn => {
            let playerid = parse_playerid(&matched.params)?;
            set_loudness(playerid, true);
            loudness_result(command, true)
        }
        ZoneTransportCommand::Status => {
            let playerid = parse_playerid(&matched.params)?;
            let (tx, rx) = tokio::sync::oneshot::channel();
            sender.send(IoCommand::Status(playerid, tx)).await?;
            let state = rx.await?;
            build_wire_result_response(
                &format!("{result_name}_result"),
                json!([state]),
                Some(command),
                &[],
            )
        }
        ZoneTransportCommand::Volume => {
            let playerid = parse_playerid(&matched.params)?;
            let step = if command.ends_with("/volume/+") || command.ends_with("/volume/-") {
                parse_volume_command(command, "")?
            } else {
                let step_str = matched
                    .params
                    .get("step")
                    .context("Missing parameter step")?;
                parse_volume_command(command, step_str)?
            };
            sender.send(IoCommand::Volume(playerid, step)).await?;
            empty_result(command, &result_name)
        }
        ZoneTransportCommand::VolumeUp => {
            let playerid = parse_playerid(&matched.params)?;
            sender
                .send(IoCommand::Volume(playerid, VolumeStep::Delta(1)))
                .await?;
            empty_result(command, &result_name)
        }
        ZoneTransportCommand::VolumeDown => {
            let playerid = parse_playerid(&matched.params)?;
            sender
                .send(IoCommand::Volume(playerid, VolumeStep::Delta(-1)))
                .await?;
            empty_result(command, &result_name)
        }
    }
}

fn parse_playerid(params: &Params<'_, '_>) -> anyhow::Result<Playerid> {
    params
        .get("playerid")
        .context("Missing parameter playerid")?
        .parse()
}

fn loudness_result(command: &str, enabled: bool) -> anyhow::Result<String> {
    build_wire_result_response("loudness_result", json!(enabled), Some(command), &[])
}

fn empty_result(command: &str, result_name: &str) -> anyhow::Result<String> {
    build_wire_result_response(
        &format!("{result_name}_result"),
        json!([]),
        Some(command),
        &[],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_position_route() {
        assert!(is_zone_transport("audio/1/position/120"));
    }

    #[test]
    fn matches_volume_routes() {
        assert!(is_zone_transport("audio/1/volume/50"));
        assert!(is_zone_transport("audio/1/volume/+"));
        assert!(is_zone_transport("audio/1/volume/-"));
        assert!(is_zone_transport("audio/1/volup"));
        assert!(is_zone_transport("audio/1/voldown"));
    }

    #[tokio::test]
    async fn dispatch_volume_absolute_sends_io_command() {
        let (tx, mut rx) = mpsc::channel(4);
        let body = dispatch("audio/1/volume/42", &tx).await.unwrap();
        assert!(body.contains("volume_result"));
        match rx.recv().await {
            Some(IoCommand::Volume(Playerid(1), VolumeStep::Absolute(42))) => {}
            other => panic!("expected Volume absolute 42, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_volup_sends_io_command() {
        let (tx, mut rx) = mpsc::channel(4);
        dispatch("audio/2/volup", &tx).await.unwrap();
        match rx.recv().await {
            Some(IoCommand::Volume(Playerid(2), VolumeStep::Delta(1))) => {}
            other => panic!("expected Volume delta +1, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_status_returns_state() {
        use crate::player_state::{Mode, PlayerState};

        let (tx, mut rx) = mpsc::channel(4);
        let status_task = tokio::spawn(async move {
            if let Some(IoCommand::Status(Playerid(1), reply_tx)) = rx.recv().await {
                let state = PlayerState {
                    playerid: Playerid(1),
                    mode: Mode::Play,
                    title: "Status Song".into(),
                    ..Default::default()
                };
                let _ = reply_tx.send(state);
            }
        });

        let body = dispatch("audio/1/status", &tx).await.unwrap();
        status_task.await.expect("status task");
        assert!(body.contains("status_result"));
        assert!(body.contains("Status Song"));
    }

    #[test]
    fn matches_play_route() {
        assert!(is_zone_transport("audio/1/play"));
        assert!(is_zone_transport("audio/1/resume"));
        assert!(!is_zone_transport("audio/cfg/getconfig"));
    }

    #[test]
    fn matches_shuffle_mode_routes() {
        assert!(is_zone_transport("audio/1/shuffle"));
        assert!(is_zone_transport("audio/1/shuffle/0"));
        assert!(is_zone_transport("audio/1/shuffle/1"));
    }

    #[test]
    fn matches_loudness_routes() {
        assert!(is_zone_transport("audio/2/loudness"));
        assert!(is_zone_transport("audio/2/loudness/1"));
        assert!(!is_zone_transport("audio/2/loudness/on"));
    }

    #[test]
    fn loudness_result_is_boolean() {
        let wire = loudness_result("audio/2/loudness", false).unwrap();
        assert!(wire.starts_with("{\"loudness_result\":false"));
        assert!(wire.contains("\"command\":\"audio/2/loudness\""));
    }

    #[tokio::test]
    async fn dispatch_loudness_get_and_set() {
        let (tx, _rx) = mpsc::channel(4);
        let get = dispatch("audio/2/loudness", &tx).await.unwrap();
        assert!(get.contains("\"loudness_result\":false"));
        let set = dispatch("audio/2/loudness/1", &tx).await.unwrap();
        assert!(set.contains("\"loudness_result\":true"));
        let get2 = dispatch("audio/2/loudness", &tx).await.unwrap();
        assert!(get2.contains("\"loudness_result\":true"));
        let off = dispatch("audio/2/loudness/0", &tx).await.unwrap();
        assert!(off.contains("\"loudness_result\":false"));
    }

    #[test]
    fn parse_volume_command_relative_and_absolute() {
        use super::{parse_volume_command, parse_volume_step, VolumeStep};
        assert_eq!(parse_volume_step("+").unwrap(), 1);
        assert_eq!(parse_volume_step("-3").unwrap(), -3);
        assert_eq!(
            parse_volume_command("audio/1/volume/-1", "-1").unwrap(),
            VolumeStep::Delta(-1)
        );
        assert_eq!(
            parse_volume_command("audio/1/volume/+1", "+1").unwrap(),
            VolumeStep::Delta(1)
        );
        assert_eq!(
            parse_volume_command("audio/1/volume/50", "50").unwrap(),
            VolumeStep::Absolute(50)
        );
        assert_eq!(
            parse_volume_command("audio/1/volume/+", "").unwrap(),
            VolumeStep::Delta(1)
        );
    }

    #[test]
    fn infer_result_name_play() {
        assert_eq!(infer_result_name("audio/1/play"), "play");
        assert_eq!(infer_result_name("audio/1/resume"), "resume");
    }

    #[test]
    fn pause_result_key_precedes_command() {
        let wire = empty_result("audio/1/pause", "pause").unwrap();
        assert!(wire.starts_with("{\"pause_result\":"));
        assert!(wire.contains("\"command\":\"audio/1/pause\""));
    }

    #[tokio::test]
    async fn dispatch_play_sends_io_command() {
        let (tx, mut rx) = mpsc::channel(4);
        let body = dispatch("audio/1/play", &tx).await.unwrap();
        assert!(body.contains("play_result"));
        match rx.recv().await {
            Some(IoCommand::Play(Playerid(1), None)) => {}
            other => panic!("expected Play(1), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_position_sends_io_command() {
        let (tx, mut rx) = mpsc::channel(4);
        let body = dispatch("audio/2/position/256", &tx).await.unwrap();
        assert!(body.contains("position_result"));
        match rx.recv().await {
            Some(IoCommand::Position(Playerid(2), 256)) => {}
            other => panic!("expected Position(2, 256), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_shuffle_mode_sends_io_command() {
        let (tx, mut rx) = mpsc::channel(4);
        dispatch("audio/2/shuffle/1", &tx).await.unwrap();
        match rx.recv().await {
            Some(IoCommand::Shuffle(Playerid(2), Some(true))) => {}
            other => panic!("expected Shuffle(2, Some(true)), got {other:?}"),
        }
        dispatch("audio/2/shuffle/0", &tx).await.unwrap();
        match rx.recv().await {
            Some(IoCommand::Shuffle(Playerid(2), Some(false))) => {}
            other => panic!("expected Shuffle(2, Some(false)), got {other:?}"),
        }
        dispatch("audio/2/shuffle", &tx).await.unwrap();
        match rx.recv().await {
            Some(IoCommand::Shuffle(Playerid(2), None)) => {}
            other => panic!("expected Shuffle(2, None), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_pause_sends_io_command() {
        let (tx, mut rx) = mpsc::channel(4);
        let body = dispatch("audio/1/pause", &tx).await.unwrap();
        assert!(body.contains("pause_result"));
        match rx.recv().await {
            Some(IoCommand::Pause(Playerid(1))) => {}
            other => panic!("expected Pause(1), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_repeat_sends_io_command() {
        use crate::remote::Repeat;

        let (tx, mut rx) = mpsc::channel(4);
        dispatch("audio/1/repeat/1", &tx).await.unwrap();
        match rx.recv().await {
            Some(IoCommand::Repeat(Playerid(1), Repeat::All)) => {}
            other => panic!("expected Repeat all, got {other:?}"),
        }
    }
}
