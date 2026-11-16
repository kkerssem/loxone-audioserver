//! 7091 WebSocket command route table (`matchit`).

use std::sync::OnceLock;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Authenticate,
    AuthenticateFalse,
    Diagnosis,
    GetInputs,
    GetKey,
    GetKeyFull,
    GetMediaFolder,
    GetQueue,
    GetQueueLegacy,
    GetPlayLists,
    GetRadios,
    GetRoomFavs,
    RoomFavsCfg,
    RoomFavPlay,
    RoomFavDelete,
    RoomFavSave,
    RoomFavSavePath,
    GetServices,
    GetSyncedPlayers,
    GetAvailableServices,
    GetMac,
    GetPlayersDetails,
    GetServiceFolder,
    SearchDetails,
    ServicePlay,
    PlayUrl,
    PropsUpdateLevel,
    GlobalSearchDescribe,
    GlobalSearch,
    ServiceCfgGetLink,
    ServiceCfgAdd,
    ServiceCfgDelete,
    IsFollowed,
    VolumeUp,
    VolumeDown,
    Identify,
    StorageList,
    Recent,
    LibraryPlay,
    PlaylistPlay,
    Position,
    Reboot,
    /// `audio/grouped/playuploadedfile/{file}/{zones}`: play an uploaded voice recording.
    PlayUploadedFile,
    Rescan,
    ScanStatus,
    Shuffle,
    Loudness,
    GetEq,
    SetEq,
    GetEqualizerLegacy,
    SetEqualizerLegacy,
    Pause,
    Play,
    RadioAdd,
    RadioDel,
    RadioModify,
    QueueMinus,
    QueuePlus,
    QueuePlay,
    QueueRemove,
    QueueMove,
    Resume,
    SearchCategory,
    Repeat,
    Status,
    Volume,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GetQueueRoute {
    Paged,
    Legacy,
}

fn insert_route(router: &mut matchit::Router<Command>, path: &str, cmd: Command) {
    router.insert(path, cmd).unwrap();
}

