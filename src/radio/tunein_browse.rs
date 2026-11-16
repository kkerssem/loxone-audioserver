//! Map TuneIn API responses to Loxone browse wire items.

use anyhow::Result;

use crate::remote::{BrowseResult, BrowseResultItem, FILE_TYPE_FOLDER, FILE_TYPE_TRACK};

use super::audiopath::{
    custom_station_audiopath, station_id_from_tunein_row, tunein_station_audiopath,
};
use super::custom_streams::CustomRadioStore;
use super::tunein_api::{
    browse_by_category, browse_by_id, browse_by_outline_url, browse_root_categories,
    extract_id_from_tunein_url, CategoryRow, TuneInCategory, TuneInRow,
};

fn empty_browse(id: &str, name: &str, start: usize, service: &str) -> BrowseResult {
    BrowseResult {
        id: id.to_string(),
        name: name.to_string(),
        start,
        service: Some(service.to_string()),
        total_items: 0,
        items: vec![],
    }
}

fn category_folder_id(cat: &CategoryRow) -> String {
    if let Some(gid) = cat.guide_id.as_ref().filter(|g| !g.is_empty()) {
        return gid.clone();
    }
    if let Some(id) = extract_id_from_tunein_url(cat.url.as_deref()) {
        return id;
    }
    cat.key
        .clone()
        .filter(|k| !k.is_empty())
        .unwrap_or_else(|| cat.text.clone())
}

fn category_folder_item(cat: &CategoryRow, slot: usize) -> BrowseResultItem {
    let id = category_folder_id(cat);
    BrowseResultItem {
        id: id.clone(),
        name: cat.text.clone(),
        audiopath: Some(id),
        r#type: FILE_TYPE_FOLDER,
        slot,
        title: Some(cat.text.clone()),
        coverurl: None,
        artist: None,
        album: None,
        content_type: None,
        thumbnail: None,
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
        station: None,
        streamurl: None,
    }
}

fn is_playable_station(outline: &TuneInRow) -> bool {
    outline
        .guide_id
        .as_ref()
        .is_some_and(|g| g.starts_with('s'))
}

fn folder_id_for_outline(outline: &TuneInRow, parent_id: &str) -> String {
    if let Some(gid) = outline
        .guide_id
        .as_ref()
        .filter(|g| !g.is_empty() && !g.starts_with('s'))
    {
        return gid.clone();
    }
    if let Some(key) = outline.key.as_ref().filter(|k| !k.is_empty()) {
        return format!("{parent_id}:{key}");
    }
    if let Some(id) = extract_id_from_tunein_url(outline.url.as_deref()) {
        return id;
    }
    outline.text.clone()
}

fn folder_item(outline: &TuneInRow, parent_id: &str, slot: usize) -> BrowseResultItem {
    let id = folder_id_for_outline(outline, parent_id);
    BrowseResultItem {
        id: id.clone(),
        name: outline.text.clone(),
        audiopath: Some(id),
        r#type: FILE_TYPE_FOLDER,
        slot,
        title: Some(outline.text.clone()),
        coverurl: outline.image.clone(),
        thumbnail: outline.image.clone(),
        artist: None,
        album: None,
        content_type: None,
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
        station: None,
        streamurl: None,
    }
}

fn tunein_station_item(
    name: &str,
    station_id: &str,
    cover: Option<String>,
    slot: usize,
) -> BrowseResultItem {
    let ap = tunein_station_audiopath(station_id);
    BrowseResultItem {
        id: station_id.to_string(),
        name: name.to_string(),
        audiopath: Some(ap.clone()),
        station: Some(name.to_string()),
        title: Some(name.to_string()),
        coverurl: cover.clone(),
        thumbnail: cover,
        r#type: FILE_TYPE_TRACK,
        slot,
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
        streamurl: None,
    }
}

fn outline_to_browse_item(outline: &TuneInRow, parent_id: &str, slot: usize) -> BrowseResultItem {
    if is_playable_station(outline) {
        let sid = station_id_from_tunein_row(outline.guide_id.as_deref(), &outline.element);
        tunein_station_item(&outline.text, &sid, outline.image.clone(), slot)
    } else {
        folder_item(outline, parent_id, slot)
    }
}

