//! Room favorite wire handlers (`audio/cfg/roomfavs`, `audio/{playerid}/roomfav/*`).

use anyhow::Context;
use matchit::Params;
use serde_json::json;
use tokio::sync::oneshot;

use crate::player_state::Playerid;
use crate::remote::{IoCommand, IoEvent, PlayOpts, RoomFavsResult};
use crate::room_favs::{
    self, audiopath_for_playback, decode_segment, parse_add_tail, FavoriteMetadata, RoomFavPage,
    StationSource,
};
use crate::speaker::STATION_AUDIOPATH_PREFIX;
use crate::wire_response::build_wire_result_response;

use super::super::Api;

impl Api {
    fn emit_room_fav_changed(&self, player_id: usize) {
        let count = room_favs::count(player_id);
        let _ = self.events.send(IoEvent::RoomFavChanged {
            playerid: player_id,
            count,
        });
    }

    /// For a station the zone's speaker plays from its own app: its description, to keep with
    /// the favorite. Refused when that station is no longer what the speaker plays.
    async fn speaker_station_for_add(
        &self,
        player_id: usize,
        audiopath: &str,
    ) -> anyhow::Result<Option<StationSource>> {
        if !audiopath.starts_with(STATION_AUDIOPATH_PREFIX) {
            return Ok(None);
        }
        let (reply, rx) = oneshot::channel();
        self.sender
            .send(IoCommand::SpeakerStation {
                player_id: Playerid(player_id),
                reply,
            })
            .await?;
        match rx.await.context("speaker station reply dropped")? {
            Some((kind, station)) if station.audiopath == audiopath => Ok(Some(StationSource {
                kind,
                media: station.media,
            })),
            _ => anyhow::bail!("station {audiopath} is no longer playing on the speaker"),
        }
    }

    async fn favorite_metadata_for_add(
        &self,
        player_id: usize,
        audiopath: &str,
    ) -> FavoriteMetadata {
        let mut meta = room_favs::resolve_favorite_metadata(audiopath).await;
        if meta.coverurl.is_empty() {
            let (tx, rx) = oneshot::channel();
            if self
                .sender
                .send(IoCommand::Status(Playerid(player_id), tx))
                .await
                .is_ok()
            {
                if let Ok(state) = rx.await {
                    if room_favs::paths_match_for_metadata(&state.audiopath, audiopath) {
                        let from_state = FavoriteMetadata::from_player_state(&state);
                        if !from_state.coverurl.is_empty() {
                            meta.coverurl = from_state.coverurl;
                        }
                        if meta.title.is_empty() && !from_state.title.is_empty() {
                            meta.title = from_state.title;
                        }
                        if meta.artist.is_empty() && !from_state.artist.is_empty() {
                            meta.artist = from_state.artist;
                        }
                        if meta.album.is_empty() && !from_state.album.is_empty() {
                            meta.album = from_state.album;
                        }
                    }
                }
            }
        }
        meta
    }

    pub(in crate::remote) fn get_room_favs_page(
        &self,
        command: &str,
        params: &Params,
    ) -> anyhow::Result<String> {
        let playerid: Playerid = params
            .get("playerid")
            .context("Missing parameter playerid")?
            .parse()?;
        let start: usize = params
            .get("start")
            .context("Missing parameter start")?
            .parse()?;
        let max_items: usize = params
            .get("max_items")
            .context("Missing parameter max_items")?
            .parse()?;
        let RoomFavPage { items, total } = room_favs::get_page(playerid.0, start, max_items);
        build_wire_result_response(
            "getroomfavs_result",
            json!([RoomFavsResult {
                id: playerid.0,
                result_type: 4,
                start,
                total_items: total,
                items,
            }]),
            Some(command),
            &[],
        )
    }

    pub(in crate::remote) async fn room_favs_cfg(
        &self,
        command: &str,
        params: &Params<'_, '_>,
    ) -> anyhow::Result<String> {
        let playerid: usize = params
            .get("playerid")
            .context("Missing parameter playerid")?
            .parse()?;
        let action = params
            .get("action")
            .context("Missing parameter action")?
            .to_ascii_lowercase();
        let rest = params.get("rest").unwrap_or("").trim_start_matches('/');

        match action.as_str() {
            "add" => {
                let (name, audiopath) = parse_add_tail(rest)?;
                let station = self.speaker_station_for_add(playerid, &audiopath).await?;
                let meta = self.favorite_metadata_for_add(playerid, &audiopath).await;
                let item = room_favs::add(playerid, &name, &audiopath, meta, station)?;
                self.emit_room_fav_changed(playerid);
                build_wire_result_response(
                    "roomfavs_add_result",
                    json!({ "id": item.id, "name": item.name }),
                    Some(command),
                    &[],
                )
            }
            "delete" => {
                let id: usize = rest
                    .split('/')
                    .next()
                    .context("missing favorite id")?
                    .parse()
                    .context("invalid favorite id")?;
                room_favs::remove(playerid, id)?;
                self.emit_room_fav_changed(playerid);
                build_wire_result_response(
                    "roomfavs_delete_result",
                    json!({ "delete_id": id }),
                    Some(command),
                    &[],
                )
            }
            "setname" => {
                let mut parts = rest.splitn(2, '/');
                let id: usize = parts
                    .next()
                    .context("missing id")?
                    .parse()
                    .context("invalid id")?;
                let name = decode_segment(parts.next().context("missing name")?);
                room_favs::set_name(playerid, id, &name)?;
                self.emit_room_fav_changed(playerid);
                build_wire_result_response(
                    "roomfavs_setname_result",
                    json!({ "id": id, "name": name }),
                    Some(command),
                    &[],
                )
            }
            "setid" => {
                let mut parts = rest.split('/');
                let old_id: usize = parts
                    .next()
                    .context("missing old id")?
                    .parse()
                    .context("invalid old id")?;
                let new_id: usize = parts
                    .next()
                    .context("missing new id")?
                    .parse()
                    .context("invalid new id")?;
                room_favs::set_id(playerid, old_id, new_id)?;
                self.emit_room_fav_changed(playerid);
                build_wire_result_response(
                    "roomfavs_set_result",
                    json!({ "changed_from": old_id, "changed_to": new_id }),
                    Some(command),
                    &[],
                )
            }
            "reorder" => {
                let order: Vec<usize> = rest
                    .split(',')
                    .filter_map(|s| s.trim().parse().ok())
                    .collect();
                room_favs::reorder(playerid, &order)?;
                self.emit_room_fav_changed(playerid);
                build_wire_result_response(
                    "roomfavs_reorder_result",
                    json!("ok"),
                    Some(command),
                    &[],
                )
            }
            "copy" => {
                let dest: Vec<usize> = rest
                    .split(',')
                    .filter_map(|s| s.trim().parse().ok())
                    .collect();
                room_favs::copy_to_zones(playerid, &dest)?;
                for &dest_id in &dest {
                    self.emit_room_fav_changed(dest_id);
                }
                build_wire_result_response("roomfavs_copy_result", json!("ok"), Some(command), &[])
            }
            _ => build_wire_result_response("roomfavs_error_result", json!({}), Some(command), &[]),
        }
    }

