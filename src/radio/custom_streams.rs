//! Persisted custom radio stream URLs (`custom` service).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::fs;
use tracing::{info, warn};
use uuid::Uuid;

const DEFAULT_FILENAME: &str = "custom-radios.json";
const COVERS_DIR_NAME: &str = "custom-radio-covers";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomRadioStation {
    pub id: String,
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub coverurl: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Store {
    stations: Vec<CustomRadioStation>,
}

pub struct CustomRadioStore {
    path: PathBuf,
}

impl CustomRadioStore {
    pub fn new(path: Option<PathBuf>) -> Self {
        Self {
            path: path.unwrap_or_else(|| PathBuf::from(DEFAULT_FILENAME)),
        }
    }

    fn covers_dir(&self) -> PathBuf {
        self.path
            .parent()
            .map(|parent| parent.join(COVERS_DIR_NAME))
            .unwrap_or_else(|| PathBuf::from(COVERS_DIR_NAME))
    }

    pub async fn load(&self) -> Result<Vec<CustomRadioStation>> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let raw = fs::read_to_string(&self.path)
            .await
            .context("read custom radios")?;
        let store: Store = serde_json::from_str(&raw).context("parse custom radios")?;
        Ok(store.stations)
    }

    async fn save(&self, stations: &[CustomRadioStation]) -> Result<()> {
        let store = Store {
            stations: stations.to_vec(),
        };
        let raw = serde_json::to_string_pretty(&store)?;
        fs::write(&self.path, raw)
            .await
            .context("write custom radios")?;
        Ok(())
    }

    pub async fn list(&self) -> Result<Vec<CustomRadioStation>> {
        self.load().await
    }

    pub async fn add(&self, name: &str, url: &str) -> Result<CustomRadioStation> {
        validate_stream_url(url)?;
        let mut stations = self.load().await?;
        let station = CustomRadioStation {
            id: Uuid::new_v4().to_string(),
            name: name.to_string(),
            url: url.to_string(),
            coverurl: None,
        };
        stations.push(station.clone());
        self.save(&stations).await?;
        info!(station_id = %station.id, name = %station.name, "custom radio station added");
        Ok(station)
    }

    pub async fn delete(&self, id: &str) -> Result<bool> {
        let mut stations = self.load().await?;
        let before = stations.len();
        stations.retain(|s| s.id != id);
        if stations.len() == before {
            return Ok(false);
        }
        self.save(&stations).await?;
        self.remove_cover_files(id).await?;
        info!(station_id = %id, "custom radio station deleted");
        Ok(true)
    }

    pub async fn modify(
        &self,
        id: &str,
        name: &str,
        url: &str,
    ) -> Result<Option<CustomRadioStation>> {
        validate_stream_url(url)?;
        let mut stations = self.load().await?;
        let mut found = None;
        for s in &mut stations {
            if s.id == id {
                s.name = name.to_string();
                s.url = url.to_string();
                found = Some(s.clone());
                break;
            }
        }
        if let Some(station) = found {
            self.save(&stations).await?;
            info!(station_id = %id, name = %station.name, "custom radio station modified");
            Ok(Some(station))
        } else {
            Ok(None)
        }
    }

    pub async fn get(&self, id: &str) -> Result<Option<CustomRadioStation>> {
        Ok(self.load().await?.into_iter().find(|s| s.id == id))
    }

    /// Store cover image bytes and persist a public `coverurl` for browse/UI.
    pub async fn set_cover(
        &self,
        id: &str,
        data: &[u8],
        content_type: &str,
        cover_base: &str,
    ) -> Result<Option<CustomRadioStation>> {
        if data.is_empty() {
            anyhow::bail!("empty cover payload");
        }
        let mut stations = self.load().await?;
        let Some(idx) = stations.iter().position(|s| s.id == id) else {
            return Ok(None);
        };

        let ext = extension_for_content_type(content_type);
        let covers_dir = self.covers_dir();
        fs::create_dir_all(&covers_dir)
            .await
            .context("create custom radio covers dir")?;
        self.remove_cover_files(id).await?;

        let file_path = covers_dir.join(format!("{id}.{ext}"));
        fs::write(&file_path, data)
            .await
            .with_context(|| format!("write cover {}", file_path.display()))?;

        let version = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let coverurl = cover_public_url(cover_base, id, version);
        stations[idx].coverurl = Some(coverurl);
        let station = stations[idx].clone();
        self.save(&stations).await?;
        Ok(Some(station))
    }

    /// Read stored cover bytes and MIME type for HTTP GET.
    pub async fn read_cover(&self, id: &str) -> Result<Option<(Vec<u8>, &'static str)>> {
        let Some(path) = self.find_cover_path(id).await? else {
            return Ok(None);
        };
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("jpg");
        let content_type = content_type_for_extension(ext);
        let bytes = fs::read(&path)
            .await
            .with_context(|| format!("read cover {}", path.display()))?;
        Ok(Some((bytes, content_type)))
    }

    async fn find_cover_path(&self, id: &str) -> Result<Option<PathBuf>> {
        let covers_dir = self.covers_dir();
        if !covers_dir.is_dir() {
            return Ok(None);
        }
        let mut entries = fs::read_dir(&covers_dir)
            .await
            .with_context(|| format!("read covers dir {}", covers_dir.display()))?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .is_some_and(|stem| stem == id)
            {
                return Ok(Some(path));
            }
        }
        Ok(None)
    }

    async fn remove_cover_files(&self, id: &str) -> Result<()> {
        let covers_dir = self.covers_dir();
        if !covers_dir.is_dir() {
            return Ok(());
        }
        let mut entries = fs::read_dir(&covers_dir)
            .await
            .with_context(|| format!("read covers dir {}", covers_dir.display()))?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .is_some_and(|stem| stem == id)
            {
                let _ = fs::remove_file(&path).await;
            }
        }
        Ok(())
    }
}

