//! Browse, radio search, play, queue, and EQ handlers.

use anyhow::Context;
use axum::extract::ws::Message;
use matchit::Params;
use serde_json::json;
use tokio::sync::oneshot;

use crate::{
    player_state::Playerid,
    wire_response::build_wire_result_response,
    zone_equalizer::{
        build_equalizer_legacy_result, build_geteq_result, build_seteq_all_result,
        build_seteq_single_result, get_bands, parse_settings_csv, set_band, set_bands,
    },
};

use super::super::{
    build_getqueue_response, Api, BrowseResult, GlobalSearchResultBody, IoCommand, IoEvent,
    PlayOpts,
};
use tracing::{debug, warn};

pub(crate) fn globalsearch_provider_type(provider_spec: &str) -> &str {
    provider_spec
        .split('@')
        .next()
        .unwrap_or("spotify")
        .split(':')
        .next()
        .unwrap_or("spotify")
}

pub(crate) fn build_globalsearch_immediate(command: &str, unique: &str) -> anyhow::Result<String> {
    build_wire_result_response(
        "globalsearch_result",
        json!({ "unique": unique }),
        Some(command),
        &[],
    )
}

pub(crate) fn build_globalsearch_broadcast(
    result: &GlobalSearchResultBody,
    provider_type: &str,
    unique: &str,
) -> anyhow::Result<String> {
    if provider_type.eq_ignore_ascii_case("tunein") {
        return build_globalsearch_tunein_broadcast(result, unique);
    }
    build_wire_result_response(
        "globalsearch_result",
        serde_json::to_value(result)?,
        None,
        &[("type", json!(provider_type)), ("unique", json!(unique))],
    )
}

/// TuneIn global search uses a flat envelope (no `error`/`result` wrapper).
/// Reference: lox-audioserver `notifier.ts` `notifyGlobalSearchResult` tunein branch.
pub(crate) fn build_globalsearch_tunein_broadcast(
    result: &GlobalSearchResultBody,
    unique: &str,
) -> anyhow::Result<String> {
    let inner = result.result.as_ref();
    let station = inner.and_then(|r| r.station.as_ref());
    let custom = inner.and_then(|r| r.custom.as_ref());
    let station_items = station.map(|c| c.items.as_slice()).unwrap_or(&[]);
    let custom_items = custom.map(|c| c.items.as_slice()).unwrap_or(&[]);
    let station_len = station_items.len();
    let custom_len = custom_items.len();

    let payload = json!({
        "station": {
            "caption": station.map(|c| c.caption.as_str()).unwrap_or("Stations"),
            "items": station_items,
            "link": "audio/cfg/search/radio/nouser/station/arr/0/50",
            "totalitems": station_len,
        },
        "custom": {
            "caption": custom.map(|c| c.caption.as_str()).unwrap_or("Custom"),
            "items": custom_items,
            "link": "audio/cfg/search/radio/nouser/custom/arr/0/50",
            "totalitems": custom_len,
        },
    });

    build_wire_result_response(
        "globalsearch_result",
        payload,
        None,
        &[("type", json!("tunein")), ("unique", json!(unique))],
    )
}

impl Api {
    pub(in crate::remote) async fn get_queue_paged(
        &self,
        command: &str,
        params: &Params<'_, '_>,
    ) -> anyhow::Result<String> {
        let playerid = params
            .get("playerid")
            .context("Missing parameter playerid")?
            .parse::<usize>()
            .map(Playerid)?;
        let start = params
            .get("start")
            .context("Missing parameter start")?
            .parse()?;
        build_getqueue_response(command, playerid, start)
    }

    pub(in crate::remote) async fn get_queue_legacy(
        &self,
        command: &str,
        params: &Params<'_, '_>,
    ) -> anyhow::Result<String> {
        let playerid = params
            .get("playerid")
            .context("Missing parameter playerid")?
            .parse::<usize>()
            .map(Playerid)?;
        build_getqueue_response(command, playerid, 0)
    }

