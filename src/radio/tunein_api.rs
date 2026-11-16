//! In-tree TuneIn OPML client (replaces the `tunein` crate).

#![allow(dead_code)] // API response DTOs include fields we deserialize but do not read.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::sync::OnceLock;
use std::time::Duration;

const BASE: &str = "https://opml.radiotime.com";

static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

fn client() -> &'static reqwest::Client {
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .expect("tunein reqwest client")
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TuneInCategory {
    ByLocation,
    ByLanguage,
    Sports,
    Talk,
    Music,
    LocalRadio,
    Podcasts,
}

impl TuneInCategory {
    pub fn category_id(self) -> &'static str {
        match self {
            Self::ByLocation => "r0",
            Self::ByLanguage => "lang",
            Self::Sports => "sports",
            Self::Talk => "talk",
            Self::Music => "music",
            Self::LocalRadio => "local",
            Self::Podcasts => "podcast",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::ByLocation => "By Location",
            Self::ByLanguage => "By Language",
            Self::Sports => "Sports",
            Self::Talk => "Talk",
            Self::Music => "Music",
            Self::LocalRadio => "Local Radio",
            Self::Podcasts => "Podcasts",
        }
    }

    pub fn from_folder_name(folder: &str) -> Option<Self> {
        match folder.to_ascii_lowercase().as_str() {
            "local" => Some(Self::LocalRadio),
            "music" => Some(Self::Music),
            "talk" => Some(Self::Talk),
            "sports" => Some(Self::Sports),
            "podcast" | "podcasts" => Some(Self::Podcasts),
            "language" | "lang" => Some(Self::ByLanguage),
            _ => None,
        }
    }
}

/// Shared row shape for browse outlines, search hits, and nested children.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TuneInRow {
    #[serde(default)]
    pub element: String,
    pub text: String,
    #[serde(rename = "URL", default)]
    pub url: Option<String>,
    #[serde(default)]
    pub guide_id: Option<String>,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub item: Option<String>,
    #[serde(default)]
    pub r#type: Option<String>,
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub children: Option<Vec<TuneInRow>>,
}

