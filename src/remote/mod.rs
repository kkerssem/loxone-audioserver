use crate::{
    loxone_config::PlayerNames,
    miniserver::HardwareState,
    player_state::{PlayerState, Playerid},
    wire_response::build_wire_result_response,
    AppConfig,
};
use anyhow::{anyhow, Context};
use axum::extract::ws::{Message, Utf8Bytes};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::{broadcast, mpsc, oneshot};
use tracing::debug;

mod handlers;
mod router;
mod ws;

pub use ws::{format_customurl_changed_event, format_roomfav_changed_event, start};

pub use router::getqueue_router;
use router::{remote_command_router, Command, GetQueueRoute};

pub(super) struct Api {
    pub(super) app_config: AppConfig,
    pub(super) router: matchit::Router<Command>,
    pub(super) sender: mpsc::Sender<IoCommand>,
    pub(super) events: broadcast::Sender<IoEvent>,
    pub(super) session_token: String,
    pub(super) async_outbound: mpsc::UnboundedSender<Message>,
    // user: Option<String>,
}

#[derive(Clone)]
pub struct AppState {
    pub app_config: AppConfig,
    client: Client,
    pub from_roon: broadcast::Sender<IoEvent>,
    from_api: mpsc::Sender<IoCommand>,
}

/// Loxone `MediaEnum.FileType` values used in browse/search payloads.
pub(crate) const FILE_TYPE_FOLDER: usize = 1;
pub(crate) const FILE_TYPE_TRACK: usize = 2;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BrowseResultItem {
    // pub cmd: String, // this is the name of the servicefolder in audio/cfg/getservicefolder/{presets}/noUser/0/0/50
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audiopath: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artist: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coverurl: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub followed: Option<bool>,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub items: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// `getservices` account fields — V1 copies `lastSelectedItem` into `config.service`; must include `uid`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cmd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fake: Option<bool>,
    /// Nested duplicate for clients that read `item.service` directly.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service: Option<serde_json::Value>,
    pub name: String, // this is the name of the folder in the UI
    pub slot: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub station: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub streamurl: Option<String>,
    pub r#type: usize,
}

#[derive(Debug)]
pub struct BrowseOpts {
    pub id: String,
    pub sender: oneshot::Sender<BrowseResult>,
    #[allow(dead_code)]
    pub session_token: String,
    pub start: usize,
}

#[derive(Debug, Serialize)]
pub struct BrowseResult {
    pub id: String,
    pub name: String,
    pub start: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    #[serde(rename = "totalitems")]
    pub total_items: usize,
    pub items: Vec<BrowseResultItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalSearchCategory {
    pub caption: String,
    pub count: usize,
    pub items: Vec<BrowseResultItem>,
    pub link: String,
    #[serde(rename = "totalitems")]
    pub total_items: usize,
}

/// Top hits section in Spotify-style global search (reference `buildTopResults`).
#[derive(Debug, Serialize, Deserialize)]
pub struct GlobalSearchTopResults {
    pub caption: String,
    pub count: usize,
    pub tracks: Vec<BrowseResultItem>,
    pub albums: Vec<BrowseResultItem>,
    pub artists: Vec<BrowseResultItem>,
    pub playlists: Vec<BrowseResultItem>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GlobalSearchResultInner {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tracks: Option<GlobalSearchCategory>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub albums: Option<GlobalSearchCategory>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artists: Option<GlobalSearchCategory>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub playlists: Option<GlobalSearchCategory>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shows: Option<GlobalSearchCategory>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub episodes: Option<GlobalSearchCategory>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub station: Option<GlobalSearchCategory>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom: Option<GlobalSearchCategory>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topresults: Option<GlobalSearchTopResults>,
    pub user: String,
    /// Full `getservices` entry so the UI can bind `ServiceLoader` when opening a hit (V2 firmware).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GlobalSearchResultBody {
    pub error: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<GlobalSearchResultInner>,
}

impl GlobalSearchResultBody {
    pub fn empty() -> Self {
        Self {
            error: 0,
            result: Some(GlobalSearchResultInner {
                tracks: None,
                albums: None,
                artists: None,
                playlists: None,
                shows: None,
                episodes: None,
                station: None,
                custom: None,
                topresults: None,
                user: "noUser".to_string(),
                service: None,
            }),
        }
    }

