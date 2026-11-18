//! Qobuz in the Loxone App, browsed and played through a speaker logged into Qobuz.
//!
//! The Loxone App only accepts `spotify` (and `soundsuit`) music services, so Qobuz shows as a
//! virtual Spotify account (`cmd:"spotify"`, `user:"qobuz"`). Browsing reads the speaker's own
//! Qobuz tree (StreamUnlimited `getRows qobuz:`); playing sends the speaker a `mediaRoles` for
//! the item. See [docs/speakers/streamunlimited.md](../docs/speakers/streamunlimited.md).
//!
//! Loxone ids cannot contain `/`, so speaker paths map to ids:
//! `qobuz:/album?id=X` ↔ `qobuz:album:X` (also `playlist`, `track`, `artist`), any other
//! container ↔ `qobuz:folder:<base64url of the path>`.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde_json::{json, Value};

use crate::remote::{BrowseResultItem, FILE_TYPE_FOLDER, FILE_TYPE_TRACK};

/// `cmd` of the virtual account: the Loxone App only accepts Spotify-shaped services.
pub const SERVICE_CMD: &str = "spotify";
/// `user` of the virtual account. Must be the same in `getservices`, search results and items.
pub const SERVICE_USER: &str = "qobuz";
/// Loxone item type for albums and playlists that open and play as a whole.
const FILE_TYPE_PLAYLIST_BROWSABLE: usize = 7;
/// Loxone item type for service items such as artists.
const FILE_TYPE_SERVICE_ITEM: usize = 12;

const SPEAKER_ROOT: &str = "qobuz:";
const FOLDER_PREFIX: &str = "qobuz:folder:";
/// Catalogue items whose speaker path is `qobuz:/<kind>?id=<id>`.
const ITEM_KINDS: [&str; 4] = ["album", "playlist", "track", "artist"];

/// Whether a `getservicefolder` / play request names this service.
pub fn is_service(service: &str) -> bool {
    service.eq_ignore_ascii_case(SERVICE_CMD) || service.eq_ignore_ascii_case(SERVICE_USER)
}

/// The `getservices` row of the virtual account.
pub fn service_entry() -> Value {
    json!({
        "cmd": SERVICE_CMD,
        "name": "Qobuz",
        "icon": "https://static.qobuz.com/img/logo/qobuz.svg",
        "id": SERVICE_USER,
        "user": SERVICE_USER,
        "uid": format!("{SERVICE_CMD}/{SERVICE_USER}"),
        "product": "",
        "asdefault": [],
        "offline_storage": [],
        "configerror": false,
        "provider": SERVICE_USER,
        "fake": true,
    })
}

/// What a Loxone folder id stands for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Folder {
    /// The service's start page: the Loxone App builds it from the fixed home folders, so
    /// it answers empty.
    Root,
    /// A home folder this service has no content for (genres, podcasts).
    Empty,
    /// A folder in the speaker's Qobuz tree.
    Speaker { path: String, name: &'static str },
}

/// Map a Loxone folder id to the speaker's tree. The Loxone Spotify start page asks for the
/// fixed ids `0`–`7`; other ids are ones this module handed out (see [`loxone_id`]).
pub fn folder(id: &str) -> Folder {
    let speaker = |path: &str, name| Folder::Speaker {
        path: path.to_string(),
        name,
    };
    match id {
        "" | "start" | "root" | "noUser" | "nouser" | "spotify" | "qobuz" => Folder::Root,
        "0" | "popular" | "features" => speaker("qobuz:/featured/playlists", "Playlists"),
        "1" | "new-releases" => speaker("qobuz:/featured/albums", "New Releases"),
        "3" | "playlists" | "qobuz:playlists" => speaker("qobuz:/user/playlists/", "Playlists"),
        "4" | "liked" | "liked-songs" | "user:collection" => {
            speaker("qobuz:/user/favourites/tracks", "Liked Songs")
        }
        "5" | "albums" => speaker("qobuz:/user/favourites/albums", "Albums"),
        "6" | "artists" => speaker("qobuz:/user/favourites/artists", "Artists"),
        "2" | "genres" | "categories" | "7" | "podcasts" | "shows" => Folder::Empty,
        other => match speaker_path(other) {
            Some(path) => Folder::Speaker { path, name: "" },
            None => Folder::Empty,
        },
    }
}

