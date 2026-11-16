//! Per-zone room favorites stored on [`PlayerMapping::room_favorites`] in `audioserver-config.json`.

use anyhow::{Context, Result};
use serde_json::{json, Value};
use urlencoding::decode;

use crate::config::{RoomFavItem, ZoneConfig};
use crate::player_state::{Mode, PlayerState};

/// Optional metadata merged into a new favorite (reference `contentPort.resolveMetadata`).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FavoriteMetadata {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub coverurl: String,
}

impl FavoriteMetadata {
    pub fn from_player_state(state: &PlayerState) -> Self {
        Self {
            title: state.title.clone(),
            artist: state.artist.clone(),
            album: state.album.clone(),
            coverurl: state.cover_url.clone(),
        }
    }
}

/// True when normalized audiopaths refer to the same favorite target.
pub fn paths_match_for_metadata(a: &str, b: &str) -> bool {
    normalize_favorite_audiopath(a) == normalize_favorite_audiopath(b)
}

#[cfg(feature = "miniserver")]
pub async fn resolve_favorite_metadata(audiopath: &str) -> FavoriteMetadata {
    use tracing::debug;

    let normalized = normalize_favorite_audiopath(audiopath);
    let lower = normalized.to_ascii_lowercase();

    if lower.starts_with("tunein:station:") {
        let station_id = normalized["tunein:station:".len()..].trim();
        if !station_id.is_empty() {
            match crate::radio::station_metadata(station_id).await {
                Ok(Some((title, cover))) => {
                    return FavoriteMetadata {
                        title,
                        coverurl: cover.unwrap_or_default(),
                        ..Default::default()
                    };
                }
                Ok(None) => {}
                Err(e) => debug!(error = %e, station_id, "tunein favorite metadata failed"),
            }
        }
    }

    if let Some(id) = normalized.strip_prefix("custom:station:") {
        let store = crate::radio::CustomRadioStore::new(None);
        if let Ok(stations) = store.list().await {
            if let Some(station) = stations.iter().find(|s| s.id == id) {
                return FavoriteMetadata {
                    title: station.name.clone(),
                    coverurl: station.coverurl.clone().unwrap_or_default(),
                    ..Default::default()
                };
            }
        }
    }

    FavoriteMetadata::default()
}

#[cfg(not(feature = "miniserver"))]
pub async fn resolve_favorite_metadata(_audiopath: &str) -> FavoriteMetadata {
    FavoriteMetadata::default()
}

pub struct RoomFavPage {
    pub items: Vec<Value>,
    pub total: usize,
}

fn player_favorites(config: &ZoneConfig, player_id: usize) -> Option<&[RoomFavItem]> {
    config
        .players
        .iter()
        .find(|p| p.player_id == player_id)
        .map(|p| p.room_favorites.as_slice())
}

fn player_favorites_mut(config: &mut ZoneConfig, player_id: usize) -> &mut Vec<RoomFavItem> {
    config.ensure_player_stub(player_id);
    &mut config
        .players
        .iter_mut()
        .find(|p| p.player_id == player_id)
        .expect("player stub")
        .room_favorites
}

fn renumber_slots(items: &mut [RoomFavItem]) {
    for (i, item) in items.iter_mut().enumerate() {
        item.slot = i + 1;
    }
}

fn next_fav_id(items: &[RoomFavItem]) -> usize {
    items.iter().map(|i| i.id).max().unwrap_or(0) + 1
}

/// Legacy virtual-Spotify ids (`spotify@qobuz:track:1`) were stored as `qobuz:track:1`.
fn normalize_playable_item_id(item_id: &str) -> String {
    match item_id.strip_prefix("spotify@qobuz:") {
        Some(rest) => format!("qobuz:{rest}"),
        None => item_id.to_string(),
    }
}

/// Normalize stored audiopath (strip `spotify@provider:` bridge prefix for storage).
pub fn normalize_favorite_audiopath(audiopath: &str) -> String {
    let trimmed = audiopath.trim();
    let normalized = normalize_playable_item_id(trimmed);
    if normalized.starts_with("spotify@") {
        if let Some((_, tail)) = normalized.split_once(':') {
            if tail.starts_with("track:")
                || tail.starts_with("album:")
                || tail.starts_with("playlist:")
                || tail.starts_with("artist:")
            {
                return format!("spotify:{tail}");
            }
        }
    }
    normalized
}