    pub fn empty_tunein() -> Self {
        Self::empty()
    }
}

/// `getroomfavs_result` item shape (reference [trace.txt](reference/loxone-ui/trace.txt)).
#[derive(Debug, Serialize)]
pub(crate) struct RoomFavsResult {
    id: usize,
    #[serde(rename = "type")]
    result_type: u8,
    start: usize,
    #[serde(rename = "totalitems")]
    total_items: usize,
    items: Vec<serde_json::Value>,
}

#[derive(Clone, Debug)]
pub enum IoEvent {
    HardwareState(HardwareState),
    #[allow(dead_code)]
    Seek(Playerid, i32),
    State(PlayerState),
    /// Loxone config handshake progress (setconfig → ready).
    ConfigUpdated(crate::loxone_config::LoxoneRuntimeState),
    /// Custom radio stream list changed (`radios/add|del|modify`).
    CustomUrlChanged,
    /// Room favorites changed for a zone (`roomfavs` CRUD).
    RoomFavChanged {
        playerid: usize,
        count: usize,
    },
}

#[derive(Debug)]
pub enum IoCommand {
    /// Play an uploaded voice recording (`voice-uploads/{file}`) once on `zones`, then restore
    /// what they played.
    PlayVoiceMessage {
        file: String,
        zones: Vec<Playerid>,
    },
    GetMediaFolder(BrowseOpts),
    GetServiceFolder {
        service: String,
        folder_id: String,
        start: usize,
        max_items: usize,
        sender: oneshot::Sender<BrowseResult>,
    },
    GlobalSearch {
        provider_spec: String,
        query: String,
        sender: oneshot::Sender<GlobalSearchResultBody>,
    },
    // GetRadios(oneshot::Sender<RadioList>),
    Pause(Playerid),
    Play(Playerid, Option<PlayOpts>),
    /// The station the zone's speaker plays from its own app, with that speaker's API, if
    /// the speaker can play it again (for saving a room favorite).
    SpeakerStation {
        player_id: Playerid,
        reply: oneshot::Sender<Option<(String, crate::speaker::SpeakerStation)>>,
    },
    /// Play a room favorite that the zone's speaker described: `kind` is the speaker API it
    /// was saved from, `media` the speaker's description.
    PlaySpeakerStation {
        player_id: Playerid,
        kind: String,
        media: serde_json::Value,
    },
    /// `audio/cfg/diagnosis`: zones, speakers and Cast devices as the audioserver sees them.
    Diagnosis(oneshot::Sender<crate::coordinator::DiagnosisReport>),
    /// Play a Qobuz item (`qobuz:album:…`, `qobuz:track:…`, …) on the zone's speaker.
    PlayServiceItem {
        player_id: Playerid,
        id: String,
    },
    /// Whether a speaker that can browse Qobuz is bound (the music service is offered).
    QobuzAvailable(oneshot::Sender<bool>),
    Players(PlayerNames),
    /// Seek target in seconds (KEF/Naim speakers via `seekTime`; Cast radio is live).
    Position(Playerid, i32),
    QueueMinus(Playerid),
    QueuePlus(Playerid),
    /// Global search category drill-down: `audio/cfg/search/{service}/{user}/{kind}/{query}/{start}/{max}`.
    SearchCategory {
        service: String,
        kind: String,
        query: String,
        start: usize,
        max_items: usize,
        sender: oneshot::Sender<BrowseResult>,
    },
    #[allow(dead_code)]
    Repeat(Playerid, Repeat),
    Status(Playerid, oneshot::Sender<PlayerState>),
    AllPlayerStatus(oneshot::Sender<Vec<PlayerState>>),
    /// `None` = toggle current shuffle; `Some` = explicit on/off (`shuffle/0`, `shuffle/1`).
    /// `None` toggles; no backend applies shuffle yet (the KEF/Naim API has `playMode`).
    Shuffle(Playerid, #[allow(dead_code)] Option<bool>),
    Volume(Playerid, crate::zone_transport::VolumeStep),
    /// Raw + parsed `setconfig` payload.
    LoxoneSetConfig {
        raw: Vec<u8>,
        parsed: crate::loxone_config::SerialWithConfig,
    },
    LoxoneSpeakerTypes(Vec<crate::loxone_config::SpeakerType>),
    LoxoneVolumes(Vec<crate::loxone_config::Volume>),
    LoxonePlayerNames(PlayerNames),
    LoxonePlayerOpts(Vec<crate::loxone_config::PlayerOpt>),
    LoxoneReady,
}

#[derive(Debug)]
pub struct PlayOpts {
    pub id: String,
    #[allow(dead_code)]
    pub session_token: String,
}

#[derive(Debug, Serialize)]
pub enum Repeat {
    Off = 0,
    All = 1,
    One = 3,
}

impl std::str::FromStr for Repeat {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "0" => Ok(Repeat::Off),
            "1" => Ok(Repeat::All),
            "3" => Ok(Repeat::One),
            _ => Err(anyhow!("Unknown Type: {s}")),
        }
    }
}

/// Answer `getqueue` on 7095 when the Miniserver config router does not match.
pub fn try_dispatch_getqueue(command: &str) -> anyhow::Result<Option<String>> {
    let matched = match getqueue_router().at(command) {
        Ok(m) => m,
        Err(_) => return Ok(None),
    };
    Ok(Some(dispatch_getqueue(command, matched)?))
}

pub fn dispatch_getqueue(
    command: &str,
    matched: matchit::Match<'_, '_, &GetQueueRoute>,
) -> anyhow::Result<String> {
    let playerid = matched
        .params
        .get("playerid")
        .context("Missing parameter playerid")?
        .parse::<usize>()
        .map(Playerid)?;
    let start = match matched.value {
        GetQueueRoute::Paged => matched
            .params
            .get("start")
            .context("Missing parameter start")?
            .parse()?,
        GetQueueRoute::Legacy => 0,
    };
    build_getqueue_response(command, playerid, start)
}

/// Empty `getqueue_result`: playback is started from the speaker's own app, so there is no
/// audioserver-side queue to show.
pub(in crate::remote) fn build_getqueue_response(
    command: &str,
    playerid: Playerid,
    start: usize,
) -> anyhow::Result<String> {
    build_wire_result_response(
        "getqueue_result",
        json!([{
            "id": playerid.0,
            "playerid": playerid.0,
            "items": [],
            "queue": [],
            "shuffle": false,
            "start": start,
            "totalitems": 0,
        }]),
        Some(command),
        &[],
    )
}

fn infer_result_name(command: &str) -> String {
    command
        .split('/')
        .filter(|s| !s.is_empty())
        .rev()
        .find(|s| s.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) && !s.contains(':'))
        .unwrap_or("response")
        .to_string()
}