/// The Loxone id for a speaker path (see the module docs).
pub fn loxone_id(path: &str) -> String {
    for kind in ITEM_KINDS {
        if let Some(id) = path.strip_prefix(&format!("qobuz:/{kind}?id=")) {
            if !id.is_empty() && !id.contains(['&', '/']) {
                return format!("qobuz:{kind}:{id}");
            }
        }
    }
    format!("{FOLDER_PREFIX}{}", URL_SAFE_NO_PAD.encode(path))
}

/// The speaker path for a Loxone id; `None` when the id is not one of ours.
pub fn speaker_path(id: &str) -> Option<String> {
    let id = normalize_id(id)?;
    if let Some(encoded) = id.strip_prefix(FOLDER_PREFIX) {
        let path = String::from_utf8(URL_SAFE_NO_PAD.decode(encoded).ok()?).ok()?;
        return path.starts_with(SPEAKER_ROOT).then_some(path);
    }
    let (kind, item) = id.strip_prefix("qobuz:")?.split_once(':')?;
    (ITEM_KINDS.contains(&kind) && !item.is_empty()).then(|| format!("qobuz:/{kind}?id={item}"))
}

/// Item ids the Loxone App sends back in several spellings: `spotify:album:X`,
/// `spotify@qobuz:album:X` and `qobuz:album:X` all become `qobuz:album:X`.
pub fn normalize_id(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.starts_with("qobuz:") {
        return Some(raw.to_string());
    }
    let rest = raw.strip_prefix("spotify")?;
    let rest = match rest.strip_prefix('@') {
        Some(tagged) => tagged.split_once(':')?.1,
        None => rest.strip_prefix(':')?,
    };
    let (kind, _) = rest.split_once(':')?;
    ITEM_KINDS.contains(&kind).then(|| format!("qobuz:{rest}"))
}

/// Kind of a Loxone item id (`album`, `playlist`, `track`, `artist`), if it is one.
pub fn item_kind(id: &str) -> Option<&'static str> {
    let id = normalize_id(id)?;
    let kind = id.strip_prefix("qobuz:")?.split(':').next()?;
    ITEM_KINDS.into_iter().find(|k| *k == kind)
}

/// Audiopath of the track that plays, from the speaker's `trackRoles.path`
/// (`qobuz:/track?id=X` → `qobuz:track:X`).
pub fn track_audiopath(track_path: &str) -> Option<String> {
    let id = loxone_id(track_path);
    (item_kind(&id) == Some("track")).then_some(id)
}

/// What an item shows: for a track, what its `mediaRoles` carries while it plays (the speaker
/// does not look a track up); for any item, a room favorite's title and cover.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ItemInfo {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub cover_url: String,
}

/// Items seen while browsing or playing, so an item played or saved by id can show its title
/// and cover (see [`media_for`], room favorites).
static ITEMS: LazyLock<Mutex<HashMap<String, ItemInfo>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
/// Items remembered at most; the list starts over when full.
const ITEM_CACHE_LIMIT: usize = 2000;

/// Remember what an item shows (`id` in any of the App's spellings).
pub fn remember_item(id: &str, info: ItemInfo) {
    let (Some(id), Ok(mut items)) = (normalize_id(id), ITEMS.lock()) else {
        return;
    };
    if info.title.is_empty() {
        return;
    }
    if items.len() >= ITEM_CACHE_LIMIT && !items.contains_key(&id) {
        items.clear();
    }
    items.insert(id, info);
}