    pub(in crate::remote) async fn get_service_folder(
        &self,
        command: &str,
        params: &Params<'_, '_>,
    ) -> anyhow::Result<String> {
        let service = params.get("service").context("Missing parameter service")?;
        let folder_id = params.get("folder_id").unwrap_or(service);
        let start: usize = params
            .get("start")
            .context("Missing parameter start")?
            .parse()?;
        let max_items: usize = params
            .get("max_items")
            .context("Missing parameter max_items")?
            .parse()?;
        let (sender, receiver) = oneshot::channel();
        self.sender
            .send(IoCommand::GetServiceFolder {
                service: service.to_string(),
                folder_id: folder_id.to_string(),
                start,
                max_items,
                sender,
            })
            .await?;
        let mut browse = receiver.await.unwrap_or_else(|_| BrowseResult {
            id: folder_id.to_string(),
            name: service.to_string(),
            start,
            service: Some(service.to_string()),
            total_items: 0,
            items: vec![],
        });
        if browse.service.is_none() {
            browse.service = Some(service.to_string());
        }
        build_wire_result_response(
            "getservicefolder_result",
            json!([browse]),
            Some(command),
            &[],
        )
    }

    pub(in crate::remote) fn global_search_describe(
        &self,
        command: &str,
    ) -> anyhow::Result<String> {
        /*
         * Command: audio/cfg/globalsearch/describe
         * Maps provider id → searchable entity types (lox-audioserver contentManager).
         */
        build_wire_result_response(
            "globalsearch_result",
            json!({
                "tunein": ["station", "custom"],
            }),
            Some(command),
            &[],
        )
    }

    pub(in crate::remote) async fn search_category(
        &self,
        command: &str,
        params: &Params<'_, '_>,
    ) -> anyhow::Result<String> {
        let service = params
            .get("service")
            .context("Missing parameter service")?
            .to_string();
        let kind = params
            .get("kind")
            .context("Missing parameter kind")?
            .to_string();
        let query_raw = params.get("query").context("Missing parameter query")?;
        let query = urlencoding::decode(query_raw)
            .map(|s| s.into_owned())
            .unwrap_or_else(|_| query_raw.to_string());
        let start: usize = params
            .get("start")
            .context("Missing parameter start")?
            .parse()?;
        let max_items: usize = params
            .get("max_items")
            .context("Missing parameter max_items")?
            .parse()?;
        let kind_tag = kind.clone();
        let service_tag = service.clone();
        let (sender, receiver) = oneshot::channel();
        self.sender
            .send(IoCommand::SearchCategory {
                service,
                kind,
                query,
                start,
                max_items,
                sender,
            })
            .await?;
        let browse = receiver.await.unwrap_or_else(|_| BrowseResult {
            id: kind_tag,
            items: vec![],
            name: String::new(),
            start,
            service: Some(service_tag.clone()),
            total_items: 0,
        });
        let user = params.get("user").unwrap_or("nouser").to_string();
        let payload = crate::radio::build_radio_search_wire_payload(&browse, &user);
        build_wire_result_response("search_result", payload, Some(command), &[])
    }