fn empty_stub_response(command: &str) -> anyhow::Result<String> {
    let name = infer_result_name(command);
    let payload = if name == "recent" {
        json!({ "ts": 0, "items": [] })
    } else {
        json!([])
    };
    build_wire_result_response(&format!("{name}_result"), payload, Some(command), &[])
}

/// Reply for a Qobuz-era command that no longer does anything: the result key and payload
/// shape the Loxone UI waits for, with empty content.
fn retired_command_reply(
    cmd: &Command,
    command: &str,
    params: &matchit::Params,
) -> anyhow::Result<String> {
    let (name, payload) = match cmd {
        Command::GetPlayLists => {
            let empty = BrowseResult {
                id: params.get("id").unwrap_or("0").to_string(),
                name: String::new(),
                start: params
                    .get("start")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0),
                service: None,
                total_items: 0,
                items: vec![],
            };
            ("getplaylists2_result", json!([empty]))
        }
        Command::ServiceCfgGetLink => ("servicecfg_result", json!({ "link": "" })),
        Command::ServiceCfgAdd => (
            "servicecfg_result",
            json!({ "action": "error", "error": 1 }),
        ),
        Command::ServiceCfgDelete => (
            "servicecfg_result",
            json!({ "action": "deleted", "error": 0 }),
        ),
        Command::IsFollowed => (
            "isfollowed_result",
            json!({
                "action": "isfollowed",
                "id": params.get("item_id").unwrap_or(""),
                "isfollowed": false,
                "isowner": false,
            }),
        ),
        Command::PlaylistPlay => ("play_result", json!([])),
        _ => return empty_stub_response(command),
    };
    build_wire_result_response(name, payload, Some(command), &[])
}