/// Root browse category row (Browse.ashx?render=json).
#[derive(Debug, Clone, Deserialize)]
pub struct CategoryRow {
    #[serde(default)]
    pub r#type: Option<String>,
    pub text: String,
    #[serde(rename = "URL", default)]
    pub url: Option<String>,
    #[serde(default)]
    pub guide_id: Option<String>,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub element: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StreamLink {
    #[serde(default)]
    pub bitrate: u32,
    pub element: String,
    #[serde(default)]
    pub is_direct: bool,
    #[serde(default)]
    pub is_hls_advanced: String,
    #[serde(default)]
    pub media_type: String,
    pub reliability: u32,
    pub url: String,
}

#[derive(Debug, Deserialize)]
struct TuneInHead {
    #[serde(default)]
    title: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BrowseRowsResponse {
    head: TuneInHead,
    body: Vec<TuneInRow>,
}

#[derive(Debug, Deserialize)]
struct CategoryRowsResponse {
    head: TuneInHead,
    body: Vec<CategoryRow>,
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    head: TuneInHead,
    body: Vec<TuneInRow>,
}

#[derive(Debug, Deserialize)]
struct TuneResponse {
    head: TuneInHead,
    body: Vec<StreamLink>,
}

async fn get_json<T: for<'de> Deserialize<'de>>(path: &str) -> Result<T> {
    let url = format!("{BASE}/{path}");
    client()
        .get(&url)
        .send()
        .await
        .with_context(|| format!("TuneIn GET {path}"))?
        .error_for_status()
        .with_context(|| format!("TuneIn GET {path} HTTP error"))?
        .json::<T>()
        .await
        .with_context(|| format!("TuneIn GET {path} JSON"))
}

pub async fn browse_root_categories() -> Result<Vec<CategoryRow>> {
    let response: CategoryRowsResponse = get_json("Browse.ashx?render=json").await?;
    Ok(response.body)
}

pub async fn browse_by_category(cat: TuneInCategory) -> Result<(Option<String>, Vec<TuneInRow>)> {
    browse_rows(&format!(
        "Browse.ashx?c={}&render=json",
        urlencoding::encode(cat.category_id())
    ))
    .await
}

pub async fn browse_by_id(id: &str) -> Result<(Option<String>, Vec<TuneInRow>)> {
    browse_rows(&format!(
        "Browse.ashx?id={}&render=json",
        urlencoding::encode(id)
    ))
    .await
}

async fn browse_rows(path: &str) -> Result<(Option<String>, Vec<TuneInRow>)> {
    let response: BrowseRowsResponse = get_json(path).await?;
    Ok((response.head.title, response.body))
}

pub async fn search_stations(query: &str) -> Result<Vec<TuneInRow>> {
    let response: SearchResponse = get_json(&format!(
        "Search.ashx?query={}&render=json",
        urlencoding::encode(query)
    ))
    .await?;
    Ok(response.body)
}

pub async fn station_stream_links(station_id: &str) -> Result<Vec<StreamLink>> {
    let response: TuneResponse = get_json(&format!(
        "Tune.ashx?id={}&render=json",
        urlencoding::encode(station_id)
    ))
    .await?;
    Ok(response.body)
}

/// Station title and cover from Browse.ashx?id=.
///
/// Station pages (`id=s…`) put the name in `head.title`; `body` is recommendations
/// and nested folders, not a row for the station itself.
pub async fn station_metadata(station_id: &str) -> Result<Option<(String, Option<String>)>> {
    let (head_title, rows) = browse_by_id(station_id).await?;
    Ok(station_metadata_from_browse(
        head_title.as_deref(),
        &rows,
        station_id,
    ))
}

fn station_metadata_from_browse(
    head_title: Option<&str>,
    rows: &[TuneInRow],
    station_id: &str,
) -> Option<(String, Option<String>)> {
    if let Some((title, cover)) = find_station_row(rows, station_id) {
        return Some((title, cover));
    }
    let title = head_title.filter(|t| !t.is_empty())?.to_string();
    Some((title, default_station_logo_url(station_id)))
}

fn find_station_row(rows: &[TuneInRow], station_id: &str) -> Option<(String, Option<String>)> {
    for row in rows {
        if row.guide_id.as_deref() == Some(station_id) {
            return Some((row.text.clone(), row.image.clone()));
        }
        if let Some(children) = row.children.as_ref() {
            if let Some(found) = find_station_row(children, station_id) {
                return Some(found);
            }
        }
    }
    None
}

fn default_station_logo_url(station_id: &str) -> Option<String> {
    station_id
        .starts_with('s')
        .then(|| format!("http://cdn-radiotime-logos.tunein.com/{station_id}q.png"))
}

pub fn extract_id_from_tunein_url(url: Option<&str>) -> Option<String> {
    let url = url?;
    for prefix in ["id=", "c="] {
        if let Some(rest) = url.split(prefix).nth(1) {
            let id = rest.split('&').next()?.trim();
            if !id.is_empty() {
                return Some(id.to_string());
            }
        }
    }
    None
}

/// Query string for `Browse.ashx` from an outline URL, preserving `filter`, `pivot`, etc.
pub fn tunein_browse_query_from_url(url: &str) -> Option<String> {
    if !url.to_ascii_lowercase().contains("browse.ashx") {
        return None;
    }
    let query = url.split('?').nth(1)?;
    let mut parts = Vec::new();
    let mut has_browse_key = false;
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        if key.eq_ignore_ascii_case("render") {
            continue;
        }
        if key.eq_ignore_ascii_case("id") || key.eq_ignore_ascii_case("c") {
            has_browse_key = true;
        }
        parts.push(format!("{}={}", key, urlencoding::encode(value)));
    }
    has_browse_key.then(|| parts.join("&"))
}

/// Fetch a browse page using all parameters from an outline row URL.
pub async fn browse_by_outline_url(url: &str) -> Result<Option<(Option<String>, Vec<TuneInRow>)>> {
    let Some(query) = tunein_browse_query_from_url(url) else {
        return Ok(None);
    };
    browse_rows(&format!("Browse.ashx?{query}&render=json"))
        .await
        .map(Some)
}

pub fn is_hls_link(link: &StreamLink) -> bool {
    let lower = link.url.to_ascii_lowercase();
    lower.contains(".m3u8")
        || link.is_hls_advanced.eq_ignore_ascii_case("true")
        || link.media_type.to_ascii_lowercase().contains("mpegurl")
}

pub fn is_direct_progressive_link(link: &StreamLink) -> bool {
    if is_hls_link(link) {
        return false;
    }
    let lower = link.url.to_ascii_lowercase();
    lower.contains(".mp3")
        || lower.contains(".aac")
        || link.media_type.to_ascii_lowercase().contains("mpeg")
        || link.media_type.to_ascii_lowercase().contains("aac")
        || link.is_direct
}

/// Prefer direct MP3/AAC over HLS; within a bucket pick highest reliability.
pub fn pick_stream_link(links: &[StreamLink]) -> Option<&StreamLink> {
    if links.is_empty() {
        return None;
    }
    let direct: Vec<&StreamLink> = links
        .iter()
        .filter(|l| is_direct_progressive_link(l))
        .collect();
    let pool: Vec<&StreamLink> = if direct.is_empty() {
        links.iter().collect()
    } else {
        direct
    };
    pool.iter()
        .max_by_key(|l| l.reliability)
        .copied()
        .or_else(|| links.first())
}

pub fn content_type_for_url(url: &str) -> String {
    let lower = url.to_ascii_lowercase();
    if lower.contains(".aac") {
        "audio/aac".into()
    } else if lower.contains(".ogg") {
        "audio/ogg".into()
    } else if lower.contains(".m3u8") || lower.contains("hls") {
        "application/vnd.apple.mpegurl".into()
    } else {
        "audio/mpeg".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!("tests/fixtures/tunein/{name}"))
            .unwrap_or_else(|e| panic!("read fixture {name}: {e}"))
    }

