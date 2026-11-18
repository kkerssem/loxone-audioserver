//! Qobuz through a speaker logged into Qobuz: browse folders and play items on a zone (see
//! [`crate::qobuz`]).

use std::sync::Arc;

use tokio::sync::broadcast;
use tracing::warn;

use crate::{
    cast::CastPlaybackManager,
    loxone_config::ZoneDescriptor,
    player_state::Playerid,
    qobuz::{self, Folder},
    remote::{BrowseResult, IoEvent},
    speaker::{
        SpeakerBackend, SpeakerCommand, SpeakerKind, SpeakerManager, SpeakerPower, SpeakerStatus,
    },
    zone_enrich::proxied_cover_url,
    zone_registry::ZoneRegistry,
};

use super::cast::release_cast_zone;

/// The speaker Qobuz is browsed through: the bound speaker with the lowest zone id that can.
pub(crate) fn browse_backend(manager: &SpeakerManager) -> Option<Arc<dyn SpeakerBackend>> {
    manager.first_backend(SpeakerKind::plays_qobuz)
}

fn empty(folder_id: &str, name: &str, start: usize) -> BrowseResult {
    BrowseResult {
        id: folder_id.to_string(),
        name: name.to_string(),
        start,
        service: Some(qobuz::SERVICE_CMD.to_string()),
        total_items: 0,
        items: vec![],
    }
}

/// One page of a Loxone folder id, read from the speaker. Failures answer an empty folder.
pub(crate) async fn browse_folder(
    backend: Option<Arc<dyn SpeakerBackend>>,
    folder_id: &str,
    start: usize,
    max_items: usize,
    cover_proxy_base: &str,
) -> BrowseResult {
    let (path, name) = match qobuz::folder(folder_id) {
        Folder::Speaker { path, name } => (path, name),
        Folder::Root | Folder::Empty => return empty(folder_id, "", start),
    };
    // The start page prefetches "Your playlists" with a small page for a carousel, and asks
    // again with a larger page for the library view (as the earlier Qobuz bridge answered).
    if folder_id == "3" && start == 0 && max_items <= 20 {
        return empty(folder_id, name, start);
    }
    let Some(backend) = backend else {
        warn!(folder = %folder_id, "qobuz browse: no speaker logged into qobuz is bound");
        return empty(folder_id, name, start);
    };
    let page = match backend.browse(&path, start, max_items).await {
        Ok(page) => page,
        Err(e) => {
            warn!(folder = %folder_id, error = %e, "qobuz browse failed");
            return empty(folder_id, name, start);
        }
    };
    qobuz::remember_tracks(&page.rows);
    let cover = |url: &str| proxied_cover_url(cover_proxy_base, url, "x");
    let items = page
        .rows
        .iter()
        .enumerate()
        .map(|(i, row)| qobuz::browse_item(row, start + i + 1, &cover))
        .collect();
    BrowseResult {
        total_items: page.total,
        items,
        ..empty(folder_id, name, start)
    }
}

/// Commands that play a Qobuz item on the zone's speaker, or why it cannot (pure; see
/// [`play_item`]).
pub(crate) fn plan_play_item(
    bound_kind: Option<SpeakerKind>,
    last: Option<&SpeakerStatus>,
    id: &str,
) -> Result<Vec<SpeakerCommand>, &'static str> {
    let Some(kind) = bound_kind else {
        return Err("no speaker bound to zone");
    };
    if !kind.plays_qobuz() {
        return Err("speaker cannot play qobuz");
    }
    let track = qobuz::known_track(id);
    let Some(media) = qobuz::media_for(id, track.as_ref()) else {
        return Err("not a qobuz item");
    };
    let mut commands = Vec::new();
    if last.is_some_and(|s| s.power != SpeakerPower::On) {
        commands.push(SpeakerCommand::Power(true));
    }
    commands.push(SpeakerCommand::PlayItem(Arc::new(media)));
    Ok(commands)
}