/// Audiopath for playback (stored paths are played back unchanged).
pub fn audiopath_for_playback(stored: &str) -> String {
    stored.trim().to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceInfo {
    pub name: &'static str,
    pub service_type: u8,
}

pub fn detect_service(audiopath: &str) -> ServiceInfo {
    let path = audiopath.to_ascii_lowercase();
    if path.starts_with("library:") || path.starts_with("local:") {
        return ServiceInfo {
            name: "library",
            service_type: 2,
        };
    }
    if path.starts_with("spotify:") || path.starts_with("qobuz:") {
        return ServiceInfo {
            name: "spotify",
            service_type: 3,
        };
    }
    if path.starts_with("tunein:") {
        return ServiceInfo {
            name: "tunein",
            service_type: 3,
        };
    }
    if path.starts_with("linein:") {
        return ServiceInfo {
            name: "linein",
            service_type: 99,
        };
    }
    ServiceInfo {
        name: "custom",
        service_type: 3,
    }
}

pub fn detect_type_from_audiopath(audiopath: &str) -> &'static str {
    let path = audiopath;
    let lower = path.to_ascii_lowercase();
    if lower.starts_with("library:") || lower.starts_with("local:") {
        if lower.contains(":playlist:") {
            return "playlist";
        }
        if lower.contains(":folder:") || lower.contains(":album:") || lower.contains(":artist:") {
            return "library_folder";
        }
        return "library_track";
    }
    if lower.starts_with("spotify:") || lower.starts_with("qobuz:") {
        if lower.contains(":user:collection") || lower.ends_with(":collection") {
            return "spotify_collection";
        }
        if lower.contains(":playlist:") || lower.contains("library-playlist:") {
            return "spotify_playlist";
        }
        if lower.contains(":album:") {
            return "spotify_album";
        }
        if lower.contains(":artist:") {
            return "spotify_artist";
        }
        if lower.contains(":show:") {
            return "spotify_show";
        }
        if lower.contains(":episode:") {
            return "spotify_episode";
        }
        return "spotify_track";
    }
    if lower.starts_with("linein:") {
        return "linein";
    }
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return "custom_stream";
    }
    if lower.starts_with("tunein:") || lower.contains("tunein") || lower.contains("radio") {
        return "tunein";
    }
    if lower.starts_with("custom:station:") {
        return "custom_stream";
    }
    "custom_stream"
}

fn apply_metadata(item: &mut RoomFavItem, meta: &FavoriteMetadata, fallback_name: &str) {
    if !meta.title.is_empty() {
        item.title = meta.title.clone();
        item.name = meta.title.clone();
    } else {
        item.title = fallback_name.to_string();
        item.name = fallback_name.to_string();
    }
    if !meta.artist.is_empty() {
        item.artist = meta.artist.clone();
    }
    if !meta.album.is_empty() {
        item.album = meta.album.clone();
    }
    if !meta.coverurl.is_empty() {
        item.coverurl = meta.coverurl.clone();
    }
}

fn build_item(
    name: &str,
    audiopath: &str,
    id: usize,
    slot: usize,
    meta: &FavoriteMetadata,
) -> RoomFavItem {
    let audiopath = normalize_favorite_audiopath(audiopath);
    let service = detect_service(&audiopath);
    let item_type = detect_type_from_audiopath(&audiopath);
    let mut item = RoomFavItem {
        id,
        slot,
        plus: true,
        name: name.to_string(),
        title: name.to_string(),
        audiopath,
        item_type: item_type.to_string(),
        coverurl: String::new(),
        artist: String::new(),
        album: String::new(),
        service: service.name.to_string(),
        service_type: service.service_type,
        owner: String::new(),
    };
    apply_metadata(&mut item, meta, name);
    item
}