/// Remember what the track that plays shows. Called for every status publish (the position
/// changes each second), so an entry that already says the same is left alone, without cloning.
pub fn remember_playing(track_path: &str, title: &str, artist: &str, album: &str, cover_url: &str) {
    let Some(id) = track_audiopath(track_path) else {
        return;
    };
    if title.is_empty() {
        return;
    }
    if let Ok(items) = ITEMS.lock() {
        if items.get(&id).is_some_and(|known| {
            known.title == title
                && known.artist == artist
                && known.album == album
                && known.cover_url == cover_url
        }) {
            return;
        }
    }
    remember_item(
        &id,
        ItemInfo {
            title: title.to_string(),
            artist: artist.to_string(),
            album: album.to_string(),
            cover_url: cover_url.to_string(),
        },
    );
}

/// Remember the catalogue items among browsed rows.
pub fn remember_rows(rows: &[Value]) {
    for row in rows {
        let id = loxone_id(text(row, "path"));
        if item_kind(&id).is_none() {
            continue;
        }
        remember_item(
            &id,
            ItemInfo {
                title: text(row, "title").to_string(),
                artist: meta(row, "artist").to_string(),
                album: meta(row, "album").to_string(),
                cover_url: text(row, "icon").to_string(),
            },
        );
    }
}

/// An item remembered by [`remember_item`] / [`remember_rows`].
pub fn known_item(id: &str) -> Option<ItemInfo> {
    let id = normalize_id(id)?;
    ITEMS.lock().ok()?.get(&id).cloned()
}

/// The cover URL behind our cors-proxy URL (`…/cors-proxy?url=<encoded>&…`), or `url` itself.
pub fn unproxied_cover(url: &str) -> String {
    url.split_once("/cors-proxy?url=")
        .map(|(_, rest)| rest.split('&').next().unwrap_or(rest))
        .and_then(|encoded| urlencoding::decode(encoded).ok())
        .map(|decoded| decoded.into_owned())
        .unwrap_or_else(|| url.to_string())
}

/// The `mediaRoles` that plays a Loxone item on the speaker.
///
/// Verified on a Mu-so Qb 2 (2026-11-18): an album, playlist or artist plays from its path
/// alone. A track needs its `prePlayPath`, and without a title the speaker reports none.
pub fn media_for(id: &str, track: Option<&ItemInfo>) -> Option<Value> {
    let path = speaker_path(id)?;
    if item_kind(id) != Some("track") {
        return Some(json!({ "type": "container", "path": path, "containerPlayable": true }));
    }
    let info = track.cloned().unwrap_or_default();
    let mut meta = json!({
        "serviceID": "qobuz",
        "prePlayPath": path.replacen("/track?", "/preplay?", 1),
    });
    if !info.artist.is_empty() {
        meta["artist"] = json!(info.artist);
    }
    if !info.album.is_empty() {
        meta["album"] = json!(info.album);
    }
    let mut media = json!({
        "type": "audio",
        "path": path,
        "audioType": "musicTrack",
        "mediaData": { "metaData": meta },
    });
    if !info.title.is_empty() {
        media["title"] = json!(info.title);
    }
    if !info.cover_url.is_empty() {
        media["icon"] = json!(info.cover_url);
    }
    Some(media)
}

/// What the Loxone App should play for a `serviceplay` / `playurl` / `library/play` path.
///
/// A track tapped inside an album or playlist (`…/parentpath/<container>/…` or
/// `…/parentid/<container>/…`) plays that container from the start: the speaker cannot start
/// a container at a given track. Returns the normalized Loxone id.
pub fn play_target(rest: &str) -> Option<String> {
    let segments: Vec<String> = rest
        .split('/')
        .map(|s| {
            urlencoding::decode(s)
                .map(|d| d.into_owned())
                .unwrap_or_else(|_| s.to_string())
        })
        .collect();
    for marker in ["parentpath", "parentid"] {
        if let Some(pos) = segments.iter().position(|s| s == marker) {
            if let Some(parent) = segments.get(pos + 1).and_then(|p| normalize_id(p)) {
                if matches!(item_kind(&parent), Some("album" | "playlist" | "artist")) {
                    return Some(parent);
                }
            }
        }
    }
    segments
        .iter()
        .take_while(|s| !s.starts_with('?'))
        .find_map(|s| normalize_id(s).filter(|id| item_kind(id).is_some()))
}