fn search_result_station(item: &TuneInRow, slot: usize) -> Option<BrowseResultItem> {
    if item.element != "station" && !item.guide_id.as_deref().is_some_and(|g| g.starts_with('s')) {
        return None;
    }
    let sid = station_id_from_tunein_row(item.guide_id.as_deref(), &item.element);
    Some(tunein_station_item(
        &item.text,
        &sid,
        item.image.clone(),
        slot,
    ))
}

pub(crate) fn paginate<T: Clone>(items: &[T], start: usize, max: usize) -> (Vec<T>, usize) {
    let total = items.len();
    let page: Vec<T> = items.iter().skip(start).take(max).cloned().collect();
    (page, total)
}

fn is_invalid_tunein_response(title: Option<&str>, outlines: &[TuneInRow]) -> bool {
    title == Some("Invalid request")
        || outlines.is_empty() && title.is_some_and(|t| t.contains("Invalid"))
}

/// Bare folder keys emitted by older browse rows (without `{parent}:{key}` prefix).
fn parent_for_bare_subkey(service: &str, key: &str) -> Option<&'static str> {
    match (service, key) {
        ("local", "stations") | ("local", "localCountry") => Some("local"),
        ("tunein", "stations") | ("tunein", "localCountry") => Some("local"),
        _ => None,
    }
}

fn outlines_to_browse_result(
    folder_id: &str,
    folder_name: &str,
    service: &str,
    outlines: &[TuneInRow],
    start: usize,
    max_items: usize,
) -> BrowseResult {
    let items: Vec<BrowseResultItem> = outlines
        .iter()
        .enumerate()
        .map(|(i, o)| outline_to_browse_item(o, folder_id, i))
        .collect();
    let (page, total) = paginate(&items, start, max_items);
    BrowseResult {
        id: folder_id.to_string(),
        name: folder_name.to_string(),
        start,
        service: Some(service.to_string()),
        total_items: total,
        items: page,
    }
}

pub async fn browse_service_folder(
    service: &str,
    folder_id: &str,
    start: usize,
    max_items: usize,
) -> Result<BrowseResult> {
    let svc = service.to_ascii_lowercase();
    match svc.as_str() {
        "custom" => browse_custom(folder_id, start, max_items).await,
        "local" | "music" | "world" | "tunein" => {
            browse_tunein(&svc, folder_id, start, max_items).await
        }
        _ => Ok(empty_browse(folder_id, service, start, service)),
    }
}

async fn browse_custom(folder_id: &str, start: usize, max_items: usize) -> Result<BrowseResult> {
    let store = CustomRadioStore::new(None);
    let stations = store.list().await?;
    let items: Vec<BrowseResultItem> = stations
        .iter()
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
            r#type: FILE_TYPE_TRACK,
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
        .collect();
    let (page, total) = paginate(&items, start, max_items);
    Ok(BrowseResult {
        id: folder_id.to_string(),
        name: "Custom Streams".into(),
        start,
        service: Some("custom".into()),
        total_items: total,
        items: page,
    })
}

async fn browse_tunein(
    service: &str,
    folder_id: &str,
    start: usize,
    max_items: usize,
) -> Result<BrowseResult> {
    let folder = normalize_tunein_folder(folder_id);

    if folder == "start" || folder == "root" {
        let categories = browse_root_categories().await?;
        let items: Vec<BrowseResultItem> = categories
            .iter()
            .enumerate()
            .map(|(i, c)| category_folder_item(c, i))
            .collect();
        let (page, total) = paginate(&items, start, max_items);
        return Ok(BrowseResult {
            id: folder_id.to_string(),
            name: "TuneIn".into(),
            start,
            service: Some(service.to_string()),
            total_items: total,
            items: page,
        });
    }

    if let Some((parent, key)) = split_composite_folder_id(&folder) {
        return browse_tunein_subfolder(service, folder_id, parent, key, start, max_items).await;
    }

    if let Some(parent) = parent_for_bare_subkey(service, folder.as_str()) {
        return browse_tunein_subfolder(
            service,
            &format!("{parent}:{}", folder),
            parent,
            &folder,
            start,
            max_items,
        )
        .await;
    }

    if let Some(cat) = TuneInCategory::from_folder_name(&folder) {
        return browse_tunein_category(service, folder_id, cat, start, max_items).await;
    }

    let (title, outlines) = browse_by_id(&folder).await?;
    if is_invalid_tunein_response(title.as_deref(), &outlines) {
        return Ok(empty_browse(
            folder_id,
            title.as_deref().unwrap_or(&folder),
            start,
            service,
        ));
    }
    let folder_name = title.as_deref().unwrap_or(folder.as_str());
    Ok(outlines_to_browse_result(
        &folder,
        folder_name,
        service,
        &outlines,
        start,
        max_items,
    ))
}

