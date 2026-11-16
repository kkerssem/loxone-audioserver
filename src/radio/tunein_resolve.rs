//! Resolve TuneIn / custom station ids to HTTPS stream URLs for Cast LOAD.

use anyhow::{Context, Result};

use super::audiopath::RadioAudiopath;
use super::custom_streams::CustomRadioStore;
use super::tunein_api::{
    content_type_for_url, pick_stream_link, station_metadata, station_stream_links,
};
use tracing::debug;

#[derive(Debug, Clone)]
pub struct ResolvedRadioStream {
    pub stream_url: String,
    pub title: String,
    pub cover_url: Option<String>,
    pub content_type: String,
    pub audiopath: String,
    pub is_custom: bool,
}

pub async fn resolve_radio_stream(path: &RadioAudiopath) -> Result<ResolvedRadioStream> {
    match path {
        RadioAudiopath::TuneInStation { id } => resolve_tunein_station(id).await,
        RadioAudiopath::CustomStation { id } => resolve_custom_station(id).await,
        RadioAudiopath::RawUrl(url) | RadioAudiopath::LegacyRadioUrl(url) => {
            Ok(ResolvedRadioStream {
                stream_url: url.clone(),
                title: String::new(),
                cover_url: None,
                content_type: content_type_for_url(url),
                audiopath: url.clone(),
                is_custom: url.contains("custom:station") || !url.starts_with("tunein:"),
            })
        }
    }
}

pub async fn resolve_radio_audiopath(audiopath: &str) -> Result<ResolvedRadioStream> {
    let path = super::audiopath::parse_radio_audiopath(audiopath)
        .with_context(|| format!("not a radio audiopath: {audiopath}"))?;
    resolve_radio_stream(&path).await
}

async fn resolve_tunein_station(station_id: &str) -> Result<ResolvedRadioStream> {
    let links = station_stream_links(station_id)
        .await
        .with_context(|| format!("TuneIn Tune.ashx({station_id})"))?;
    let best = pick_stream_link(&links).context("TuneIn station returned no stream links")?;

    let (title, cover_url) = match station_metadata(station_id).await {
        Ok(Some(meta)) => meta,
        Ok(None) => (station_id.to_string(), None),
        Err(e) => {
            debug!(error = %e, station_id, "tunein metadata fetch failed; using station id as title");
            (station_id.to_string(), None)
        }
    };

    let ap = super::audiopath::tunein_station_audiopath(station_id);
    Ok(ResolvedRadioStream {
        stream_url: best.url.clone(),
        title,
        cover_url,
        content_type: content_type_for_url(&best.url),
        audiopath: ap,
        is_custom: false,
    })
}

async fn resolve_custom_station(id: &str) -> Result<ResolvedRadioStream> {
    let store = CustomRadioStore::new(None);
    let station = store
        .get(id)
        .await?
        .with_context(|| format!("custom station not found: {id}"))?;
    Ok(ResolvedRadioStream {
        stream_url: station.url.clone(),
        title: station.name.clone(),
        cover_url: station.coverurl.clone(),
        content_type: content_type_for_url(&station.url),
        audiopath: super::audiopath::custom_station_audiopath(id),
        is_custom: true,
    })
}

#[cfg(test)]
mod tests {
    use crate::radio::tunein_api::{pick_stream_link, StreamLink};

    #[test]
    fn resolve_picks_direct_stream() {
        let links = vec![
            StreamLink {
                bitrate: 96,
                element: "audio".into(),
                is_direct: false,
                is_hls_advanced: "true".into(),
                media_type: "application/vnd.apple.mpegurl".into(),
                reliability: 95,
                url: "https://example/hls.m3u8".into(),
            },
            StreamLink {
                bitrate: 128,
                element: "audio".into(),
                is_direct: true,
                is_hls_advanced: "false".into(),
                media_type: "audio/mpeg".into(),
                reliability: 90,
                url: "https://example/stream.mp3".into(),
            },
        ];
        let picked = pick_stream_link(&links).unwrap();
        assert_eq!(picked.url, "https://example/stream.mp3");
    }
}