impl Api {
    pub(super) fn new(
        app_config: AppConfig,
        session_token: String,
        sender: mpsc::Sender<IoCommand>,
        events: broadcast::Sender<IoEvent>,
        async_outbound: mpsc::UnboundedSender<Message>,
    ) -> Self {
        Self {
            app_config,
            async_outbound,
            events,
            sender,
            session_token,
            router: remote_command_router().clone(),
        }
    }

    pub(super) async fn process_message(&self, utf8_bytes: Utf8Bytes) -> anyhow::Result<Message> {
        let command = utf8_bytes.trim();
        let matched = match self.router.at(command) {
            Ok(m) => m,
            Err(_) => {
                debug!(command, "unknown remote command — empty stub reply");
                return Ok(Message::from(empty_stub_response(command)?));
            }
        };
        let reply = match matched.value {
            Command::Authenticate => self.authenticate(command, &matched.params),
            Command::AuthenticateFalse => self.authenticate_false(command, &matched.params),
            Command::Diagnosis => self.diagnosis(command).await,
            Command::LibraryPlay => self.library_play(command, &matched.params).await,
            Command::GetPlayLists
            | Command::ServiceCfgGetLink
            | Command::ServiceCfgAdd
            | Command::ServiceCfgDelete
            | Command::IsFollowed
            | Command::PlaylistPlay => {
                retired_command_reply(matched.value, command, &matched.params)
            }
            Command::GetInputs => self.get_inputs(command),
            Command::GetKey => self.get_key(command),
            Command::GetKeyFull => self.get_key_full(command),
            Command::GetMediaFolder => self.get_media_folder(command, &matched.params).await,
            Command::GetQueue => self.get_queue_paged(command, &matched.params).await,
            Command::GetQueueLegacy => self.get_queue_legacy(command, &matched.params).await,
            Command::GetRadios => self.get_radios(command).await,
            Command::RadioAdd => self.radio_add(command, &matched.params).await,
            Command::RadioDel => self.radio_del(command, &matched.params).await,
            Command::RadioModify => self.radio_modify(command, &matched.params).await,
            Command::GetRoomFavs => self.get_room_favs(command, &matched.params),
            Command::RoomFavsCfg => self.room_favs_cfg(command, &matched.params).await,
            Command::RoomFavPlay => self.room_fav_play(command, &matched.params).await,
            Command::RoomFavDelete => self.room_fav_delete(command, &matched.params).await,
            Command::RoomFavSave => self.room_fav_save(command, &matched.params).await,
            Command::RoomFavSavePath => self.room_fav_save_path(command, &matched.params).await,
            Command::GetServices => self.get_services(command).await,
            Command::GetAvailableServices => self.get_available_services(command),
            Command::GetMac => self.get_mac(command),
            Command::GetEq => self.get_eq(command, &matched.params),
            Command::SetEq => self.set_eq(command, &matched.params),
            Command::GetEqualizerLegacy => self.get_equalizer_legacy(command, &matched.params),
            Command::SetEqualizerLegacy => self.set_equalizer_legacy(command, &matched.params),
            Command::GetPlayersDetails => self.get_players_details(command).await,
            Command::PropsUpdateLevel => self.props_update_level(command),
            Command::GetServiceFolder | Command::SearchDetails => {
                self.get_service_folder(command, &matched.params).await
            }
            Command::ServicePlay => self.service_play(command, &matched.params).await,
            Command::PlayUrl => self.play_url(command, &matched.params).await,
            Command::GlobalSearchDescribe => self.global_search_describe(command),
            Command::GlobalSearch => self.global_search(command, &matched.params).await,
            Command::Identify => self.identify(command),
            Command::StorageList => self.storage_list(command),
            Command::Recent => self.recent(command, &matched.params),
            Command::GetSyncedPlayers => self.get_synced_players(command),
            Command::Pause
            | Command::Play
            | Command::Resume
            | Command::Position
            | Command::QueueMinus
            | Command::QueuePlus
            | Command::Repeat
            | Command::Shuffle
            | Command::Loudness
            | Command::Status
            | Command::Volume
            | Command::VolumeUp
            | Command::VolumeDown
            | Command::QueuePlay
            | Command::QueueRemove
            | Command::QueueMove => {
                Ok(crate::zone_transport::dispatch(command, &self.sender).await?)
            }
            Command::SearchCategory => self.search_category(command, &matched.params).await,
            Command::Reboot => self.not_implemented(command),
            Command::PlayUploadedFile => self.play_uploaded_file(command, &matched.params).await,
            Command::Rescan => self.rescan(command),
            Command::ScanStatus => self.scan_status(command),
        }?;
        Ok(Message::from(reply))
    }
}
#[cfg(test)]
mod tests {
    use super::handlers::browse::{
        build_globalsearch_broadcast, build_globalsearch_immediate, globalsearch_provider_type,
    };
    use super::{GlobalSearchCategory, GlobalSearchResultBody, GlobalSearchResultInner};
    use crate::wire_response::{build_wire_result_response, first_wire_key};
    use serde_json::json;