    pub(in crate::remote) async fn global_search(
        &self,
        command: &str,
        params: &Params<'_, '_>,
    ) -> anyhow::Result<String> {
        let provider_spec = params
            .get("provider_spec")
            .context("Missing parameter provider_spec")?
            .to_string();
        let query_raw = params.get("query").context("Missing parameter query")?;
        let query = urlencoding::decode(query_raw)
            .map(|s| s.into_owned())
            .unwrap_or_else(|_| query_raw.to_string());
        let unique = format!(
            "gs-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0)
        );
        let provider_type = globalsearch_provider_type(&provider_spec).to_string();
        let io_sender = self.sender.clone();
        let outbound = self.async_outbound.clone();
        let unique_for_task = unique.clone();
        tokio::spawn(async move {
            let (sender, receiver) = oneshot::channel();
            if io_sender
                .send(IoCommand::GlobalSearch {
                    provider_spec,
                    query,
                    sender,
                })
                .await
                .is_err()
            {
                return;
            }
            let result = receiver
                .await
                .unwrap_or_else(|_| GlobalSearchResultBody::empty());
            let broadcast = build_globalsearch_broadcast(&result, &provider_type, &unique_for_task)
                .unwrap_or_else(|e| {
                    warn!(error = %e, "failed to encode globalsearch broadcast");
                    String::new()
                });
            if !broadcast.is_empty() {
                debug!(
                    provider_type,
                    unique = %unique_for_task,
                    broadcast,
                    "globalsearch broadcast"
                );
                if outbound.send(Message::from(broadcast)).is_err() {
                    debug!("globalsearch broadcast dropped: client disconnected");
                }
            }
        });
        build_globalsearch_immediate(command, &unique)
    }

    pub(in crate::remote) async fn play_from_browse_path(
        &self,
        command: &str,
        playerid: Playerid,
        rest: &str,
        result_name: &str,
    ) -> anyhow::Result<String> {
        let Some(id) = crate::radio::parse_serviceplay_item_id(rest) else {
            if let Some(id) = crate::qobuz::play_target(rest) {
                self.sender
                    .send(IoCommand::PlayServiceItem {
                        player_id: playerid,
                        id,
                    })
                    .await?;
            } else {
                debug!(path = rest, "play ignored: not a radio or qobuz item");
            }
            return build_wire_result_response(result_name, json!([]), Some(command), &[]);
        };
        let opts = PlayOpts {
            id,
            session_token: self.session_token.clone(),
        };
        self.sender
            .send(IoCommand::Play(playerid, Some(opts)))
            .await?;
        build_wire_result_response(result_name, json!([]), Some(command), &[])
    }

    /// `audio/{playerid}/library/play/{id}[/…]`: the App's play button on a service item.
    pub(in crate::remote) async fn library_play(
        &self,
        command: &str,
        params: &Params<'_, '_>,
    ) -> anyhow::Result<String> {
        let playerid: Playerid = params
            .get("playerid")
            .context("Missing parameter playerid")?
            .parse()?;
        let id = params.get("id").context("Missing parameter id")?;
        // A track inside an album or playlist (`parentid`) plays that container.
        let target = params
            .get("parentid")
            .and_then(crate::qobuz::play_target)
            .or_else(|| crate::qobuz::play_target(id));
        match target {
            Some(id) => {
                self.sender
                    .send(IoCommand::PlayServiceItem {
                        player_id: playerid,
                        id,
                    })
                    .await?
            }
            None => debug!(id, "library play ignored: not a qobuz item"),
        }
        build_wire_result_response("play_result", json!([]), Some(command), &[])
    }

    pub(in crate::remote) async fn service_play(
        &self,
        command: &str,
        params: &Params<'_, '_>,
    ) -> anyhow::Result<String> {
        let playerid: Playerid = params
            .get("playerid")
            .context("Missing parameter playerid")?
            .parse()?;
        let rest = params.get("rest").context("Missing parameter rest")?;
        self.play_from_browse_path(command, playerid, rest, "serviceplay_result")
            .await
    }

    /// `audio/grouped/playuploadedfile/{file}/{zones}` — sent right after the App uploaded a
    /// voice recording (`PUT /audio/cfg/upload/audioupload/add/{file}`). Playback runs in the
    /// background; the reply only acknowledges the command.
    pub(in crate::remote) async fn play_uploaded_file(
        &self,
        command: &str,
        params: &Params<'_, '_>,
    ) -> anyhow::Result<String> {
        // Decoded like the upload's path segment (axum `Path`), so both name the same file.
        let raw = params.get("file").context("Missing parameter file")?;
        let file = urlencoding::decode(raw)
            .map(|s| s.into_owned())
            .unwrap_or_else(|_| raw.to_string());
        let zones: Vec<Playerid> = crate::voice_upload::parse_zone_list(
            params.get("zones").context("Missing parameter zones")?,
        )
        .into_iter()
        .map(Playerid)
        .collect();
        if zones.is_empty() {
            debug!(command, "voice message ignored: no zones");
        } else {
            self.sender
                .send(IoCommand::PlayVoiceMessage { file, zones })
                .await?;
        }
        build_wire_result_response("playuploadedfile_result", json!([]), Some(command), &[])
    }