pub fn item_to_wire_json(item: &RoomFavItem) -> Value {
    let mut v = serde_json::to_value(item).expect("room fav item serializes");
    let obj = v.as_object_mut().expect("room fav object");
    if matches!(
        item.item_type.as_str(),
        "spotify_playlist" | "spotify_collection"
    ) {
        let owner = item.owner.clone();
        obj.insert("ownerId".into(), json!(owner));
        obj.insert("owner_id".into(), json!(owner));
    }
    Value::Object(obj.clone())
}

pub fn get_page(player_id: usize, start: usize, max_items: usize) -> RoomFavPage {
    let config = ZoneConfig::get();
    let all = player_favorites(&config, player_id).unwrap_or(&[]);
    let total = all.len();
    let page: Vec<Value> = all
        .iter()
        .skip(start)
        .take(max_items)
        .map(item_to_wire_json)
        .collect();
    RoomFavPage { items: page, total }
}

pub fn count(player_id: usize) -> usize {
    let config = ZoneConfig::get();
    player_favorites(&config, player_id)
        .map(|s| s.len())
        .unwrap_or(0)
}

pub fn lookup(player_id: usize, favorite_id: usize) -> Option<RoomFavItem> {
    let config = ZoneConfig::get();
    player_favorites(&config, player_id)?
        .iter()
        .find(|i| i.id == favorite_id)
        .cloned()
}

pub fn add(
    player_id: usize,
    name: &str,
    audiopath: &str,
    meta: FavoriteMetadata,
) -> Result<RoomFavItem> {
    ZoneConfig::update(|config| {
        let items = player_favorites_mut(config, player_id);
        let id = next_fav_id(items);
        let slot = items.len() + 1;
        let item = build_item(name, audiopath, id, slot, &meta);
        items.push(item.clone());
        Ok(item)
    })
}

pub fn add_from_player_state(player_id: usize, state: &PlayerState) -> Result<RoomFavItem> {
    if state.audiopath.is_empty() {
        anyhow::bail!("no audiopath in player state");
    }
    if !matches!(state.mode, Mode::Play | Mode::Pause) {
        anyhow::bail!("player not playing");
    }
    let name = if state.title.is_empty() {
        state.audiopath.clone()
    } else {
        state.title.clone()
    };
    ZoneConfig::update(|config| {
        let items = player_favorites_mut(config, player_id);
        let id = next_fav_id(items);
        let slot = items.len() + 1;
        let service = detect_service(&state.audiopath);
        let item_type = detect_type_from_audiopath(&state.audiopath);
        let item = RoomFavItem {
            id,
            slot,
            plus: true,
            name: name.clone(),
            title: name,
            audiopath: normalize_favorite_audiopath(&state.audiopath),
            item_type: item_type.to_string(),
            coverurl: state.cover_url.clone(),
            artist: state.artist.clone(),
            album: state.album.clone(),
            service: service.name.to_string(),
            service_type: service.service_type,
            owner: if state.audiopath.starts_with("qobuz:") {
                "qobuz".into()
            } else {
                String::new()
            },
        };
        items.push(item.clone());
        Ok(item)
    })
}

pub fn remove(player_id: usize, favorite_id: usize) -> Result<()> {
    ZoneConfig::update(|config| {
        let items = player_favorites_mut(config, player_id);
        let before = items.len();
        items.retain(|i| i.id != favorite_id);
        if items.len() == before {
            anyhow::bail!("favorite {favorite_id} not found for player {player_id}");
        }
        renumber_slots(items);
        Ok(())
    })
}

pub fn set_name(player_id: usize, favorite_id: usize, name: &str) -> Result<()> {
    ZoneConfig::update(|config| {
        let items = player_favorites_mut(config, player_id);
        let item = items
            .iter_mut()
            .find(|i| i.id == favorite_id)
            .with_context(|| format!("favorite {favorite_id} not found"))?;
        item.name = name.to_string();
        item.title = name.to_string();
        Ok(())
    })
}

pub fn set_id(player_id: usize, old_id: usize, new_id: usize) -> Result<()> {
    ZoneConfig::update(|config| {
        let items = player_favorites_mut(config, player_id);
        let item = items
            .iter_mut()
            .find(|i| i.id == old_id)
            .with_context(|| format!("favorite {old_id} not found"))?;
        item.id = new_id;
        Ok(())
    })
}