fn build_remote_router() -> matchit::Router<Command> {
    let mut router = matchit::Router::new();
    insert_route(&mut router, "audio/cfg/diagnosis", Command::Diagnosis);
    insert_route(&mut router, "audio/cfg/getinputs", Command::GetInputs);
    insert_route(
        &mut router,
        "audio/cfg/getavailableservices",
        Command::GetAvailableServices,
    );
    insert_route(
        &mut router,
        "audio/cfg/globalsearch/describe",
        Command::GlobalSearchDescribe,
    );
    insert_route(
        &mut router,
        "audio/cfg/globalsearch/{provider_spec}/{*query}",
        Command::GlobalSearch,
    );
    insert_route(&mut router, "audio/cfg/identify", Command::Identify);
    insert_route(&mut router, "audio/cfg/storage/list", Command::StorageList);
    insert_route(&mut router, "audio/cfg/getkey/full", Command::GetKeyFull);
    insert_route(&mut router, "audio/cfg/getkey", Command::GetKey);
    insert_route(
        &mut router,
        "audio/cfg/seteq/{playerid}/{*rest}",
        Command::SetEq,
    );
    insert_route(&mut router, "audio/cfg/geteq/{playerid}", Command::GetEq);
    insert_route(
        &mut router,
        "audio/cfg/equalizer/{playerid}/{*rest}",
        Command::SetEqualizerLegacy,
    );
    insert_route(
        &mut router,
        "audio/cfg/equalizer/{playerid}",
        Command::GetEqualizerLegacy,
    );
    insert_route(&mut router, "audio/cfg/mac", Command::GetMac);
    insert_route(
        &mut router,
        "audio/cfg/getplayersdetails",
        Command::GetPlayersDetails,
    );
    insert_route(
        &mut router,
        "audio/cfg/props/updatelevel",
        Command::PropsUpdateLevel,
    );
    insert_route(
        &mut router,
        "audio/cfg/isfollowed/{service}/{user}/{item_id}",
        Command::IsFollowed,
    );
    insert_route(
        &mut router,
        "audio/cfg/getservicefolder/{service}/{user}/{folder_id}/{start}/{max_items}",
        Command::GetServiceFolder,
    );
    insert_route(
        &mut router,
        "audio/cfg/getservicefolder/{service}/noUser/{start}/{max_items}",
        Command::GetServiceFolder,
    );
    insert_route(
        &mut router,
        "audio/cfg/getservicefolder/{service}/{folder_id}/{start}/{max_items}",
        Command::GetServiceFolder,
    );
    insert_route(
        &mut router,
        "audio/cfg/searchdetails/{service}/{user}/{folder_id}/{start}/{max_items}",
        Command::SearchDetails,
    );
    insert_route(
        &mut router,
        "audio/cfg/searchdetails/{service}/{folder_id}/{start}/{max_items}",
        Command::SearchDetails,
    );
    insert_route(
        &mut router,
        "audio/cfg/search/{service}/{user}/{kind}/{query}/{start}/{max_items}",
        Command::SearchCategory,
    );
    insert_route(
        &mut router,
        "audio/cfg/servicecfg/getlink/{service}",
        Command::ServiceCfgGetLink,
    );
    insert_route(
        &mut router,
        "audio/cfg/servicecfg/add/{service}",
        Command::ServiceCfgAdd,
    );
    insert_route(
        &mut router,
        "audio/cfg/servicecfg/delete/{service}/{id}",
        Command::ServiceCfgDelete,
    );
    insert_route(
        &mut router,
        "audio/cfg/getmediafolder/{id}/{start}/{max_items}",
        Command::GetMediaFolder,
    );
    insert_route(&mut router, "audio/cfg/getradios", Command::GetRadios);
    insert_route(
        &mut router,
        "audio/cfg/radios/add/{name}/{*rest}",
        Command::RadioAdd,
    );
    insert_route(&mut router, "audio/cfg/radios/del/{id}", Command::RadioDel);
    insert_route(
        &mut router,
        "audio/cfg/radios/modify/{id}/{name}/{*rest}",
        Command::RadioModify,
    );
    insert_route(
        &mut router,
        "audio/cfg/getplaylists2/lms/nouser/{id}/{start}/{max_items}",
        Command::GetPlayLists,
    );
    insert_route(
        &mut router,
        "audio/cfg/getroomfavs/{playerid}/{start}/{max_items}",
        Command::GetRoomFavs,
    );
    insert_route(
        &mut router,
        "audio/cfg/roomfavs/{playerid}/{action}/{*rest}",
        Command::RoomFavsCfg,
    );
    insert_route(&mut router, "audio/cfg/getservices", Command::GetServices);
    insert_route(
        &mut router,
        "audio/cfg/getsyncedplayers",
        Command::GetSyncedPlayers,
    );
    insert_route(&mut router, "audio/cfg/reboot", Command::Reboot);
    insert_route(
        &mut router,
        "audio/grouped/playuploadedfile/{file}/{zones}",
        Command::PlayUploadedFile,
    );
    insert_route(&mut router, "audio/cfg/rescan", Command::Rescan);
    insert_route(&mut router, "audio/cfg/scanstatus", Command::ScanStatus);
    insert_route(
        &mut router,
        "audio/{playerid}/getqueue/{start}/{max_items}",
        Command::GetQueue,
    );
    insert_route(
        &mut router,
        "audio/{playerid}/getqueue",
        Command::GetQueueLegacy,
    );
    insert_route(
        &mut router,
        "audio/{playerid}/library/play/{id}/parentid/{parentid}/{item_number}/noshuffle",
        Command::LibraryPlay,
    );
    insert_route(
        &mut router,
        "audio/{playerid}/library/play/{id}",
        Command::LibraryPlay,
    );
    insert_route(
        &mut router,
        "audio/{playerid}/library/play/{id}/noshuffle",
        Command::LibraryPlay,
    );
    insert_route(
        &mut router,
        "audio/{playerid}/playlist/play/{id}/noshuffle",
        Command::PlaylistPlay,
    );
    insert_route(
        &mut router,
        "audio/{playerid}/playlist/play/{id}",
        Command::PlaylistPlay,
    );
    insert_route(&mut router, "audio/{playerid}/recent", Command::Recent);
    insert_route(
        &mut router,
        "audio/{playerid}/recent/{start}",
        Command::Recent,
    );
    insert_route(&mut router, "audio/{playerid}/pause", Command::Pause);
    insert_route(&mut router, "audio/{playerid}/play", Command::Play);
    insert_route(&mut router, "audio/{playerid}/resume", Command::Resume);
    insert_route(
        &mut router,
        "audio/{playerid}/queue/play/{unique_id}",
        Command::QueuePlay,
    );
    insert_route(
        &mut router,
        "audio/{playerid}/queue/remove/{unique_id}",
        Command::QueueRemove,
    );
    insert_route(
        &mut router,
        "audio/{playerid}/queue/move/{src_uid}/before/{before_uid}",
        Command::QueueMove,
    );
    insert_route(
        &mut router,
        "audio/{playerid}/position/{position}",
        Command::Position,
    );
    insert_route(
        &mut router,
        "audio/{playerid}/queueminus",
        Command::QueueMinus,
    );
    insert_route(
        &mut router,
        "audio/{playerid}/queueplus",
        Command::QueuePlus,
    );
    insert_route(
        &mut router,
        "audio/{playerid}/repeat/{repeat}",
        Command::Repeat,
    );
    insert_route(&mut router, "audio/{playerid}/shuffle/0", Command::Shuffle);
    insert_route(&mut router, "audio/{playerid}/shuffle/1", Command::Shuffle);
    insert_route(&mut router, "audio/{playerid}/shuffle", Command::Shuffle);
    insert_route(
        &mut router,
        "audio/{playerid}/loudness/0",
        Command::Loudness,
    );
    insert_route(
        &mut router,
        "audio/{playerid}/loudness/1",
        Command::Loudness,
    );
    insert_route(&mut router, "audio/{playerid}/loudness", Command::Loudness);
    insert_route(&mut router, "audio/{playerid}/status", Command::Status);
    insert_route(
        &mut router,
        "audio/{playerid}/volume/{step}",
        Command::Volume,
    );
    insert_route(&mut router, "audio/{playerid}/volume/+", Command::Volume);
    insert_route(&mut router, "audio/{playerid}/volume/-", Command::Volume);
    insert_route(&mut router, "audio/{playerid}/volup", Command::VolumeUp);
    insert_route(&mut router, "audio/{playerid}/voldown", Command::VolumeDown);
    insert_route(
        &mut router,
        "audio/{playerid}/roomfav/play/{*rest}",
        Command::RoomFavPlay,
    );
    insert_route(
        &mut router,
        "audio/{playerid}/roomfav/delete/{*rest}",
        Command::RoomFavDelete,
    );
    insert_route(
        &mut router,
        "audio/{playerid}/roomfav/save/",
        Command::RoomFavSave,
    );
    insert_route(
        &mut router,
        "audio/{playerid}/roomfav/savepath/{*rest}",
        Command::RoomFavSavePath,
    );
    insert_route(
        &mut router,
        "audio/{playerid}/serviceplay/{*rest}",
        Command::ServicePlay,
    );
    insert_route(
        &mut router,
        "audio/{playerid}/playurl/{*rest}",
        Command::PlayUrl,
    );
    insert_route(
        &mut router,
        "secure/authenticate/{user}/false/{signature}",
        Command::AuthenticateFalse,
    );
    insert_route(
        &mut router,
        "secure/authenticate/{user}/{tuple}/{ciphertext}",
        Command::Authenticate,
    );

    router
}