    #[test]
    fn deserialize_browse_root_fixture() {
        let response: CategoryRowsResponse =
            serde_json::from_str(&fixture("browse_root.json")).expect("browse_root.json");
        assert!(!response.body.is_empty());
        assert!(response.body[0].text.len() > 0);
    }

    #[test]
    fn deserialize_search_fixture() {
        let response: SearchResponse =
            serde_json::from_str(&fixture("search.json")).expect("search.json");
        assert_eq!(response.body[0].element, "station");
    }

    #[test]
    fn deserialize_tune_fixture() {
        let response: TuneResponse =
            serde_json::from_str(&fixture("tune.json")).expect("tune.json");
        assert!(response.body.len() >= 2);
    }

    #[test]
    fn pick_stream_prefers_direct_over_hls() {
        let response: TuneResponse =
            serde_json::from_str(&fixture("tune.json")).expect("tune.json");
        let picked = pick_stream_link(&response.body).expect("pick");
        assert!(is_direct_progressive_link(picked));
        assert!(!is_hls_link(picked));
        assert_eq!(picked.reliability, 90);
    }

    #[test]
    fn station_metadata_from_flat_browse_row() {
        let response: BrowseRowsResponse =
            serde_json::from_str(&fixture("browse_station.json")).expect("browse_station.json");
        let meta =
            station_metadata_from_browse(response.head.title.as_deref(), &response.body, "s99")
                .expect("metadata");
        assert_eq!(meta.0, "Jazz FM");
        assert_eq!(meta.1.as_deref(), Some("https://cdn.example/jazz.png"));
    }

    #[test]
    fn station_metadata_uses_head_title_for_station_page_fixture() {
        let response: BrowseRowsResponse =
            serde_json::from_str(&fixture("browse_station_page.json"))
                .expect("browse_station_page.json");
        let meta =
            station_metadata_from_browse(response.head.title.as_deref(), &response.body, "s6707")
                .expect("metadata");
        assert_eq!(meta.0, "NPO 3FM");
        assert_eq!(
            meta.1.as_deref(),
            Some("http://cdn-radiotime-logos.tunein.com/s6707q.png")
        );
    }

    #[test]
    fn extract_id_from_url() {
        assert_eq!(
            extract_id_from_tunein_url(Some("http://opml.radiotime.com/Browse.ashx?id=r100385")),
            Some("r100385".into())
        );
    }

    #[test]
    fn tunein_browse_query_preserves_filter_and_pivot() {
        assert_eq!(
            tunein_browse_query_from_url(
                "http://opml.radiotime.com/Browse.ashx?id=r100385&filter=s:popular"
            ),
            Some("id=r100385&filter=s%3Apopular".into())
        );
        assert_eq!(
            tunein_browse_query_from_url(
                "http://opml.radiotime.com/Browse.ashx?id=r100385&pivot=genre&filter=country"
            ),
            Some("id=r100385&pivot=genre&filter=country".into())
        );
        assert_eq!(
            tunein_browse_query_from_url("http://opml.radiotime.com/Browse.ashx?c=local"),
            Some("c=local".into())
        );
        assert_eq!(
            tunein_browse_query_from_url("http://opml.radiotime.com/Tune.ashx?id=s99"),
            None
        );
    }

    #[test]
    fn category_ids() {
        assert_eq!(TuneInCategory::LocalRadio.category_id(), "local");
        assert_eq!(TuneInCategory::Music.category_id(), "music");
    }
}
