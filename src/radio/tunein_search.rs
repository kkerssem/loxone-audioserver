//! TuneIn global search for V2 `globalsearch` ticket flow.

use anyhow::Result;
use serde_json::{json, Value};

use crate::remote::{
    BrowseResult, BrowseResultItem, GlobalSearchCategory, GlobalSearchResultBody,
    GlobalSearchResultInner, GlobalSearchTopResults,
};

use super::audiopath::custom_station_audiopath;
use super::custom_streams::CustomRadioStore;
use super::search_cache::last_tunein_query;
use super::tunein_api::search_stations;
use super::tunein_browse::{paginate, search_result_to_item};
use tracing::warn;

pub const DRILL_DOWN_MAX: usize = 50;

#[derive(Debug, Default, Clone)]
pub struct TuneinSearchLimits {
    pub station: u32,
    pub custom: u32,
}

pub fn parse_tunein_provider_spec(spec: &str) -> TuneinSearchLimits {
    let mut limits = TuneinSearchLimits {
        station: 5,
        custom: 5,
    };
    let rest = spec
        .split('@')
        .next()
        .unwrap_or(spec)
        .split(':')
        .nth(1)
        .unwrap_or(spec);
    for part in rest.split(',') {
        if let Some((kind, count)) = part.split_once('#') {
            let count: u32 = count.parse().unwrap_or(5);
            match kind {
                "station" | "stations" => limits.station = count,
                "custom" => limits.custom = count,
                _ => {}
            }
        }
    }
    limits
}

pub async fn global_search_tunein(
    query: &str,
    limits: &TuneinSearchLimits,
) -> GlobalSearchResultBody {
    match global_search_tunein_inner(query, limits).await {
        Ok(body) => body,
        Err(e) => {
            warn!(error = %e, query, "tunein global search failed");
            GlobalSearchResultBody::empty_tunein()
        }
    }
}

/// Paged category drill-down for `audio/cfg/search/radio/{user}/{kind}/arr/{start}/{max}`.
pub async fn search_category_tunein(
    kind: &str,
    query_token: &str,
    start: usize,
    max_items: usize,
) -> BrowseResult {
    let Some(query) = resolve_search_query(query_token) else {
        return empty_radio_browse(kind, start);
    };
    if query.is_empty() || max_items == 0 {
        return empty_radio_browse(kind, start);
    }

    let collect_cap = max_items.min(DRILL_DOWN_MAX);

    match kind {
        "station" | "stations" => match search_stations(&query).await {
            Ok(results) => {
                let items: Vec<BrowseResultItem> = results
                    .iter()
                    .enumerate()
                    .filter_map(|(i, r)| search_result_to_item(r, i))
                    .take(collect_cap)
                    .collect();
                browse_from_items(kind, start, max_items, items)
            }
            Err(e) => {
                warn!(error = %e, query, kind, "tunein search category failed");
                empty_radio_browse(kind, start)
            }
        },
        "custom" => match filter_custom_items(&query, collect_cap).await {
            Ok(items) => browse_from_items(kind, start, max_items, items),
            Err(e) => {
                warn!(error = %e, query, kind, "custom search category failed");
                empty_radio_browse(kind, start)
            }
        },
        _ => empty_radio_browse(kind, start),
    }
}

/// V2 `SearchScheme` envelope for radio category drill-down (`where: "radio"`).
pub fn build_radio_search_wire_payload(browse: &BrowseResult, user: &str) -> Value {
    let items = serde_json::to_value(&browse.items).unwrap_or_else(|_| json!([]));
    json!([{
        "results": [{
            "where": "radio",
            "user": user,
            "tag": browse.id,
            "totalitems": browse.total_items,
            "items": items,
        }]
    }])
}

fn resolve_search_query(query_token: &str) -> Option<String> {
    if query_token == "arr" {
        last_tunein_query()
    } else {
        Some(query_token.to_string())
    }
}

fn browse_from_items(
    kind: &str,
    start: usize,
    max_items: usize,
    items: Vec<BrowseResultItem>,
) -> BrowseResult {
    let (page, total) = paginate(&items, start, max_items);
    BrowseResult {
        id: kind.to_string(),
        name: kind.to_string(),
        start,
        service: Some("radio".to_string()),
        total_items: total,
        items: page,
    }
}

fn empty_radio_browse(kind: &str, start: usize) -> BrowseResult {
    BrowseResult {
        id: kind.to_string(),
        name: kind.to_string(),
        start,
        service: Some("radio".to_string()),
        total_items: 0,
        items: vec![],
    }
}