pub(crate) fn remote_command_router() -> &'static matchit::Router<Command> {
    static ROUTER: OnceLock<matchit::Router<Command>> = OnceLock::new();
    ROUTER.get_or_init(build_remote_router)
}

/// Shared router for `audio/{playerid}/getqueue` (7091 and 7095 fallback).
pub fn getqueue_router() -> &'static matchit::Router<GetQueueRoute> {
    static ROUTER: OnceLock<matchit::Router<GetQueueRoute>> = OnceLock::new();
    ROUTER.get_or_init(|| {
        let mut router = matchit::Router::new();
        router
            .insert(
                "audio/{playerid}/getqueue/{start}/{max_items}",
                GetQueueRoute::Paged,
            )
            .unwrap();
        router
            .insert("audio/{playerid}/getqueue", GetQueueRoute::Legacy)
            .unwrap();
        router
    })
}

#[cfg(test)]
mod tests {
    use super::{getqueue_router, remote_command_router, Command, GetQueueRoute};

    #[test]
    fn browse_routes_match_ui_paths() {
        let router = remote_command_router();

        let ui = router
            .at("audio/cfg/getservicefolder/spotify/noUser/start/0/50")
            .unwrap();
        assert_eq!(ui.value, &Command::GetServiceFolder);
        assert_eq!(ui.params.get("service"), Some("spotify"));
        assert_eq!(ui.params.get("user"), Some("noUser"));
        assert_eq!(ui.params.get("folder_id"), Some("start"));

        let radio = router
            .at("audio/cfg/getservicefolder/local/local/start/0/20")
            .unwrap();
        assert_eq!(radio.params.get("service"), Some("local"));
        assert_eq!(radio.params.get("user"), Some("local"));
        assert_eq!(radio.params.get("folder_id"), Some("start"));

        let search_drilldown = router
            .at("audio/cfg/getservicefolder/spotify/qobuz:album:hfho3xifs8dre/0/20")
            .unwrap();
        assert_eq!(search_drilldown.value, &Command::GetServiceFolder);
        assert_eq!(search_drilldown.params.get("service"), Some("spotify"));
        assert_eq!(
            search_drilldown.params.get("folder_id"),
            Some("qobuz:album:hfho3xifs8dre")
        );
        assert_eq!(search_drilldown.params.get("start"), Some("0"));
        assert_eq!(search_drilldown.params.get("max_items"), Some("20"));

        let no_user_shorthand = router
            .at("audio/cfg/getservicefolder/spotify/noUser/0/50")
            .unwrap();
        assert_eq!(no_user_shorthand.value, &Command::GetServiceFolder);
        assert_eq!(no_user_shorthand.params.get("service"), Some("spotify"));
        assert_eq!(no_user_shorthand.params.get("start"), Some("0"));
        assert_eq!(no_user_shorthand.params.get("max_items"), Some("50"));
        assert!(no_user_shorthand.params.get("folder_id").is_none());

        let playurl = router
            .at("audio/2/playurl/qobuz:track:64868955/noshuffle")
            .unwrap();
        assert_eq!(playurl.value, &Command::PlayUrl);
        assert_eq!(playurl.params.get("playerid"), Some("2"));
        assert_eq!(
            playurl.params.get("rest"),
            Some("qobuz:track:64868955/noshuffle")
        );

        let search_details = router
            .at("audio/cfg/searchdetails/spotify/user@example.com/qobuz:album:0088807204128/0/20")
            .unwrap();
        assert_eq!(search_details.value, &Command::SearchDetails);
        assert_eq!(
            search_details.params.get("folder_id"),
            Some("qobuz:album:0088807204128")
        );

        let search_details_short = router
            .at("audio/cfg/searchdetails/spotify/qobuz:album:0088807204128/0/20")
            .unwrap();
        assert_eq!(search_details_short.value, &Command::SearchDetails);
        assert_eq!(
            search_details_short.params.get("folder_id"),
            Some("qobuz:album:0088807204128")
        );

        let search = router
            .at("audio/cfg/globalsearch/qobuz:track#5,album#5/hello%20world")
            .unwrap();
        assert_eq!(search.value, &Command::GlobalSearch);
        assert_eq!(
            search.params.get("provider_spec"),
            Some("qobuz:track#5,album#5")
        );
        assert_eq!(search.params.get("query"), Some("hello%20world"));

        let radio_search = router
            .at("audio/cfg/search/radio/nouser/station/arr/0/50")
            .unwrap();
        assert_eq!(radio_search.value, &Command::SearchCategory);
        assert_eq!(radio_search.params.get("service"), Some("radio"));
        assert_eq!(radio_search.params.get("kind"), Some("station"));
        assert_eq!(radio_search.params.get("query"), Some("arr"));
        assert_eq!(radio_search.params.get("start"), Some("0"));
        assert_eq!(radio_search.params.get("max_items"), Some("50"));

        let spotify_search = router
            .at("audio/cfg/search/spotify/noUser/track/beatles/0/50")
            .unwrap();
        assert_eq!(spotify_search.value, &Command::SearchCategory);
        assert_eq!(spotify_search.params.get("service"), Some("spotify"));

        assert!(router.at("audio/cfg/globalsearch/describe").is_ok());
    }