pub fn reorder(player_id: usize, order: &[usize]) -> Result<()> {
    ZoneConfig::update(|config| {
        let items = player_favorites_mut(config, player_id);
        let by_id: std::collections::HashMap<usize, RoomFavItem> =
            items.drain(..).map(|i| (i.id, i)).collect();
        let mut ordered = Vec::new();
        for &id in order {
            if let Some(item) = by_id.get(&id) {
                ordered.push(item.clone());
            }
        }
        for item in by_id.values() {
            if !ordered.iter().any(|o| o.id == item.id) {
                ordered.push(item.clone());
            }
        }
        renumber_slots(&mut ordered);
        *items = ordered;
        Ok(())
    })
}

pub fn copy_to_zones(source_player_id: usize, dest_player_ids: &[usize]) -> Result<()> {
    let source_items = ZoneConfig::get()
        .players
        .iter()
        .find(|p| p.player_id == source_player_id)
        .map(|p| p.room_favorites.clone())
        .unwrap_or_default();
    ZoneConfig::update(|config| {
        for &dest_id in dest_player_ids {
            if dest_id == source_player_id {
                continue;
            }
            let dest = player_favorites_mut(config, dest_id);
            *dest = source_items.clone();
            renumber_slots(dest);
        }
        Ok(())
    })
}

pub fn decode_segment(segment: &str) -> String {
    decode(segment)
        .map(|s| s.into_owned())
        .unwrap_or_else(|_| segment.to_string())
}

