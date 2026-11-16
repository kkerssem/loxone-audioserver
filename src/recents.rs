//! Per-zone recently played list for `audio/{playerid}/recent` (Loxone media UI).

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::player_state::{Mode, PlayerState};

const MAX_RECENTS: usize = 5;
const RECENTS_DIR: &str = "recents";

#[cfg(test)]
static TEST_RECENTS_DIR: std::sync::LazyLock<std::sync::RwLock<Option<PathBuf>>> =
    std::sync::LazyLock::new(|| std::sync::RwLock::new(None));

fn recents_dir() -> PathBuf {
    #[cfg(test)]
    if let Ok(guard) = TEST_RECENTS_DIR.read() {
        if let Some(path) = guard.as_ref() {
            return path.clone();
        }
    }
    PathBuf::from(RECENTS_DIR)
}

/// One row in `recent_result.items` (Loxone `RecentlyPlayedBasedItemSchema`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RecentItem {
    pub audiopath: String,
    pub coverurl: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub owner: String,
    pub owner_id: String,
    pub service: String,
    #[serde(rename = "serviceType")]
    pub service_type: u8,
    #[serde(rename = "type")]
    pub item_type: u8,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct RecentStoreFile {
    pub ts: u64,
    pub items: Vec<RecentItem>,
}

fn recents_path(playerid: usize) -> PathBuf {
    recents_dir().join(format!("{playerid}.json"))
}

fn unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Load stored recents for a Loxone player id (empty if missing).
pub fn load(playerid: usize) -> RecentStoreFile {
    let path = recents_path(playerid);
    let Ok(data) = fs::read_to_string(path) else {
        return RecentStoreFile::default();
    };
    serde_json::from_str(&data).unwrap_or_default()
}

/// Load with optional pagination offset (`audio/.../recent/{start}`).
pub fn load_page(playerid: usize, start: usize) -> RecentStoreFile {
    let mut file = load(playerid);
    if start > 0 {
        if start >= file.items.len() {
            file.items.clear();
        } else {
            file.items = file.items.split_off(start);
        }
    }
    file
}

pub fn save(playerid: usize, file: &RecentStoreFile) -> std::io::Result<()> {
    let dir = recents_dir();
    fs::create_dir_all(&dir)?;
    let path = recents_path(playerid);
    let data = serde_json::to_string_pretty(file)?;
    fs::write(path, data)
}

/// Insert or move-to-front; cap at [`MAX_RECENTS`].
pub fn record(playerid: usize, entry: RecentItem) -> std::io::Result<()> {
    if entry.audiopath.is_empty() {
        return Ok(());
    }
    let mut file = load(playerid);
    file.items.retain(|i| i.audiopath != entry.audiopath);
    file.items.insert(0, entry);
    file.items.truncate(MAX_RECENTS);
    file.ts = unix_secs();
    save(playerid, &file)
}

/// Build a recent row from live Qobuz playback state.
pub fn recent_item_from_player_state(state: &PlayerState) -> Option<RecentItem> {
    if !matches!(state.mode, Mode::Play | Mode::Pause) {
        return None;
    }
    if state.audiopath.is_empty() || !state.audiopath.starts_with("qobuz:") {
        return None;
    }
    if state.title.is_empty() {
        return None;
    }
    Some(RecentItem {
        audiopath: state.audiopath.clone(),
        coverurl: state.cover_url.clone(),
        title: state.title.clone(),
        artist: state.artist.clone(),
        album: state.album.clone(),
        owner: "qobuz".into(),
        owner_id: "qobuz".into(),
        service: "spotify".into(),
        service_type: 3,
        item_type: 2,
    })
}

pub fn record_from_player_state(playerid: usize, state: &PlayerState) -> std::io::Result<()> {
    if let Some(entry) = recent_item_from_player_state(state) {
        record(playerid, entry)
    } else {
        Ok(())
    }
}

/// True when a new Qobuz track should be appended to recents (not rewind / position-only).
pub fn should_record_recent(prior: &PlayerState, state: &PlayerState) -> bool {
    if recent_item_from_player_state(state).is_none() {
        return false;
    }
    prior.audiopath != state.audiopath
}

