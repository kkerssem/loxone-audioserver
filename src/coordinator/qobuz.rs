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
    remote::{
        BrowseResult, GlobalSearchCategory, GlobalSearchResultBody, GlobalSearchTopResults, IoEvent,
    },
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
    qobuz::remember_rows(&page.rows);
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

/// One page of a search category from the speaker, as browse items (empty on failure).
async fn search_page(
    backend: &dyn SpeakerBackend,
    kind: &str,
    query: &str,
    start: usize,
    count: usize,
    cover_proxy_base: &str,
) -> (usize, Vec<crate::remote::BrowseResultItem>) {
    let Some(path) = qobuz::search_path(kind, query) else {
        return (0, vec![]);
    };
    match backend.browse(&path, start, count).await {
        Ok(page) => {
            qobuz::remember_rows(&page.rows);
            let cover = |url: &str| proxied_cover_url(cover_proxy_base, url, "x");
            let items = page
                .rows
                .iter()
                .enumerate()
                .map(|(i, row)| qobuz::browse_item(row, start + i + 1, &cover))
                .collect();
            (page.total, items)
        }
        Err(e) => {
            warn!(kind, error = %e, "qobuz search failed");
            (0, vec![])
        }
    }
}

/// The App's global search: a few hits per category, each with a link to its full list.
pub(crate) async fn global_search(
    backend: Option<Arc<dyn SpeakerBackend>>,
    provider_spec: &str,
    query: &str,
    cover_proxy_base: &str,
) -> GlobalSearchResultBody {
    let mut body = GlobalSearchResultBody::empty();
    let Some(backend) = backend else {
        warn!("qobuz search: no speaker logged into qobuz is bound");
        return body;
    };
    let limits = qobuz::search_limits(provider_spec);
    // The categories are separate speaker requests: ask them at once.
    let mut tasks = tokio::task::JoinSet::new();
    for (index, (kind, n)) in limits.iter().copied().enumerate() {
        let backend = Arc::clone(&backend);
        let (query, cover_proxy_base) = (query.to_string(), cover_proxy_base.to_string());
        tasks.spawn(async move {
            let page = search_page(backend.as_ref(), kind, &query, 0, n, &cover_proxy_base).await;
            (index, page)
        });
    }
    let mut pages = vec![(0, vec![]); limits.len()];
    while let Some(done) = tasks.join_next().await {
        if let Ok((index, page)) = done {
            pages[index] = page;
        }
    }
    let inner = body.result.as_mut().expect("empty search keeps its result");
    let empty_category = |caption: &str, kind: &str| GlobalSearchCategory {
        caption: caption.to_string(),
        count: 0,
        items: vec![],
        link: qobuz::search_link(kind, query),
        total_items: 0,
    };
    let mut top = GlobalSearchTopResults {
        caption: "Top results".to_string(),
        count: 0,
        tracks: vec![],
        albums: vec![],
        artists: vec![],
        playlists: vec![],
    };
    for ((kind, _), (total, items)) in limits.iter().zip(pages) {
        let items: Vec<_> = items.into_iter().map(qobuz::search_hit).collect();
        if let Some(first) = items.first() {
            top.count += 1;
            match *kind {
                "track" => top.tracks.push(first.clone()),
                "album" => top.albums.push(first.clone()),
                "artist" => top.artists.push(first.clone()),
                _ => top.playlists.push(first.clone()),
            }
        }
        let category = GlobalSearchCategory {
            caption: qobuz::search_caption(kind).to_string(),
            count: items.len(),
            items,
            link: qobuz::search_link(kind, query),
            total_items: total,
        };
        match *kind {
            "track" => inner.tracks = Some(category),
            "album" => inner.albums = Some(category),
            "artist" => inner.artists = Some(category),
            _ => inner.playlists = Some(category),
        }
    }
    // Present even though Qobuz has none: the App expects every category it may show.
    inner.shows = Some(empty_category("Podcasts", "show"));
    inner.episodes = Some(empty_category("Episodes", "episode"));
    inner.topresults = Some(top);
    inner.user = qobuz::SERVICE_USER.to_string();
    inner.service = Some(qobuz::search_hit_service());
    body
}

/// A search category's full list ("show more"), paged.
pub(crate) async fn search_category(
    backend: Option<Arc<dyn SpeakerBackend>>,
    kind: &str,
    query: &str,
    start: usize,
    max_items: usize,
    cover_proxy_base: &str,
) -> BrowseResult {
    let Some(backend) = backend else {
        return empty(kind, "", start);
    };
    let (total, items) = search_page(
        backend.as_ref(),
        kind,
        query,
        start,
        max_items,
        cover_proxy_base,
    )
    .await;
    BrowseResult {
        total_items: total,
        items: items.into_iter().map(qobuz::category_hit).collect(),
        ..empty(kind, "", start)
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
    let track = qobuz::known_item(id);
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
    async fn search_fills_each_category_with_hits_and_links() {
        let fake = FakeSpeaker::with_status(SpeakerStatus::default());
        let row = |kind: &str, path: &str, title: &str| {
            json!({"type": if kind == "track" { "audio" } else { "container" },
                "path": path, "title": title, "containerType": kind})
        };
        {
            let mut folders = fake.folders.lock().unwrap();
            folders.insert(
                "qobuz:/search/albums?query=abbey road".into(),
                ServiceRows {
                    total: 120,
                    rows: vec![
                        row("album", "qobuz:/album?id=A1", "Abbey Road"),
                        row("album", "qobuz:/album?id=A2", "Abbey Road (Remaster)"),
                    ],
                },
            );
            folders.insert(
                "qobuz:/search/tracks?query=abbey road".into(),
                ServiceRows {
                    total: 1,
                    rows: vec![row("track", "qobuz:/track?id=T1", "Come Together")],
                },
            );
        }
        let backend: Arc<dyn SpeakerBackend> = fake;
        let body = global_search(
            Some(Arc::clone(&backend)),
            "spotify@qobuz:track#5,album#1",
            "abbey road",
            "http://h:7091",
        )
        .await;
        let result = body.result.unwrap();
        let albums = result.albums.unwrap();
        assert_eq!((albums.count, albums.total_items), (1, 120));
        assert_eq!(albums.items[0].uid.as_deref(), Some("spotify/qobuz"));
        assert_eq!(
            albums.link,
            "audio/cfg/search/spotify/qobuz/album/abbey%20road/0/50"
        );
        assert_eq!(result.tracks.unwrap().items[0].name, "Come Together");
        assert!(result.artists.is_none(), "not asked for");
        assert!(result.shows.unwrap().items.is_empty());
        let top = result.topresults.unwrap();
        assert_eq!((top.tracks.len(), top.albums.len()), (1, 1));
        assert_eq!(result.user, "qobuz");

        let more = search_category(Some(backend), "album", "abbey road", 1, 50, "h").await;
        assert_eq!(more.total_items, 120);
        assert_eq!(more.items[0].id, "spotify:album:A2");
        assert!(global_search(None, "qobuz", "x", "h")
            .await
            .result
            .unwrap()
            .albums
            .is_none());
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
        for (kind, query) in [("album", "abbey road"), ("track", "beatles")] {
            let result = search_category(
                Some(Arc::clone(&backend)),
                kind,
                query,
                0,
                3,
                "http://proxy",
            )
            .await;
            println!("search {kind} {query:?}: {} hits", result.total_items);
            for item in &result.items {
                println!("  {} {}", item.id, item.name);
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