    pub(in crate::remote) async fn play_url(
        &self,
        command: &str,
        params: &Params<'_, '_>,
    ) -> anyhow::Result<String> {
        let playerid: Playerid = params
            .get("playerid")
            .context("Missing parameter playerid")?
            .parse()?;
        let rest = params.get("rest").context("Missing parameter rest")?;
        self.play_from_browse_path(command, playerid, rest, "play_result")
            .await
    }

    pub(in crate::remote) fn recent(
        &self,
        command: &str,
        params: &Params,
    ) -> anyhow::Result<String> {
        /*
         * Command: audio/{playerid}/recent
         * Command: audio/{playerid}/recent/{start}
         */
        let playerid: usize = params
            .get("playerid")
            .context("Missing parameter playerid")?
            .parse()?;
        let start = params
            .get("start")
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(0);
        let file = crate::recents::load_page(playerid, start);
        build_wire_result_response(
            "recent_result",
            serde_json::to_value(&file)?,
            Some(command),
            &[],
        )
    }

    pub(in crate::remote) fn get_eq(
        &self,
        command: &str,
        params: &Params<'_, '_>,
    ) -> anyhow::Result<String> {
        let playerid: Playerid = params
            .get("playerid")
            .context("Missing parameter playerid")?
            .parse()?;
        let bands = get_bands(playerid);
        build_wire_result_response(
            "geteq_result",
            serde_json::to_value(build_geteq_result(&bands))?,
            Some(command),
            &[],
        )
    }

    pub(in crate::remote) fn set_eq(
        &self,
        command: &str,
        params: &Params<'_, '_>,
    ) -> anyhow::Result<String> {
        let playerid: Playerid = params
            .get("playerid")
            .context("Missing parameter playerid")?
            .parse()?;
        let rest = params.get("rest").context("Missing parameter rest")?;

        let (bands, set_all) = if rest.contains(',') {
            let bands = parse_settings_csv(rest)?;
            set_bands(playerid, bands);
            (bands, true)
        } else if let Some((band_s, val_s)) = rest.split_once('/') {
            let band_id: usize = band_s.parse().context("invalid seteq band id")?;
            let value: f32 = val_s.parse().context("invalid seteq value")?;
            (set_band(playerid, band_id, value)?, false)
        } else {
            anyhow::bail!("invalid seteq path tail: {rest}");
        };

        let payload = if set_all {
            build_seteq_all_result(&bands)
        } else {
            build_seteq_single_result(playerid, &bands)
        };
        build_wire_result_response("seteq_result", payload, Some(command), &[])
    }

    pub(in crate::remote) fn get_equalizer_legacy(
        &self,
        command: &str,
        params: &Params<'_, '_>,
    ) -> anyhow::Result<String> {
        let playerid: Playerid = params
            .get("playerid")
            .context("Missing parameter playerid")?
            .parse()?;
        let bands = get_bands(playerid);
        build_wire_result_response(
            "equalizer_result",
            build_equalizer_legacy_result(playerid, &bands),
            Some(command),
            &[],
        )
    }