    #[test]
    fn geteq_and_seteq_routes_match() {
        let router = remote_command_router();

        assert_eq!(
            router.at("audio/cfg/geteq/2").unwrap().value,
            &Command::GetEq
        );
        let set_one = router.at("audio/cfg/seteq/2/3/1.5").unwrap();
        assert_eq!(set_one.value, &Command::SetEq);
        assert_eq!(set_one.params.get("rest"), Some("3/1.5"));
        let set_all = router.at("audio/cfg/seteq/2/1,0,0,0,0,0,0,0,0,0").unwrap();
        assert_eq!(set_all.params.get("rest"), Some("1,0,0,0,0,0,0,0,0,0"));
    }

    #[test]
    fn getqueue_routes_extract_params() {
        let router = getqueue_router();

        let paged = router.at("audio/1/getqueue/0/50").unwrap();
        assert_eq!(paged.value, &GetQueueRoute::Paged);
        assert_eq!(paged.params.get("playerid"), Some("1"));
        assert_eq!(paged.params.get("start"), Some("0"));
        assert_eq!(paged.params.get("max_items"), Some("50"));

        let legacy = router.at("audio/1/getqueue").unwrap();
        assert_eq!(legacy.value, &GetQueueRoute::Legacy);
        assert_eq!(legacy.params.get("playerid"), Some("1"));
    }