#[cfg(test)]
fn set_test_recents_dir(path: Option<PathBuf>) {
    *TEST_RECENTS_DIR.write().expect("recents test dir lock") = path;
}

#[cfg(test)]
fn with_test_recents_dir<R>(f: impl FnOnce() -> R) -> R {
    static TEST_LOCK: std::sync::LazyLock<std::sync::Mutex<()>> =
        std::sync::LazyLock::new(|| std::sync::Mutex::new(()));
    let _guard = TEST_LOCK.lock().expect("recents test lock");
    let dir = std::env::temp_dir().join(format!(
        "loxone-recents-test-{}-{}",
        std::process::id(),
        unix_secs()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("recents test temp dir");
    set_test_recents_dir(Some(dir.clone()));
    let out = f();
    set_test_recents_dir(None);
    let _ = fs::remove_dir_all(&dir);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_item(path: &str, title: &str) -> RecentItem {
        RecentItem {
            audiopath: path.into(),
            coverurl: "http://example/cover.jpg".into(),
            title: title.into(),
            artist: "Artist".into(),
            album: "Album".into(),
            owner: "qobuz".into(),
            owner_id: "qobuz".into(),
            service: "spotify".into(),
            service_type: 3,
            item_type: 2,
        }
    }

    #[test]
    fn record_dedupes_and_caps_at_five() {
        with_test_recents_dir(|| {
            let id = 1;
            for i in 0..6 {
                record(
                    id,
                    sample_item(&format!("qobuz:track:{i}"), &format!("T{i}")),
                )
                .unwrap();
            }
            let file = load(id);
            assert_eq!(file.items.len(), 5);
            assert_eq!(file.items[0].audiopath, "qobuz:track:5");
            assert!(file.ts > 0);
        });
    }

    #[test]
    fn record_moves_duplicate_to_front() {
        with_test_recents_dir(|| {
            let id = 1;
            record(id, sample_item("qobuz:track:1", "A")).unwrap();
            record(id, sample_item("qobuz:track:2", "B")).unwrap();
            record(id, sample_item("qobuz:track:1", "A")).unwrap();
            let file = load(id);
            assert_eq!(file.items.len(), 2);
            assert_eq!(file.items[0].audiopath, "qobuz:track:1");
        });
    }

    #[test]
    fn load_page_skips_start() {
        with_test_recents_dir(|| {
            let id = 1;
            record(id, sample_item("qobuz:track:1", "A")).unwrap();
            record(id, sample_item("qobuz:track:2", "B")).unwrap();
            let page = load_page(id, 1);
            assert_eq!(page.items.len(), 1);
            assert_eq!(page.items[0].audiopath, "qobuz:track:1");
        });
    }

    #[test]
    fn should_record_on_audiopath_change_only() {
        let mut prior = PlayerState::default();
        prior.audiopath = "qobuz:track:1".into();
        prior.title = "A".into();
        prior.mode = Mode::Play;
        let mut next = prior.clone();
        next.audiopath = "qobuz:track:2".into();
        assert!(should_record_recent(&prior, &next));
        assert!(!should_record_recent(&next, &next));
    }

    #[test]
    fn recent_item_serializes_expected_keys() {
        let item = sample_item("qobuz:track:9", "Song");
        let v = serde_json::to_value(&item).unwrap();
        assert_eq!(v["service"], "spotify");
        assert_eq!(v["serviceType"], 3);
        assert_eq!(v["type"], 2);
        assert_eq!(v["owner"], "qobuz");
    }

    #[test]
    fn recent_fixture_matches_v2_row_shape() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/recent_result_qobuz.json"))
                .unwrap();
        let row = &fixture["recent_result"]["items"][0];
        assert_eq!(row["audiopath"], "qobuz:track:12345");
        assert_eq!(row["service"], "spotify");
        assert_eq!(row["serviceType"], 3);
        assert_eq!(row["type"], 2);
        let parsed: RecentItem = serde_json::from_value(row.clone()).unwrap();
        assert_eq!(parsed.service, "spotify");
        assert_eq!(parsed.item_type, 2);
    }
}