    pub(in crate::remote) fn set_equalizer_legacy(
        &self,
        command: &str,
        params: &Params<'_, '_>,
    ) -> anyhow::Result<String> {
        let playerid: Playerid = params
            .get("playerid")
            .context("Missing parameter playerid")?
            .parse()?;
        let rest = params.get("rest").context("Missing parameter rest")?;
        let bands = parse_settings_csv(rest)?;
        set_bands(playerid, bands);
        build_wire_result_response(
            "equalizer_result",
            build_equalizer_legacy_result(playerid, &bands),
            Some(command),
            &[],
        )
    }

    pub(in crate::remote) async fn radio_add(
        &self,
        command: &str,
        params: &Params<'_, '_>,
    ) -> anyhow::Result<String> {
        use crate::radio::{CustomRadioError, CustomRadioStore};
        let name = params.get("name").context("Missing name")?;
        let rest = params.get("rest").context("Missing url")?;
        let url = rest.split("/?q&").next().unwrap_or(rest);
        let name = urlencoding::decode(name)
            .map(|s| s.into_owned())
            .unwrap_or_else(|_| name.to_string());
        let url = urlencoding::decode(url)
            .map(|s| s.into_owned())
            .unwrap_or_else(|_| url.to_string());
        let store = CustomRadioStore::new(None);
        let payload = match store.add(&name, &url).await {
            Ok(station) => json!({
                "action": "add",
                "id": station.id,
                "name": station.name,
                "url": station.url,
                "error": CustomRadioError::None.code(),
            }),
            Err(e) => {
                warn!(error = %e, "radio add failed");
                json!({
                    "action": "add",
                    "name": name,
                    "url": url,
                    "error": CustomRadioError::InvalidUrl.code(),
                    "errorInfo": e.to_string(),
                })
            }
        };
        let _ = self.events.send(IoEvent::CustomUrlChanged);
        build_wire_result_response("radios_result", payload, Some(command), &[])
    }

    pub(in crate::remote) async fn radio_del(
        &self,
        command: &str,
        params: &Params<'_, '_>,
    ) -> anyhow::Result<String> {
        use crate::radio::{CustomRadioError, CustomRadioStore};
        let id = params.get("id").context("Missing id")?;
        let store = CustomRadioStore::new(None);
        let error = if store.delete(id).await.unwrap_or(false) {
            CustomRadioError::None.code()
        } else {
            CustomRadioError::FileNotFound.code()
        };
        let payload = json!({ "action": "del", "id": id, "error": error });
        let _ = self.events.send(IoEvent::CustomUrlChanged);
        build_wire_result_response("radios_result", payload, Some(command), &[])
    }

    pub(in crate::remote) async fn radio_modify(
        &self,
        command: &str,
        params: &Params<'_, '_>,
    ) -> anyhow::Result<String> {
        use crate::radio::{CustomRadioError, CustomRadioStore};
        let id = params.get("id").context("Missing id")?;
        let name = params.get("name").context("Missing name")?;
        let rest = params.get("rest").context("Missing url")?;
        let url = rest.split("/?q&").next().unwrap_or(rest);
        let name = urlencoding::decode(name)
            .map(|s| s.into_owned())
            .unwrap_or_else(|_| name.to_string());
        let url = urlencoding::decode(url)
            .map(|s| s.into_owned())
            .unwrap_or_else(|_| url.to_string());
        let store = CustomRadioStore::new(None);
        let payload = match store.modify(id, &name, &url).await {
            Ok(Some(station)) => json!({
                "action": "modify",
                "id": station.id,
                "name": station.name,
                "url": station.url,
                "error": CustomRadioError::None.code(),
            }),
            Ok(None) => json!({
                "action": "modify",
                "id": id,
                "error": CustomRadioError::FileNotFound.code(),
            }),
            Err(e) => json!({
                "action": "modify",
                "id": id,
                "name": name,
                "url": url,
                "error": CustomRadioError::InvalidUrl.code(),
                "errorInfo": e.to_string(),
            }),
        };
        let _ = self.events.send(IoEvent::CustomUrlChanged);
        build_wire_result_response("radios_result", payload, Some(command), &[])
    }
}