/// Public URL the Loxone UI loads for custom radio cover art.
///
/// `version` busts the client cover cache when the image bytes change.
pub fn cover_public_url(cover_base: &str, id: &str, version: u128) -> String {
    format!(
        "{}/audio/cfg/radios/coverurl/{}?v={version}",
        cover_base.trim_end_matches('/'),
        urlencoding::encode(id)
    )
}

fn extension_for_content_type(content_type: &str) -> &'static str {
    let ct = content_type
        .split(';')
        .next()
        .unwrap_or(content_type)
        .trim();
    match ct {
        "image/png" => "png",
        "image/webp" => "webp",
        "image/gif" => "gif",
        _ => "jpg",
    }
}

fn content_type_for_extension(ext: &str) -> &'static str {
    match ext.to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "webp" => "image/webp",
        "gif" => "image/gif",
        _ => "image/jpeg",
    }
}

pub fn validate_stream_url(url: &str) -> Result<()> {
    let url = url.trim();
    if url.starts_with("https://") || url.starts_with("http://") {
        Ok(())
    } else {
        warn!(url, "custom radio stream URL must be http or https");
        anyhow::bail!("stream URL must be http or https")
    }
}

/// V2 `CustomRadioError` codes (comps.js).
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum CustomRadioError {
    None = 0,
    Other = 1,
    InvalidUrl = -1,
    FileCreationFailed = -2,
    FileNotFound = -3,
    MissingPayload = -4,
    ServerTimeout = -5,
    AuthenticationFailed = -6,
}

impl CustomRadioError {
    pub fn code(self) -> i32 {
        self as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use tokio::sync::OnceCell;

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    async fn temp_store() -> CustomRadioStore {
        static ROOT: OnceCell<PathBuf> = OnceCell::const_new();
        let root = ROOT
            .get_or_init(|| async { std::env::temp_dir().join("loxone-custom-radio-tests") })
            .await
            .clone();
        let n = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = root.join(format!("case-{n}"));
        let _ = fs::remove_dir_all(&dir).await;
        fs::create_dir_all(&dir).await.unwrap();
        CustomRadioStore::new(Some(dir.join("stations.json")))
    }

    #[test]
    fn validate_url_rejects_non_http() {
        assert!(validate_stream_url("ftp://x").is_err());
        assert!(validate_stream_url("https://stream.example/live").is_ok());
    }

    #[test]
    fn cover_public_url_encodes_id_and_versions() {
        let url = cover_public_url("http://192.168.1.10:7091", "abc-def", 42);
        assert_eq!(
            url,
            "http://192.168.1.10:7091/audio/cfg/radios/coverurl/abc-def?v=42"
        );
    }

    #[tokio::test]
    async fn set_cover_persists_file_and_metadata() {
        let store = temp_store().await;
        let station = store
            .add("Test FM", "https://stream.example/live")
            .await
            .unwrap();
        let updated = store
            .set_cover(
                &station.id,
                b"\xff\xd8\xff fake jpeg",
                "image/jpeg",
                "http://host:7091",
            )
            .await
            .unwrap()
            .expect("station exists");
        let coverurl = updated.coverurl.expect("coverurl set");
        assert!(coverurl.starts_with(&format!(
            "http://host:7091/audio/cfg/radios/coverurl/{}?v=",
            station.id
        )));
        let (bytes, ct) = store.read_cover(&station.id).await.unwrap().unwrap();
        assert_eq!(bytes, b"\xff\xd8\xff fake jpeg");
        assert_eq!(ct, "image/jpeg");
    }

    #[tokio::test]
    async fn set_cover_unknown_station_returns_none() {
        let store = temp_store().await;
        let result = store
            .set_cover("missing", b"data", "image/png", "http://host:7091")
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn delete_removes_cover_file() {
        let store = temp_store().await;
        let station = store
            .add("Test FM", "https://stream.example/live")
            .await
            .unwrap();
        store
            .set_cover(&station.id, b"cover", "image/png", "http://host:7091")
            .await
            .unwrap();
        assert!(store.read_cover(&station.id).await.unwrap().is_some());
        assert!(store.delete(&station.id).await.unwrap());
        assert!(store.read_cover(&station.id).await.unwrap().is_none());
    }
}