    #[test]
    fn transport_routes_resolve() {
        let router = remote_command_router();

        assert_eq!(router.at("audio/1/play").unwrap().value, &Command::Play);
        assert_eq!(router.at("audio/1/pause").unwrap().value, &Command::Pause);
        assert_eq!(
            router.at("audio/1/queueplus").unwrap().value,
            &Command::QueuePlus
        );
        assert_eq!(
            router.at("audio/1/volume/+1").unwrap().value,
            &Command::Volume
        );
        assert_eq!(
            router.at("audio/1/repeat/1").unwrap().value,
            &Command::Repeat
        );
        assert_eq!(
            router.at("audio/1/position/120").unwrap().value,
            &Command::Position
        );
        assert_eq!(
            router
                .at("audio/3/playlist/play/qobuz:track:422318103")
                .unwrap()
                .value,
            &Command::PlaylistPlay
        );
        assert_eq!(
            router
                .at("audio/3/playlist/play/qobuz:track:422318103/noshuffle")
                .unwrap()
                .value,
            &Command::PlaylistPlay
        );
        assert_eq!(
            router.at("audio/1/shuffle/1").unwrap().value,
            &Command::Shuffle
        );
    }

    #[test]
    fn bootstrap_routes_resolve() {
        let router = remote_command_router();

        assert_eq!(
            router.at("audio/cfg/getservices").unwrap().value,
            &Command::GetServices
        );
        assert_eq!(
            router.at("audio/cfg/getkey/full").unwrap().value,
            &Command::GetKeyFull
        );
        let auth = router
            .at("secure/authenticate/user1/tuple1/cipher1")
            .unwrap();
        assert_eq!(auth.value, &Command::Authenticate);
        assert_eq!(auth.params.get("user"), Some("user1"));
        assert_eq!(auth.params.get("tuple"), Some("tuple1"));
        assert_eq!(auth.params.get("ciphertext"), Some("cipher1"));
    }

    #[test]
    fn resume_route_resolves_on_7091() {
        let router = remote_command_router();
        assert_eq!(router.at("audio/1/resume").unwrap().value, &Command::Resume);
    }