    pub(in crate::remote) async fn room_fav_play(
        &self,
        command: &str,
        params: &Params<'_, '_>,
    ) -> anyhow::Result<String> {
        let playerid: Playerid = params
            .get("playerid")
            .context("Missing parameter playerid")?
            .parse()?;
        let favorite_id: usize = params
            .get("favorite_id")
            .or_else(|| params.get("rest"))
            .context("Missing favorite id")?
            .trim_start_matches('/')
            .split('/')
            .next()
            .context("empty favorite id")?
            .parse()
            .context("invalid favorite id")?;
        let fav = room_favs::lookup(playerid.0, favorite_id)
            .with_context(|| format!("favorite {favorite_id} not found"))?;
        let command_for_fav = match fav.speaker_media {
            Some(media) => IoCommand::PlaySpeakerStation {
                player_id: playerid,
                kind: fav.speaker_kind,
                media,
            },
            None => IoCommand::Play(
                playerid,
                Some(PlayOpts {
                    id: audiopath_for_playback(&fav.audiopath),
                    session_token: self.session_token.clone(),
                }),
            ),
        };
        self.sender.send(command_for_fav).await?;
        build_wire_result_response(
            "roomfav_result",
            json!([{ "playerid": playerid.0, "playing_slot": favorite_id }]),
            Some(command),
            &[],
        )
    }

    pub(in crate::remote) async fn room_fav_delete(
        &self,
        command: &str,
        params: &Params<'_, '_>,
    ) -> anyhow::Result<String> {
        let playerid: usize = params
            .get("playerid")
            .context("Missing parameter playerid")?
            .parse()?;
        let favorite_id: usize = params
            .get("favorite_id")
            .or_else(|| params.get("rest"))
            .context("Missing favorite id")?
            .trim_start_matches('/')
            .split('/')
            .next()
            .context("empty favorite id")?
            .parse()
            .context("invalid favorite id")?;
        room_favs::remove(playerid, favorite_id)?;
        self.emit_room_fav_changed(playerid);
        build_wire_result_response(
            "roomfav_delete_result",
            json!({ "delete_id": favorite_id }),
            Some(command),
            &[],
        )
    }

    pub(in crate::remote) async fn room_fav_save(
        &self,
        command: &str,
        params: &Params<'_, '_>,
    ) -> anyhow::Result<String> {
        let playerid: Playerid = params
            .get("playerid")
            .context("Missing parameter playerid")?
            .parse()?;
        let (tx, rx) = oneshot::channel();
        self.sender.send(IoCommand::Status(playerid, tx)).await?;
        let state = rx.await.context("status reply dropped")?;
        let station = self
            .speaker_station_for_add(playerid.0, &state.audiopath)
            .await?;
        let item = room_favs::add_from_player_state(playerid.0, &state, station)?;
        self.emit_room_fav_changed(playerid.0);
        build_wire_result_response(
            "roomfavs_add_result",
            json!({ "id": item.id, "name": item.name }),
            Some(command),
            &[],
        )
    }

    pub(in crate::remote) async fn room_fav_save_path(
        &self,
        command: &str,
        params: &Params<'_, '_>,
    ) -> anyhow::Result<String> {
        let playerid: usize = params
            .get("playerid")
            .context("Missing parameter playerid")?
            .parse()?;
        let rest = params
            .get("rest")
            .context("Missing audiopath")?
            .trim_start_matches('/');
        let name = rest
            .rsplit('/')
            .next()
            .map(decode_segment)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "Favorite".to_string());
        let station = self.speaker_station_for_add(playerid, rest).await?;
        let meta = self.favorite_metadata_for_add(playerid, rest).await;
        let item = room_favs::add(playerid, &name, rest, meta, station)?;
        self.emit_room_fav_changed(playerid);
        build_wire_result_response(
            "roomfavs_add_result",
            json!({ "id": item.id, "name": item.name }),
            Some(command),
            &[],
        )
    }
}