async fn browse_tunein_category(
    service: &str,
    folder_id: &str,
    cat: TuneInCategory,
    start: usize,
    max_items: usize,
) -> Result<BrowseResult> {
    let (title, outlines) = browse_by_category(cat).await?;
    let name = title.as_deref().unwrap_or(cat.display_name());
    Ok(outlines_to_browse_result(
        folder_id, name, service, &outlines, start, max_items,
    ))
}

async fn browse_tunein_subfolder(
    service: &str,
    full_folder_id: &str,
    parent_id: &str,
    key: &str,
    start: usize,
    max_items: usize,
) -> Result<BrowseResult> {
    let (parent_title, parent_outlines) = outlines_for_folder(parent_id).await?;

    let Some(sub) = parent_outlines
        .iter()
        .find(|o| o.key.as_deref() == Some(key))
    else {
        return Ok(empty_browse(
            full_folder_id,
            parent_title.as_deref().unwrap_or(key),
            start,
            service,
        ));
    };

    if let Some(children) = sub.children.as_ref().filter(|c| !c.is_empty()) {
        return Ok(outlines_to_browse_result(
            full_folder_id,
            &sub.text,
            service,
            children,
            start,
            max_items,
        ));
    }

    if let Some(gid) = sub.guide_id.as_ref().filter(|g| !g.is_empty()) {
        let (sub_title, sub_outlines) = browse_by_id(gid).await?;
        let name = sub_title.as_deref().unwrap_or(&sub.text);
        return Ok(outlines_to_browse_result(
            full_folder_id,
            name,
            service,
            &sub_outlines,
            start,
            max_items,
        ));
    }
    if let Some(url) = sub.url.as_deref() {
        if let Some((sub_title, sub_outlines)) = browse_by_outline_url(url).await? {
            let name = sub_title.as_deref().unwrap_or(&sub.text);
            return Ok(outlines_to_browse_result(
                full_folder_id,
                name,
                service,
                &sub_outlines,
                start,
                max_items,
            ));
        }
    }

    Ok(empty_browse(full_folder_id, &sub.text, start, service))
}

/// Resolve a folder id (including nested `a:b:c` paths) to its child outline rows.
async fn outlines_for_folder(folder_id: &str) -> Result<(Option<String>, Vec<TuneInRow>)> {
    if let Some(cat) = TuneInCategory::from_folder_name(folder_id) {
        return browse_by_category(cat).await;
    }
    if let Some((parent_id, key)) = split_composite_folder_id(folder_id) {
        let (parent_title, parent_outlines) = Box::pin(outlines_for_folder(parent_id)).await?;
        let Some(sub) = parent_outlines
            .iter()
            .find(|o| o.key.as_deref() == Some(key))
        else {
            return Ok((parent_title, vec![]));
        };
        return outlines_for_sub_row(sub).await;
    }
    browse_by_id(folder_id).await
}

async fn outlines_for_sub_row(sub: &TuneInRow) -> Result<(Option<String>, Vec<TuneInRow>)> {
    if let Some(children) = sub.children.as_ref().filter(|c| !c.is_empty()) {
        return Ok((Some(sub.text.clone()), children.clone()));
    }
    if let Some(gid) = sub.guide_id.as_ref().filter(|g| !g.is_empty()) {
        return browse_by_id(gid).await;
    }
    if let Some(url) = sub.url.as_deref() {
        if let Some(result) = browse_by_outline_url(url).await? {
            return Ok(result);
        }
    }
    Ok((Some(sub.text.clone()), vec![]))
}

