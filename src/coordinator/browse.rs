use crate::{
    radio::{self, is_radio_service},
    remote::{BrowseResult, GlobalSearchResultBody},
};
use tracing::warn;

pub(crate) fn empty_browse(id: &str, start: usize) -> BrowseResult {
    BrowseResult {
        id: id.to_string(),
        items: vec![],
        name: String::new(),
        start,
        service: None,
        total_items: 0,
    }
}

pub(crate) async fn handle_get_media_folder(id: &str, start: usize) -> BrowseResult {
    // Local SD card / on-device library (`id` "0" = root) is not implemented.
    empty_browse(id, start)
}

pub(crate) async fn handle_get_service_folder(
    service: &str,
    folder_id: &str,
    start: usize,
    max_items: usize,
) -> BrowseResult {
    if !is_radio_service(service) {
        return empty_browse(folder_id, start);
    }
    match radio::browse_service_folder(service, folder_id, start, max_items).await {
        Ok(result) => result,
        Err(e) => {
            warn!(error = %e, service = %service, folder = %folder_id, "radio browse failed");
            empty_browse(folder_id, start)
        }
    }
}

pub(crate) async fn handle_global_search(
    provider_spec: &str,
    query: &str,
) -> GlobalSearchResultBody {
    let is_tunein = provider_spec
        .split('@')
        .next()
        .unwrap_or(provider_spec)
        .starts_with("tunein");
    if !is_tunein {
        return GlobalSearchResultBody::empty();
    }
    let limits = radio::parse_tunein_provider_spec(provider_spec);
    radio::remember_tunein_query(query);
    radio::global_search_tunein(query, &limits).await
}

pub(crate) async fn handle_search_category(
    service: &str,
    kind: &str,
    query: &str,
    start: usize,
    max_items: usize,
) -> BrowseResult {
    if service.eq_ignore_ascii_case("radio") {
        radio::search_category_tunein(kind, query, start, max_items).await
    } else {
        empty_browse(kind, start)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn get_media_folder_is_empty_library_root() {
        let result = handle_get_media_folder("0", 0).await;
        assert_eq!(result.id, "0");
        assert_eq!(result.start, 0);
        assert!(result.items.is_empty());
    }

    #[test]
    fn empty_browse_carries_id_and_start() {
        let result = empty_browse("library/1", 25);
        assert_eq!(result.id, "library/1");
        assert_eq!(result.start, 25);
        assert_eq!(result.total_items, 0);
    }

    #[tokio::test]
    async fn non_radio_browse_and_search_are_empty() {
        let folder = handle_get_service_folder("spotify", "start", 0, 50).await;
        assert!(folder.items.is_empty());
        let search = handle_search_category("spotify", "track", "x", 0, 50).await;
        assert!(search.items.is_empty());
        let global = handle_global_search("spotify@qobuz", "x").await;
        let inner = global.result.expect("empty search keeps result envelope");
        assert!(inner.tracks.is_none() && inner.station.is_none());
    }
}