/// Play a Qobuz item on the zone's speaker. Cast radio playing on the zone is stopped first,
/// so the speaker's own playback shows again.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn play_item(
    playerid: Playerid,
    id: &str,
    manager: &SpeakerManager,
    cast_manager: &mut CastPlaybackManager,
    registry: &mut ZoneRegistry,
    zones: &[ZoneDescriptor],
    audioserver_serial: &str,
    cover_proxy_base: &str,
    events: &broadcast::Sender<IoEvent>,
) {
    let last = manager.status_for_commands(playerid.0);
    let commands = match plan_play_item(manager.kind(playerid.0), last.as_ref(), id) {
        Ok(commands) => commands,
        Err(reason) => {
            warn!(player_id = playerid.0, item = %id, reason, "qobuz item not played");
            return;
        }
    };
    if cast_manager.zone_is_radio(playerid.0) {
        cast_manager.stop_zone(playerid).await.ok();
        release_cast_zone(
            playerid,
            registry,
            zones,
            audioserver_serial,
            cover_proxy_base,
            events,
        );
    }
    for command in commands {
        if let Err(e) = manager.command(playerid.0, command) {
            warn!(player_id = playerid.0, error = %e, "qobuz item not queued");
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player_state::Mode;
    use crate::speaker::{fake::FakeSpeaker, ServiceRows};
    use serde_json::json;

    fn fake_with_albums() -> Arc<FakeSpeaker> {
        let fake = FakeSpeaker::with_status(SpeakerStatus::default());
        let rows: Vec<_> = (1..=3)
            .map(|i| {
                json!({"type": "container", "path": format!("qobuz:/album?id=A{i}"),
                    "title": format!("Album {i}"), "containerType": "album",
                    "icon": "https://static.qobuz.com/a.jpg"})
            })
            .collect();
        fake.folders.lock().unwrap().insert(
            "qobuz:/user/favourites/albums".into(),
            ServiceRows { total: 3, rows },
        );
        fake
    }

    #[tokio::test]
    async fn favourite_albums_page_through_the_speaker() {
        let backend: Arc<dyn SpeakerBackend> = fake_with_albums();
        let result = browse_folder(Some(backend), "5", 1, 50, "http://host:7091").await;
        assert_eq!(result.id, "5");
        assert_eq!(result.name, "Albums");
        assert_eq!(result.service.as_deref(), Some("spotify"));
        assert_eq!(result.total_items, 3);
        assert_eq!(result.items.len(), 2);
        assert_eq!(result.items[0].id, "qobuz:album:A2");
        assert_eq!(result.items[0].slot, 2);
        assert!(result.items[0]
            .coverurl
            .as_deref()
            .unwrap()
            .starts_with("http://host:7091/cors-proxy?url="));
    }

    #[tokio::test]
    async fn root_unknown_and_failing_folders_are_empty() {
        let backend: Arc<dyn SpeakerBackend> = fake_with_albums();
        for id in ["start", "2", "qobuz:album:missing"] {
            let result = browse_folder(Some(Arc::clone(&backend)), id, 0, 50, "b").await;
            assert!(result.items.is_empty(), "{id}");
            assert_eq!(result.id, id);
        }
        assert!(browse_folder(None, "5", 0, 50, "b").await.items.is_empty());
        // The start page's small "Your playlists" prefetch.
        assert!(browse_folder(Some(backend), "3", 0, 20, "b")
            .await
            .items
            .is_empty());
    }

    #[test]
    fn items_play_only_on_a_speaker_logged_into_qobuz() {
        let on = SpeakerStatus {
            mode: Some(Mode::Pause),
            ..Default::default()
        };
        let plan = plan_play_item(
            Some(SpeakerKind::NaimStream800),
            Some(&on),
            "qobuz:album:A1",
        )
        .unwrap();
        assert!(matches!(plan.as_slice(), [SpeakerCommand::PlayItem(m)]
            if m["path"] == "qobuz:/album?id=A1"));
        let standby = SpeakerStatus {
            power: SpeakerPower::Standby,
            ..Default::default()
        };
        let plan = plan_play_item(
            Some(SpeakerKind::NaimStream800),
            Some(&standby),
            "qobuz:track:T1",
        )
        .unwrap();
        assert_eq!(plan[0], SpeakerCommand::Power(true));
        assert_eq!(
            plan_play_item(Some(SpeakerKind::KefStream800), None, "qobuz:album:A1"),
            Err("speaker cannot play qobuz")
        );
        assert_eq!(
            plan_play_item(None, None, "qobuz:album:A1"),
            Err("no speaker bound to zone")
        );
        assert_eq!(
            plan_play_item(Some(SpeakerKind::NaimStream800), None, "tunein:s1"),
            Err("not a qobuz item")
        );
    }

    #[tokio::test]
    async fn an_album_plays_on_the_zones_naim_speaker() {
        use crate::speaker::binding::ZoneSpeakerTarget;
        use crate::speaker::fake;
        use std::time::Duration;
        use tokio::sync::mpsc;

        let fake_speaker = FakeSpeaker::with_status(SpeakerStatus {
            power: SpeakerPower::Standby,
            ..Default::default()
        });
        let (tx, mut rx) = mpsc::channel(16);
        let mut manager = SpeakerManager::with_connector(
            tx,
            fake::connector(&fake_speaker),
            crate::speaker::manager::PollTiming {
                playing: Duration::from_millis(20),
                idle: Duration::from_millis(20),
                after_command: Duration::from_millis(20),
                after_command_window: Duration::from_millis(100),
                request_timeout: Duration::from_millis(200),
                failures_until_unreachable: 3,
                wake_timeout: Duration::from_secs(2),
            },
        );
        manager.sync(&[ZoneSpeakerTarget {
            player_id: 2,
            speaker_id: "naim".into(),
            kind: SpeakerKind::NaimStream800,
            host: "10.0.0.7".into(),
        }]);
        tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(browse_backend(&manager).is_some());

        let (cast_tx, _cast_rx) = mpsc::channel(4);
        let mut cast_manager = CastPlaybackManager::new(cast_tx);
        let mut registry = ZoneRegistry::new();
        let (events, _) = broadcast::channel(8);
        play_item(
            Playerid(2),
            "spotify:album:A1",
            &manager,
            &mut cast_manager,
            &mut registry,
            &[],
            "504F94000002",
            "http://host:7091",
            &events,
        )
        .await;
        // A zone without a speaker: nothing queued, nothing panics.
        play_item(
            Playerid(9),
            "qobuz:album:A1",
            &manager,
            &mut cast_manager,
            &mut registry,
            &[],
            "504F94000002",
            "http://host:7091",
            &events,
        )
        .await;
        tokio::time::timeout(Duration::from_secs(2), async {
            while !fake_speaker
                .commands()
                .iter()
                .any(|c| matches!(c, SpeakerCommand::PlayItem(_)))
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let commands = fake_speaker.commands();
        assert_eq!(commands[0], SpeakerCommand::Power(true));
        assert!(
            matches!(&commands[1], SpeakerCommand::PlayItem(m) if m["path"] == "qobuz:/album?id=A1")
        );
    }

    /// Read-only: browse a real speaker's Qobuz folders as the Loxone App would see them.
    /// `QOBUZ_LIVE=<ip> cargo test live_qobuz_browse -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "needs a real Naim speaker logged into Qobuz"]
    async fn live_qobuz_browse() {
        let host = std::env::var("QOBUZ_LIVE").expect("QOBUZ_LIVE=<ip>");
        let backend: Arc<dyn SpeakerBackend> = Arc::new(crate::speaker::stream800::Stream800::new(
            &host,
            crate::speaker::stream800::Vendor::Naim,
        ));
        let mut drill = None;
        for folder in ["0", "1", "3", "4", "5", "6"] {
            let max = if folder == "3" { 50 } else { 3 };
            let result =
                browse_folder(Some(Arc::clone(&backend)), folder, 0, max, "http://proxy").await;
            println!(
                "folder {folder} {:?}: {} items",
                result.name, result.total_items
            );
            for item in result.items.iter().take(3) {
                println!("  {}", serde_json::to_string(item).unwrap());
            }
            if folder == "5" {
                drill = result.items.first().map(|i| i.id.clone());
            }
        }
        if let Some(album) = drill {
            let result = browse_folder(Some(backend), &album, 0, 3, "http://proxy").await;
            println!("album {album}: {} tracks", result.total_items);
            for item in &result.items {
                println!("  {}", serde_json::to_string(item).unwrap());
            }
        }
    }
}