fn normalize_tunein_folder(folder_id: &str) -> String {
    match folder_id {
        "" | "start" | "root" | "noUser" | "nouser" => "start".to_string(),
        other => other.to_string(),
    }
}

/// Split composite browse ids (`parent:…:key`) on the last colon so nested
/// folders like `r100385:pivot:popular` resolve under `r100385:pivot`.
fn split_composite_folder_id(folder_id: &str) -> Option<(&str, &str)> {
    folder_id.rsplit_once(':')
}

pub fn search_result_to_item(item: &TuneInRow, slot: usize) -> Option<BrowseResultItem> {
    search_result_station(item, slot)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tunein_station_item_shape() {
        let item = tunein_station_item("Jazz FM", "s99", None, 0);
        assert_eq!(item.audiopath.as_deref(), Some("tunein:station:s99"));
        assert_eq!(item.r#type, FILE_TYPE_TRACK);
        assert_eq!(item.station.as_deref(), Some("Jazz FM"));
    }

    #[test]
    fn extract_id_from_url() {
        assert_eq!(
            extract_id_from_tunein_url(Some("http://opml.radiotime.com/Browse.ashx?id=r100385")),
            Some("r100385".into())
        );
        assert_eq!(
            extract_id_from_tunein_url(Some("http://opml.radiotime.com/Browse.ashx?c=local")),
            Some("local".into())
        );
    }

    #[test]
    fn outline_folder_vs_station() {
        let folder = TuneInRow {
            element: "outline".into(),
            text: "Local Stations (24)".into(),
            url: None,
            guide_id: None,
            key: Some("local".into()),
            item: None,
            r#type: None,
            image: None,
            children: Some(vec![]),
        };
        let item = outline_to_browse_item(&folder, "c57943", 0);
        assert_eq!(item.r#type, FILE_TYPE_FOLDER);
        assert_eq!(item.id, "c57943:local");
        assert_eq!(item.audiopath.as_deref(), Some("c57943:local"));

        let station = TuneInRow {
            element: "outline".into(),
            text: "538 Non-Stop".into(),
            url: Some("http://opml.radiotime.com/Tune.ashx?id=s252020".into()),
            guide_id: Some("s252020".into()),
            key: None,
            item: Some("station".into()),
            r#type: Some("audio".into()),
            image: None,
            children: None,
        };
        let item = outline_to_browse_item(&station, "c57943:local", 1);
        assert_eq!(item.r#type, FILE_TYPE_TRACK);
        assert_eq!(item.audiopath.as_deref(), Some("tunein:station:s252020"));
    }

    #[test]
    fn parent_for_bare_stations_key() {
        assert_eq!(parent_for_bare_subkey("local", "stations"), Some("local"));
        assert_eq!(parent_for_bare_subkey("local", "r100385"), None);
    }

    #[test]
    fn composite_folder_id_splits_on_last_colon() {
        assert_eq!(
            split_composite_folder_id("r100385:pivot:popular"),
            Some(("r100385:pivot", "popular"))
        );
        assert_eq!(
            split_composite_folder_id("local:localCountry"),
            Some(("local", "localCountry"))
        );
        assert_eq!(
            split_composite_folder_id("r100385:pivot"),
            Some(("r100385", "pivot"))
        );
        assert_eq!(split_composite_folder_id("r100385"), None);
    }

    #[test]
    fn category_folder_uses_url_id() {
        let cat = CategoryRow {
            r#type: None,
            text: "By Location".into(),
            url: Some("http://opml.radiotime.com/Browse.ashx?id=r0".into()),
            guide_id: None,
            key: Some("location".into()),
            element: None,
        };
        let item = category_folder_item(&cat, 0);
        assert_eq!(item.id, "r0");
        assert_eq!(item.r#type, FILE_TYPE_FOLDER);
    }
}