    /// One sample path per registered `Command` variant (see `build_remote_router`).
    #[test]
    fn all_command_variants_have_route_sample() {
        const ROUTE_SAMPLES: &[(&str, Command)] = &[
            ("audio/cfg/diagnosis", Command::Diagnosis),
            ("audio/cfg/getinputs", Command::GetInputs),
            (
                "audio/cfg/getavailableservices",
                Command::GetAvailableServices,
            ),
            (
                "audio/cfg/globalsearch/describe",
                Command::GlobalSearchDescribe,
            ),
            (
                "audio/cfg/globalsearch/qobuz:track/hello",
                Command::GlobalSearch,
            ),
            ("audio/cfg/identify", Command::Identify),
            ("audio/cfg/storage/list", Command::StorageList),
            ("audio/cfg/getkey", Command::GetKey),
            ("audio/cfg/getkey/full", Command::GetKeyFull),
            ("audio/cfg/geteq/1", Command::GetEq),
            ("audio/cfg/seteq/1/0/0", Command::SetEq),
            ("audio/cfg/equalizer/1", Command::GetEqualizerLegacy),
            ("audio/cfg/equalizer/1/0,0", Command::SetEqualizerLegacy),
            ("audio/cfg/mac", Command::GetMac),
            ("audio/cfg/getplayersdetails", Command::GetPlayersDetails),
            ("audio/cfg/props/updatelevel", Command::PropsUpdateLevel),
            (
                "audio/cfg/isfollowed/spotify/user1/item1",
                Command::IsFollowed,
            ),
            (
                "audio/cfg/getservicefolder/spotify/noUser/0/50",
                Command::GetServiceFolder,
            ),
            (
                "audio/cfg/searchdetails/spotify/folder/0/20",
                Command::SearchDetails,
            ),
            (
                "audio/cfg/search/spotify/noUser/track/beatles/0/50",
                Command::SearchCategory,
            ),
            (
                "audio/cfg/servicecfg/getlink/spotify",
                Command::ServiceCfgGetLink,
            ),
            ("audio/cfg/servicecfg/add/spotify", Command::ServiceCfgAdd),
            (
                "audio/cfg/servicecfg/delete/spotify/qobuz",
                Command::ServiceCfgDelete,
            ),
            ("audio/cfg/getmediafolder/0/0/50", Command::GetMediaFolder),
            ("audio/cfg/getradios", Command::GetRadios),
            (
                "audio/cfg/getplaylists2/lms/nouser/0/0/50",
                Command::GetPlayLists,
            ),
            ("audio/cfg/getroomfavs/1/0/50", Command::GetRoomFavs),
            (
                "audio/cfg/roomfavs/3/add/Name/tunein:station:s2611",
                Command::RoomFavsCfg,
            ),
            ("audio/cfg/getservices", Command::GetServices),
            ("audio/cfg/getsyncedplayers", Command::GetSyncedPlayers),
            ("audio/cfg/reboot", Command::Reboot),
            (
                "audio/grouped/playuploadedfile/audio_recording.wav/3,4",
                Command::PlayUploadedFile,
            ),
            ("audio/cfg/rescan", Command::Rescan),
            ("audio/cfg/scanstatus", Command::ScanStatus),
            ("audio/1/getqueue/0/50", Command::GetQueue),
            ("audio/1/getqueue", Command::GetQueueLegacy),
            ("audio/1/library/play/track1", Command::LibraryPlay),
            (
                "audio/3/playlist/play/qobuz:track:422318103",
                Command::PlaylistPlay,
            ),
            (
                "audio/3/playlist/play/qobuz:track:422318103/noshuffle",
                Command::PlaylistPlay,
            ),
            ("audio/1/recent", Command::Recent),
            ("audio/1/recent/0", Command::Recent),
            ("audio/1/pause", Command::Pause),
            ("audio/1/play", Command::Play),
            ("audio/1/resume", Command::Resume),
            ("audio/1/position/30", Command::Position),
            ("audio/1/queueminus", Command::QueueMinus),
            ("audio/1/queueplus", Command::QueuePlus),
            ("audio/1/queue/play/abc123def456", Command::QueuePlay),
            ("audio/1/queue/remove/abc123def456", Command::QueueRemove),
            (
                "audio/1/queue/move/abc123def456/before/end",
                Command::QueueMove,
            ),
            ("audio/1/repeat/1", Command::Repeat),
            ("audio/1/shuffle", Command::Shuffle),
            ("audio/1/shuffle/0", Command::Shuffle),
            ("audio/1/shuffle/1", Command::Shuffle),
            ("audio/1/loudness/0", Command::Loudness),
            ("audio/1/loudness", Command::Loudness),
            ("audio/1/status", Command::Status),
            ("audio/1/volume/50", Command::Volume),
            ("audio/1/volume/+", Command::Volume),
            ("audio/1/volup", Command::VolumeUp),
            ("audio/1/voldown", Command::VolumeDown),
            ("audio/1/serviceplay/qobuz:track:1", Command::ServicePlay),
            ("audio/1/playurl/qobuz:track:1", Command::PlayUrl),
            ("audio/3/roomfav/play/1", Command::RoomFavPlay),
            ("audio/3/roomfav/delete/1", Command::RoomFavDelete),
            (
                "secure/authenticate/user1/false/sig1",
                Command::AuthenticateFalse,
            ),
            (
                "secure/authenticate/user1/tuple1/cipher1",
                Command::Authenticate,
            ),
        ];

        let router = remote_command_router();
        for (path, expected) in ROUTE_SAMPLES {
            let matched = router
                .at(path)
                .unwrap_or_else(|_| panic!("route not found: {path}"));
            assert_eq!(matched.value, expected, "path: {path}");
        }
    }
}