    #[test]
    fn parse_volume_step_literals() {
        use crate::zone_transport::parse_volume_step;
        assert_eq!(parse_volume_step("+").unwrap(), 1);
        assert_eq!(parse_volume_step("-").unwrap(), -1);
        assert_eq!(parse_volume_step("+5").unwrap(), 5);
        assert_eq!(parse_volume_step("-3").unwrap(), -3);
    }

    #[test]
    fn globalsearch_wire_response_puts_result_key_first() {
        let immediate = build_globalsearch_immediate(
            "audio/cfg/globalsearch/spotify@qobuz:track#5/beatles",
            "gs-123",
        )
        .unwrap();
        assert!(immediate.starts_with("{\"globalsearch_result\""));
        assert!(immediate.contains("\"command\":"));

        let body = GlobalSearchResultBody {
            error: 0,
            result: Some(GlobalSearchResultInner {
                tracks: Some(GlobalSearchCategory {
                    caption: "Tracks".into(),
                    count: 0,
                    items: vec![],
                    link: "audio/cfg/search/spotify/noUser/track/beatles/0/50".into(),
                    total_items: 0,
                }),
                albums: None,
                artists: None,
                playlists: None,
                shows: None,
                episodes: None,
                station: None,
                custom: None,
                topresults: None,
                user: "noUser".into(),
                service: None,
            }),
        };
        let broadcast = build_globalsearch_broadcast(&body, "spotify", "gs-123").unwrap();
        assert!(broadcast.starts_with("{\"globalsearch_result\""));
        assert!(broadcast.contains("\"type\":\"spotify\""));
        assert!(broadcast.contains("\"unique\":\"gs-123\""));
        assert!(broadcast.contains("\"result\""));
        assert!(broadcast.contains("\"count\""));
        assert!(!broadcast.contains("\"command\""));
    }