fn meta<'a>(row: &'a Value, key: &str) -> &'a str {
    row.pointer(&format!("/mediaData/metaData/{key}"))
        .and_then(Value::as_str)
        .unwrap_or("")
}

fn text<'a>(row: &'a Value, key: &str) -> &'a str {
    row.get(key).and_then(Value::as_str).unwrap_or("")
}

fn optional(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_string())
}

/// A speaker `getRows` row as a Loxone browse item. `cover` turns a cover URL into the one
/// the App loads (the cover proxy).
pub fn browse_item(row: &Value, slot: usize, cover: &dyn Fn(&str) -> String) -> BrowseResultItem {
    let path = text(row, "path");
    let id = loxone_id(path);
    let title = text(row, "title").to_string();
    let icon = optional(text(row, "icon")).map(|url| cover(&url));
    let base = BrowseResultItem {
        id: id.clone(),
        name: title.clone(),
        slot,
        coverurl: icon.clone(),
        thumbnail: icon,
        ..Default::default()
    };
    let service_item = |tag: &str, item_type, content: (&str, &str)| BrowseResultItem {
        audiopath: Some(id.clone()),
        title: Some(title.clone()),
        tag: Some(tag.to_string()),
        content_type: Some(content.0.to_string()),
        media_type: Some(content.1.to_string()),
        r#type: item_type,
        ..base.clone()
    };
    if text(row, "type") == "audio" {
        let artist = meta(row, "artist");
        return BrowseResultItem {
            artist: optional(artist),
            album: optional(meta(row, "album")),
            name: if artist.is_empty() {
                title.clone()
            } else {
                format!("{title} — {artist}")
            },
            ..service_item("track", FILE_TYPE_TRACK, ("Service", "service"))
        };
    }
    match text(row, "containerType") {
        "album" => BrowseResultItem {
            artist: optional(meta(row, "artist")),
            ..service_item(
                "album",
                FILE_TYPE_PLAYLIST_BROWSABLE,
                ("Service", "service"),
            )
        },
        "playlist" if item_kind(&id) == Some("playlist") => BrowseResultItem {
            artist: Some(String::new()),
            ..service_item(
                "playlist",
                FILE_TYPE_PLAYLIST_BROWSABLE,
                ("Playlists", "playlist"),
            )
        },
        "person" => BrowseResultItem {
            audiopath: None,
            ..service_item("artist", FILE_TYPE_SERVICE_ITEM, ("Service", "service"))
        },
        _ => BrowseResultItem {
            r#type: FILE_TYPE_FOLDER,
            ..base
        },
    }
}

/// Search categories Qobuz has, in the order the Loxone App shows them.
pub const SEARCH_KINDS: [&str; 4] = ["track", "album", "artist", "playlist"];

/// Whether a global search provider spec (`spotify@qobuz:track#5,album#5`, `qobuz`, …) is
/// for this service.
pub fn is_search_provider(spec: &str) -> bool {
    let provider = spec.split(['@', ':']).next().unwrap_or(spec);
    is_service(provider)
}

/// Rows per category asked by a provider spec (`track#5,album#5`); 5 for each category the
/// spec does not list, as the Loxone App sends none on its first search.
pub fn search_limits(spec: &str) -> Vec<(&'static str, usize)> {
    let listed: Vec<(&str, usize)> = spec
        .split_once(':')
        .map(|(_, list)| list)
        .unwrap_or("")
        .split(',')
        .filter_map(|entry| {
            let (kind, n) = entry.split_once('#')?;
            Some((kind.trim(), n.trim().parse().ok()?))
        })
        .collect();
    SEARCH_KINDS
        .into_iter()
        .filter_map(|kind| match listed.iter().find(|(k, _)| *k == kind) {
            Some((_, 0)) => None,
            Some((_, n)) => Some((kind, *n)),
            None if listed.is_empty() => Some((kind, 5)),
            None => None,
        })
        .collect()
}