pub fn parse_add_tail(rest: &str) -> Result<(String, String)> {
    let rest = rest.trim_start_matches('/');
    let mut parts = rest.splitn(2, '/');
    let title = parts
        .next()
        .filter(|s| !s.is_empty())
        .context("missing favorite title")?;
    let audiopath = parts.next().context("missing favorite audiopath")?;
    Ok((decode_segment(title), audiopath.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{PlayerMapping, ZoneConfig};

    use crate::config::AudioserverConfigFile;

    fn with_test_config(config: ZoneConfig, f: impl FnOnce()) {
        AudioserverConfigFile::with_test_cache(
            AudioserverConfigFile {
                audioserver: config,
                ..Default::default()
            },
            f,
        );
    }

    #[test]
    fn detect_tunein_type() {
        assert_eq!(detect_type_from_audiopath("tunein:station:s2611"), "tunein");
        assert_eq!(detect_service("tunein:station:s2611").name, "tunein");
    }

    #[test]
    fn detect_qobuz_type() {
        assert_eq!(
            detect_type_from_audiopath("qobuz:track:123"),
            "spotify_track"
        );
        assert_eq!(detect_service("qobuz:track:123").name, "spotify");
    }

    #[test]
    fn add_list_remove_round_trip() {
        with_test_config(
            ZoneConfig {
                players: vec![PlayerMapping {
                    player_id: 3,
                    loxone_uuid: None,
                    name: "Kitchen".into(),
                    cast_device_id: None,
                    volume_level: None,
                    ..Default::default()
                }],
            },
            || {
                let item = add(
                    3,
                    "Jazz FM",
                    "tunein:station:s99",
                    FavoriteMetadata::default(),
                )
                .unwrap();
                assert_eq!(item.id, 1);
                assert_eq!(count(3), 1);
                let page = get_page(3, 0, 50);
                assert_eq!(page.total, 1);
                remove(3, 1).unwrap();
                assert_eq!(count(3), 0);
            },
        );
    }

    #[test]
    fn playlist_wire_includes_owner_id() {
        let item = RoomFavItem {
            id: 1,
            slot: 1,
            plus: true,
            name: "List".into(),
            title: "List".into(),
            audiopath: "spotify:playlist:abc".into(),
            item_type: "spotify_playlist".into(),
            coverurl: String::new(),
            artist: String::new(),
            album: String::new(),
            service: "spotify".into(),
            service_type: 3,
            owner: "Timo".into(),
        };
        let wire = item_to_wire_json(&item);
        assert_eq!(wire["ownerId"], "Timo");
        assert_eq!(wire["owner_id"], "Timo");
    }

    #[test]
    fn persists_across_cache_reload() {
        with_test_config(ZoneConfig::default(), || {
            add(1, "Track", "qobuz:track:42", FavoriteMetadata::default()).unwrap();
            let json = serde_json::to_string(&ZoneConfig::get()).unwrap();
            let loaded: ZoneConfig = serde_json::from_str(&json).unwrap();
            ZoneConfig::reset_cache_for_test(loaded);
            assert_eq!(count(1), 1);
            assert_eq!(lookup(1, 1).unwrap().audiopath, "qobuz:track:42");
        });
    }

    #[test]
    fn add_id_is_not_player_id() {
        with_test_config(ZoneConfig::default(), || {
            let player_id = 3;
            let item = add(
                player_id,
                "Station",
                "tunein:station:s2611",
                FavoriteMetadata::default(),
            )
            .unwrap();
            assert_ne!(item.id, player_id);
            assert_eq!(item.id, 1);
        });
    }

    #[test]
    fn add_applies_metadata_cover() {
        let meta = FavoriteMetadata {
            title: "Jazz FM".into(),
            coverurl: "http://cdn/logo.png".into(),
            ..Default::default()
        };
        let item = build_item("Jazz FM", "tunein:station:s99", 1, 1, &meta);
        assert_eq!(item.coverurl, "http://cdn/logo.png");
        assert_eq!(item.title, "Jazz FM");
    }

    #[test]
    fn paths_match_normalizes_bridge_prefix() {
        assert!(paths_match_for_metadata(
            "spotify@qobuz:track:42",
            "qobuz:track:42"
        ));
        assert!(paths_match_for_metadata(
            "spotify@provider:track:99",
            "spotify:track:99"
        ));
        assert!(!paths_match_for_metadata("qobuz:track:1", "qobuz:track:2"));
    }

    #[test]
    fn detect_qobuz_playlist_and_album_types() {
        assert_eq!(
            detect_type_from_audiopath("qobuz:playlist:abc"),
            "spotify_playlist"
        );
        assert_eq!(
            detect_type_from_audiopath("qobuz:album:999"),
            "spotify_album"
        );
        assert_eq!(detect_service("qobuz:album:999").service_type, 3);
    }

    #[test]
    fn detect_library_and_linein_services() {
        assert_eq!(detect_service("library:track:1").name, "library");
        assert_eq!(
            detect_type_from_audiopath("library:album:1"),
            "library_folder"
        );
        assert_eq!(detect_service("linein:1").name, "linein");
        assert_eq!(detect_type_from_audiopath("linein:1"), "linein");
    }

    #[test]
    fn add_multiple_list_page_and_remove() {
        with_test_config(ZoneConfig::default(), || {
            add(1, "A", "tunein:station:s1", FavoriteMetadata::default()).unwrap();
            add(1, "B", "qobuz:track:2", FavoriteMetadata::default()).unwrap();
            assert_eq!(count(1), 2);
            let page = get_page(1, 0, 1);
            assert_eq!(page.total, 2);
            assert_eq!(page.items.len(), 1);
            let page2 = get_page(1, 1, 10);
            assert_eq!(page2.items.len(), 1);
            remove(1, 1).unwrap();
            assert_eq!(count(1), 1);
            assert_eq!(lookup(1, 1), None);
            assert!(lookup(1, 2).is_some());
        });
    }

    #[test]
    fn persistence_round_trip_via_json_cache() {
        with_test_config(ZoneConfig::default(), || {
            add(
                2,
                "Come Together",
                "qobuz:track:12345",
                FavoriteMetadata {
                    title: "Come Together".into(),
                    ..Default::default()
                },
            )
            .unwrap();
            let serialized = serde_json::to_string(&ZoneConfig::get()).unwrap();
            ZoneConfig::reset_cache_for_test(serde_json::from_str(&serialized).unwrap());
            let item = lookup(2, 1).unwrap();
            assert_eq!(item.audiopath, "qobuz:track:12345");
            assert_eq!(item.item_type, "spotify_track");
        });
    }
}