    #[test]
    fn globalsearch_tunein_broadcast_uses_flat_envelope() {
        use super::{BrowseResultItem, FILE_TYPE_TRACK};

        let body = GlobalSearchResultBody {
            error: 0,
            result: Some(GlobalSearchResultInner {
                tracks: None,
                albums: None,
                artists: None,
                playlists: None,
                shows: None,
                episodes: None,
                station: Some(GlobalSearchCategory {
                    caption: "Stations".into(),
                    count: 1,
                    total_items: 1,
                    link: String::new(),
                    items: vec![BrowseResultItem {
                        id: "s317396".into(),
                        name: "Paradise".into(),
                        r#type: FILE_TYPE_TRACK,
                        slot: 0,
                        audiopath: Some("tunein:station:s317396".into()),
                        station: Some("Paradise".into()),
                        title: Some("Paradise".into()),
                        content_type: Some("Playlists".into()),
                        ..Default::default()
                    }],
                }),
                custom: Some(GlobalSearchCategory {
                    caption: "Custom".into(),
                    count: 0,
                    total_items: 0,
                    link: String::new(),
                    items: vec![],
                }),
                topresults: None,
                user: "noUser".into(),
                service: None,
            }),
        };
        let broadcast = build_globalsearch_broadcast(&body, "tunein", "gs-paradise").unwrap();
        assert!(broadcast.contains("\"type\":\"tunein\""));
        assert!(broadcast.contains("\"Paradise\""));
        assert!(broadcast.contains("audio/cfg/search/radio/nouser/station/arr/0/50"));
        assert!(!broadcast.contains("\"error\":0"));
        assert!(!broadcast.contains("\"topresults\""));
        assert!(!broadcast.contains("\"result\":{"));
    }

    #[test]
    fn queue_v2_wire_responses_use_reference_result_keys() {
        use crate::wire_response::first_wire_key;
        let play = build_wire_result_response(
            "queue",
            json!({ "play": "abc123def456" }),
            Some("audio/1/queue/play/abc123def456"),
            &[],
        )
        .unwrap();
        assert_eq!(first_wire_key(&play).as_deref(), Some("queue"));
        let remove = build_wire_result_response(
            "remove",
            json!({ "deleted": "abc123def456" }),
            Some("audio/1/queue/remove/abc123def456"),
            &[],
        )
        .unwrap();
        assert_eq!(first_wire_key(&remove).as_deref(), Some("remove"));
        let move_ok = build_wire_result_response(
            "move",
            json!("ok"),
            Some("audio/1/queue/move/a/before/end"),
            &[],
        )
        .unwrap();
        assert_eq!(first_wire_key(&move_ok).as_deref(), Some("move"));
    }

    #[test]
    fn globalsearch_provider_type_maps_spotify_bridge() {
        assert_eq!(
            globalsearch_provider_type("spotify@qobuz:track#5,album#5"),
            "spotify"
        );
        assert_eq!(globalsearch_provider_type("local:track#5"), "local");
    }

    #[test]
    fn geteq_wire_has_ten_bands_not_empty_array() {
        use crate::player_state::Playerid;
        use crate::zone_equalizer::{build_geteq_result, get_bands, set_band};

        set_band(Playerid(2), 3, 1.5).unwrap();
        let wire = build_wire_result_response(
            "geteq_result",
            serde_json::to_value(build_geteq_result(&get_bands(Playerid(2)))).unwrap(),
            Some("audio/cfg/geteq/2"),
            &[],
        )
        .unwrap();
        assert!(wire.starts_with("{\"geteq_result\":["));
        assert!(wire.contains("\"name\":\"31 Hz\""));
        assert!(!wire.contains("\"geteq_result\":[]"));
    }

    #[test]
    fn retired_commands_reply_with_their_own_result_key_and_shape() {
        let router = super::router::remote_command_router();
        let reply = |path: &str| -> serde_json::Value {
            let matched = router.at(path).expect("route");
            let wire = super::retired_command_reply(matched.value, path, &matched.params).unwrap();
            serde_json::from_str(&wire).unwrap()
        };

        let v = reply("audio/cfg/servicecfg/getlink/spotify");
        assert_eq!(v["servicecfg_result"], json!({ "link": "" }));
        let v = reply("audio/cfg/isfollowed/spotify/qobuz/qobuz:track:1");
        assert_eq!(v["isfollowed_result"]["isfollowed"], json!(false));
        assert_eq!(v["isfollowed_result"]["id"], json!("qobuz:track:1"));
        let v = reply("audio/cfg/getplaylists2/lms/nouser/0/0/20");
        assert_eq!(v["getplaylists2_result"][0]["totalitems"], json!(0));
        assert_eq!(v["getplaylists2_result"][0]["items"], json!([]));
        let v = reply("audio/3/playlist/play/qobuz:track:1/noshuffle");
        assert_eq!(v["play_result"], json!([]));
    }