/// Speaker search folder for a Loxone search category; `None` for categories Qobuz lacks.
///
/// The speaker splits the path's own `?query=` part without decoding it (Mu-so Qb 2,
/// 2026-11-18): `abbey%20road` finds nothing, and `rock & roll` is cut at the `&`. So the
/// query goes in raw, with the characters that would end or change it (`&`, `#`, `?`, `=`,
/// `+`) turned into spaces.
pub fn search_path(kind: &str, query: &str) -> Option<String> {
    let folder = match kind {
        "track" | "tracks" => "tracks",
        "album" | "albums" => "albums",
        "artist" | "artists" => "artists",
        "playlist" | "playlists" => "playlists",
        _ => return None,
    };
    let query: String = query
        .chars()
        .map(|c| {
            if matches!(c, '&' | '#' | '?' | '=' | '+') {
                ' '
            } else {
                c
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    Some(format!("qobuz:/search/{folder}?query={query}"))
}

/// The App's link to a search category's full list (`audio/cfg/search/…`).
pub fn search_link(kind: &str, query: &str) -> String {
    format!(
        "audio/cfg/search/{SERVICE_CMD}/{SERVICE_USER}/{kind}/{}/0/50",
        urlencoding::encode(query)
    )
}

/// Caption of a search category.
pub fn search_caption(kind: &str) -> &'static str {
    match kind {
        "track" => "Tracks",
        "album" => "Albums",
        "artist" => "Artists",
        _ => "Playlists",
    }
}

/// The account stub search results and hits carry as `service`.
pub fn search_hit_service() -> Value {
    json!({
        "cmd": SERVICE_CMD,
        "id": SERVICE_USER,
        "user": SERVICE_USER,
        "uid": format!("{SERVICE_CMD}/{SERVICE_USER}"),
        "fake": true,
        "provider": SERVICE_USER,
    })
}

/// A browse item as a global search hit. The App opens a hit through its own service
/// (`ServiceLoader`), so hits carry the account at their root and nested as `service`; the
/// `user` must equal the `getservices` row's.
pub fn search_hit(mut item: BrowseResultItem) -> BrowseResultItem {
    if item.r#type == FILE_TYPE_TRACK {
        item.name = item.title.clone().unwrap_or(item.name);
        return item;
    }
    item.r#type = FILE_TYPE_SERVICE_ITEM;
    item.audiopath = Some(item.id.clone());
    item.cmd = Some(SERVICE_CMD.to_string());
    item.user = Some(SERVICE_USER.to_string());
    item.uid = Some(format!("{SERVICE_CMD}/{SERVICE_USER}"));
    item.fake = Some(true);
    item.provider = Some(SERVICE_USER.to_string());
    item.service = Some(search_hit_service());
    item
}

/// A browse item for a search category's "show more" list, which the Loxone App reads with
/// `spotify:` ids (as the earlier Qobuz bridge sent them); play and open map them back.
pub fn category_hit(item: BrowseResultItem) -> BrowseResultItem {
    let spotify = |id: &str| match id.strip_prefix("qobuz:") {
        Some(rest) if item_kind(id).is_some() => format!("{SERVICE_CMD}:{rest}"),
        _ => id.to_string(),
    };
    let mut hit = search_hit(item);
    hit.id = spotify(&hit.id);
    hit.audiopath = hit.audiopath.as_deref().map(spotify);
    hit
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_proxy(url: &str) -> String {
        url.to_string()
    }

    #[test]
    fn item_paths_and_loxone_ids_round_trip() {
        for (path, id) in [
            ("qobuz:/album?id=0060254746417", "qobuz:album:0060254746417"),
            ("qobuz:/playlist?id=1001", "qobuz:playlist:1001"),
            ("qobuz:/track?id=19301399", "qobuz:track:19301399"),
            ("qobuz:/artist?id=26390", "qobuz:artist:26390"),
        ] {
            assert_eq!(loxone_id(path), id);
            assert_eq!(speaker_path(id).as_deref(), Some(path));
        }
        let folder = loxone_id("qobuz:/user/purchases/albums");
        assert!(folder.starts_with(FOLDER_PREFIX) && !folder.contains('/'));
        assert_eq!(
            speaker_path(&folder).as_deref(),
            Some("qobuz:/user/purchases/albums")
        );
        // A folder id never decodes to a path outside the Qobuz tree.
        let foreign = format!("{FOLDER_PREFIX}{}", URL_SAFE_NO_PAD.encode("settings:/x"));
        assert_eq!(speaker_path(&foreign), None);
        assert_eq!(speaker_path("tunein:station:s1"), None);
        assert_eq!(speaker_path("qobuz:podcast:1"), None);
    }

    #[test]
    fn app_spellings_of_an_item_normalize() {
        for raw in [
            "qobuz:album:X1",
            "spotify:album:X1",
            "spotify@qobuz:album:X1",
        ] {
            assert_eq!(
                normalize_id(raw).as_deref(),
                Some("qobuz:album:X1"),
                "{raw}"
            );
        }
        assert_eq!(normalize_id("spotify:show:1"), None);
        assert_eq!(normalize_id("tunein:station:s1"), None);
        assert_eq!(item_kind("spotify:track:5"), Some("track"));
    }

    #[test]
    fn home_folders_map_to_the_speaker_tree() {
        assert_eq!(folder("start"), Folder::Root);
        assert_eq!(folder("noUser"), Folder::Root);
        assert_eq!(folder("2"), Folder::Empty);
        assert_eq!(folder("7"), Folder::Empty);
        assert_eq!(
            folder("4"),
            Folder::Speaker {
                path: "qobuz:/user/favourites/tracks".into(),
                name: "Liked Songs"
            }
        );
        assert_eq!(
            folder("qobuz:album:A1"),
            Folder::Speaker {
                path: "qobuz:/album?id=A1".into(),
                name: ""
            }
        );
        assert_eq!(folder("unknown"), Folder::Empty);
    }

    #[test]
    fn containers_play_from_their_path_and_tracks_carry_what_they_show() {
        assert_eq!(
            media_for("qobuz:album:A1", None).unwrap(),
            json!({"type": "container", "path": "qobuz:/album?id=A1", "containerPlayable": true})
        );
        let info = ItemInfo {
            title: "Faith".into(),
            artist: "The Cure".into(),
            album: "Faith".into(),
            cover_url: "https://static.qobuz.com/c.jpg".into(),
        };
        let track = media_for("spotify:track:723561", Some(&info)).unwrap();
        assert_eq!(track["type"], "audio");
        assert_eq!(track["path"], "qobuz:/track?id=723561");
        assert_eq!(
            track["mediaData"]["metaData"]["prePlayPath"],
            "qobuz:/preplay?id=723561"
        );
        assert_eq!(track["title"], "Faith");
        assert_eq!(track["icon"], "https://static.qobuz.com/c.jpg");
        let bare = media_for("qobuz:track:1", None).unwrap();
        assert!(bare.get("title").is_none());
        assert_eq!(media_for("tunein:station:s1", None), None);
    }

    #[test]
    fn play_paths_from_the_app_resolve_to_an_item() {
        assert_eq!(
            play_target("spotify/qobuz/qobuz:album:trrcz9pvaaz6b/?q&ZW5mb3JjZVVzZXI9dHJ1ZQ")
                .as_deref(),
            Some("qobuz:album:trrcz9pvaaz6b")
        );
        assert_eq!(
            play_target("spotify/qobuz/qobuz:playlist:1001/noshuffle/?q&ZW5mb3JjZVVzZXI9dHJ1ZQ")
                .as_deref(),
            Some("qobuz:playlist:1001")
        );
        assert_eq!(
            play_target("qobuz:track:64868955/noshuffle").as_deref(),
            Some("qobuz:track:64868955")
        );
        // A track inside a playlist plays the playlist.
        assert_eq!(
            play_target(
                "spotify/qobuz/qobuz:track:44856655/parentpath/qobuz:playlist:1001/21/noshuffle/?q&x"
            )
            .as_deref(),
            Some("qobuz:playlist:1001")
        );
        assert_eq!(
            play_target("spotify%3Atrack%3A5/noshuffle").as_deref(),
            Some("qobuz:track:5")
        );
        assert_eq!(play_target("radio/nouser/tunein:station:s1"), None);
    }

    #[test]
    fn speaker_rows_become_loxone_items() {
        let album = json!({"type": "container", "path": "qobuz:/album?id=A1", "containerPlayable": true,
            "title": "Abbey Road", "icon": "https://static.qobuz.com/a.jpg", "containerType": "album"});
        let item = browse_item(&album, 1, &|u| format!("proxy:{u}"));
        assert_eq!(item.id, "qobuz:album:A1");
        assert_eq!(item.audiopath.as_deref(), Some("qobuz:album:A1"));
        assert_eq!(item.r#type, FILE_TYPE_PLAYLIST_BROWSABLE);
        assert_eq!(item.tag.as_deref(), Some("album"));
        assert_eq!(
            item.coverurl.as_deref(),
            Some("proxy:https://static.qobuz.com/a.jpg")
        );
        assert_eq!(item.thumbnail, item.coverurl);

        let track = json!({"type": "audio", "path": "qobuz:/track?id=T1", "title": "Come Together",
            "mediaData": {"metaData": {"artist": "The Beatles", "album": "Abbey Road"}}});
        let item = browse_item(&track, 2, &no_proxy);
        assert_eq!(item.r#type, FILE_TYPE_TRACK);
        assert_eq!(item.name, "Come Together — The Beatles");
        assert_eq!(item.title.as_deref(), Some("Come Together"));
        assert_eq!(item.album.as_deref(), Some("Abbey Road"));
        assert_eq!(item.coverurl, None);

        let playlist = json!({"type": "container", "path": "qobuz:/playlist?id=P1",
            "title": "Daily Mix", "containerType": "playlist"});
        let item = browse_item(&playlist, 3, &no_proxy);
        assert_eq!(item.content_type.as_deref(), Some("Playlists"));
        assert_eq!(item.r#type, FILE_TYPE_PLAYLIST_BROWSABLE);

        let artist = json!({"type": "container", "path": "qobuz:/artist?id=R1",
            "title": "ANOHNI", "containerType": "person"});
        let item = browse_item(&artist, 4, &no_proxy);
        assert_eq!(item.r#type, FILE_TYPE_SERVICE_ITEM);
        assert_eq!(item.audiopath, None);

        // A container that is not a catalogue item (favourite tracks) is a plain folder.
        let folder_row = json!({"type": "container", "path": "qobuz:/user/favourites/tracks",
            "title": "Tracks", "containerType": "playlist"});
        let item = browse_item(&folder_row, 5, &no_proxy);
        assert_eq!(item.r#type, FILE_TYPE_FOLDER);
        assert_eq!(
            speaker_path(&item.id).as_deref(),
            Some("qobuz:/user/favourites/tracks")
        );
    }

    #[test]
    fn browsed_items_are_remembered_for_playing_and_saving_by_id() {
        remember_rows(&[
            json!({"type": "audio", "path": "qobuz:/track?id=990001", "title": "Faith",
                "icon": "https://static.qobuz.com/c.jpg",
                "mediaData": {"metaData": {"artist": "The Cure", "album": "Faith"}}}),
            json!({"type": "container", "path": "qobuz:/album?id=990002", "title": "Pornography"}),
            json!({"type": "container", "path": "qobuz:/user/favourites/tracks", "title": "Tracks"}),
        ]);
        let info = known_item("spotify:track:990001").unwrap();
        assert_eq!(
            (info.title.as_str(), info.artist.as_str()),
            ("Faith", "The Cure")
        );
        assert_eq!(
            known_item("qobuz:album:990002").unwrap().title,
            "Pornography"
        );
        remember_playing(
            "qobuz:/track?id=990004",
            "Charlotte Sometimes",
            "The Cure",
            "",
            "",
        );
        assert_eq!(known_item("qobuz:track:990004").unwrap().artist, "The Cure");
        remember_playing(
            "qobuz:/track?id=990004",
            "Charlotte Sometimes",
            "The Cure",
            "",
            "",
        );
        remember_playing("vtuner:x", "Radio", "", "", "");
        remember_item("qobuz:album:990003", ItemInfo::default());
        assert_eq!(known_item("qobuz:album:990003"), None, "nothing to show");
    }

    #[test]
    fn proxied_covers_unwrap_to_the_original() {
        assert_eq!(
            unproxied_cover("http://h:7091/cors-proxy?url=https%3A%2F%2Fs.qobuz.com%2Fa.jpg&id=x"),
            "https://s.qobuz.com/a.jpg"
        );
        assert_eq!(
            unproxied_cover("https://s.qobuz.com/a.jpg"),
            "https://s.qobuz.com/a.jpg"
        );
    }

    #[test]
    fn track_audiopath_from_the_speaker_path() {
        assert_eq!(
            track_audiopath("qobuz:/track?id=2531160").as_deref(),
            Some("qobuz:track:2531160")
        );
        assert_eq!(track_audiopath("vtuner:abc"), None);
    }

    #[test]
    fn search_specs_limits_and_paths() {
        assert!(is_search_provider("spotify@qobuz:track#5,album#5"));
        assert!(is_search_provider("qobuz"));
        assert!(!is_search_provider("tunein:station#10"));
        assert_eq!(
            search_limits("spotify@qobuz:track#3,album#0,artist#2,show#5"),
            vec![("track", 3), ("artist", 2)]
        );
        assert_eq!(search_limits("spotify@qobuz").len(), 4);
        assert_eq!(
            search_path("album", "abbey road").as_deref(),
            Some("qobuz:/search/albums?query=abbey road")
        );
        assert_eq!(
            search_path("track", "rock & roll").as_deref(),
            Some("qobuz:/search/tracks?query=rock roll")
        );
        assert_eq!(
            search_path("track", "a=b#c?d+e").as_deref(),
            Some("qobuz:/search/tracks?query=a b c d e")
        );
        assert_eq!(search_path("show", "x"), None);
    }

    #[test]
    fn search_hits_carry_the_account_and_category_lists_use_spotify_ids() {
        let album = json!({"type": "container", "path": "qobuz:/album?id=A1",
            "title": "Abbey Road", "containerType": "album"});
        let hit = search_hit(browse_item(&album, 1, &no_proxy));
        assert_eq!(hit.r#type, FILE_TYPE_SERVICE_ITEM);
        assert_eq!(hit.uid.as_deref(), Some("spotify/qobuz"));
        assert_eq!(hit.user.as_deref(), Some("qobuz"));
        assert_eq!(hit.service.as_ref().unwrap()["user"], "qobuz");
        let listed = category_hit(browse_item(&album, 1, &no_proxy));
        assert_eq!(listed.id, "spotify:album:A1");
        assert_eq!(normalize_id(&listed.id).as_deref(), Some("qobuz:album:A1"));

        let track = json!({"type": "audio", "path": "qobuz:/track?id=T1", "title": "Something",
            "mediaData": {"metaData": {"artist": "The Beatles"}}});
        let hit = search_hit(browse_item(&track, 1, &no_proxy));
        assert_eq!(hit.r#type, FILE_TYPE_TRACK);
        assert_eq!(hit.name, "Something");
        assert_eq!(hit.uid, None);
    }

    #[test]
    fn service_entry_is_the_virtual_spotify_account() {
        let entry = service_entry();
        assert_eq!(entry["cmd"], "spotify");
        assert_eq!(entry["user"], "qobuz");
        assert_eq!(entry["uid"], "spotify/qobuz");
        assert_eq!(entry["fake"], true);
        assert!(entry.get("email").is_none());
    }
}