async fn filter_custom_items(query: &str, limit: usize) -> Result<Vec<BrowseResultItem>> {
    let store = CustomRadioStore::new(None);
    let custom_all = store.list().await?;
    let q = query.to_ascii_lowercase();
    Ok(custom_all
        .iter()
        .filter(|s| s.name.to_ascii_lowercase().contains(&q))
        .take(limit)
        .enumerate()
        .map(|(i, s)| BrowseResultItem {
            id: s.id.clone(),
            name: s.name.clone(),
            audiopath: Some(custom_station_audiopath(&s.id)),
            station: Some(s.name.clone()),
            title: Some(s.name.clone()),
            coverurl: s.coverurl.clone(),
            thumbnail: s.coverurl.clone(),
            streamurl: Some(s.url.clone()),
            r#type: crate::remote::FILE_TYPE_TRACK,
            slot: i,
            content_type: Some("Playlists".into()),
            artist: None,
            album: None,
            followed: None,
            items: None,
            media_type: None,
            provider: None,
            cmd: None,
            user: None,
            uid: None,
            fake: None,
            service: None,
            tag: None,
        })
        .collect())
}

async fn global_search_tunein_inner(
    query: &str,
    limits: &TuneinSearchLimits,
) -> Result<GlobalSearchResultBody> {
    let results = search_stations(query).await?;

    let mut station_items: Vec<BrowseResultItem> = Vec::new();
    for (i, r) in results.iter().enumerate() {
        if let Some(item) = search_result_to_item(r, i) {
            station_items.push(item);
        }
        if station_items.len() >= limits.station as usize {
            break;
        }
    }

    let custom_items = filter_custom_items(query, limits.custom as usize).await?;

    let station_count = station_items.len();
    let custom_count = custom_items.len();

    let station_cat = if limits.station > 0 {
        Some(GlobalSearchCategory {
            caption: "Stations".into(),
            count: station_count,
            total_items: station_count,
            link: String::new(),
            items: station_items.clone(),
        })
    } else {
        None
    };

    let custom_cat = if limits.custom > 0 {
        Some(GlobalSearchCategory {
            caption: "Custom".into(),
            count: custom_count,
            total_items: custom_count,
            link: String::new(),
            items: custom_items,
        })
    } else {
        None
    };

    let topresults = if station_count > 0 {
        Some(GlobalSearchTopResults {
            caption: "Top Results".into(),
            count: station_count.min(5),
            tracks: station_items.into_iter().take(5).collect(),
            albums: vec![],
            artists: vec![],
            playlists: vec![],
        })
    } else {
        None
    };

    Ok(GlobalSearchResultBody {
        error: 0,
        result: Some(GlobalSearchResultInner {
            tracks: None,
            albums: None,
            artists: None,
            playlists: None,
            shows: None,
            episodes: None,
            station: station_cat,
            custom: custom_cat,
            topresults,
            user: "noUser".into(),
            service: None,
        }),
    })
}
#[cfg(test)]
mod tests {
    use super::super::search_cache::clear_tunein_query_cache;
    use super::*;
    use crate::remote::FILE_TYPE_TRACK;
    use serial_test::serial;

    #[test]
    fn radio_search_wire_payload_matches_v2_envelope() {
        let browse = BrowseResult {
            id: "station".into(),
            name: "station".into(),
            start: 0,
            service: Some("radio".into()),
            total_items: 1,
            items: vec![BrowseResultItem {
                slot: 0,
                id: "s99".into(),
                audiopath: Some("tunein:station:s99".into()),
                name: "Jazz FM".into(),
                station: Some("Jazz FM".into()),
                r#type: FILE_TYPE_TRACK,
                ..Default::default()
            }],
        };
        let payload = build_radio_search_wire_payload(&browse, "nouser");
        assert_eq!(payload[0]["results"][0]["where"], "radio");
        assert_eq!(payload[0]["results"][0]["tag"], "station");
        assert_eq!(payload[0]["results"][0]["totalitems"], 1);
    }

    #[tokio::test]
    #[serial]
    async fn search_category_arr_without_cache_returns_empty() {
        clear_tunein_query_cache();
        let result = search_category_tunein("station", "arr", 0, 50).await;
        assert_eq!(result.total_items, 0);
        assert!(result.items.is_empty());
    }

    #[test]
    fn browse_from_items_paginates() {
        let items: Vec<_> = (0..10)
            .map(|i| BrowseResultItem {
                slot: i,
                id: format!("s{i}"),
                name: format!("Station {i}"),
                ..Default::default()
            })
            .collect();
        let page = browse_from_items("station", 2, 3, items);
        assert_eq!(page.total_items, 10);
        assert_eq!(page.items.len(), 3);
        assert_eq!(page.items[0].slot, 2);
    }
}