    #[test]
    fn wire_handlers_put_result_key_first() {
        let getservices = build_wire_result_response(
            "getservices_result",
            json!([]),
            Some("audio/cfg/getservices"),
            &[],
        )
        .unwrap();
        assert_eq!(
            first_wire_key(&getservices).as_deref(),
            Some("getservices_result")
        );

        let getkey = build_wire_result_response(
            "getkey_result",
            json!([{"pubkey": "abc", "exp": 65537}]),
            Some("audio/cfg/getkey"),
            &[],
        )
        .unwrap();
        assert_eq!(first_wire_key(&getkey).as_deref(), Some("getkey_result"));
    }

    /// An `Api` whose coordinator side is the returned receiver.
    fn api() -> (super::Api, tokio::sync::mpsc::Receiver<super::IoCommand>) {
        let app_config = crate::AppConfig::new("504F94000002".into(), None, None, None).unwrap();
        let (sender, receiver) = tokio::sync::mpsc::channel(8);
        let (events, _) = tokio::sync::broadcast::channel(8);
        let (outbound, _) = tokio::sync::mpsc::unbounded_channel();
        (
            super::Api::new(app_config, "t".into(), sender, events, outbound),
            receiver,
        )
    }

    async fn send(api: &super::Api, command: &str) -> serde_json::Value {
        let reply = api.process_message(command.into()).await.unwrap();
        match reply {
            axum::extract::ws::Message::Text(text) => serde_json::from_str(text.as_str()).unwrap(),
            other => panic!("unexpected reply {other:?}"),
        }
    }

    fn played(command: Option<super::IoCommand>) -> (usize, String) {
        match command {
            Some(super::IoCommand::PlayServiceItem { player_id, id }) => (player_id.0, id),
            other => panic!("expected a qobuz play, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn qobuz_items_from_every_play_route_reach_the_coordinator() {
        let (api, mut rx) = api();
        let v = send(
            &api,
            "audio/3/serviceplay/spotify/qobuz/qobuz:album:A1/?q&ZW5mb3JjZVVzZXI9dHJ1ZQ",
        )
        .await;
        assert_eq!(v["serviceplay_result"], json!([]));
        assert_eq!(played(rx.recv().await), (3, "qobuz:album:A1".into()));

        send(&api, "audio/2/playurl/spotify:track:T1/noshuffle").await;
        assert_eq!(played(rx.recv().await), (2, "qobuz:track:T1".into()));

        let v = send(
            &api,
            "audio/4/library/play/qobuz:track:T2/parentid/qobuz:playlist:P1/3/noshuffle",
        )
        .await;
        assert_eq!(v["play_result"], json!([]));
        assert_eq!(played(rx.recv().await), (4, "qobuz:playlist:P1".into()));

        send(&api, "audio/4/library/play/qobuz:album:A2").await;
        assert_eq!(played(rx.recv().await), (4, "qobuz:album:A2".into()));

        // Neither radio nor Qobuz: answered, nothing played.
        send(&api, "audio/4/library/play/local:file:1").await;
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn the_qobuz_account_is_listed_while_a_speaker_can_browse_it() {
        for available in [true, false] {
            let (api, mut rx) = api();
            tokio::spawn(async move {
                if let Some(super::IoCommand::QobuzAvailable(reply)) = rx.recv().await {
                    let _ = reply.send(available);
                }
            });
            let v = send(&api, "audio/cfg/getservices").await;
            let services = v["getservices_result"].as_array().unwrap();
            assert_eq!(services.len(), usize::from(available));
            if available {
                assert_eq!(services[0]["uid"], "spotify/qobuz");
            }
        }
    }
}
