//! 7095 Miniserver upstream command route table (`matchit`).

use std::sync::OnceLock;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Command {
    Bluetooth,
    DiagCmd,
    Diagnosis,
    Forget,
    GetConfig,
    GetRoomFavs,
    GroupOpts,
    Identify,
    MiniserverIp,
    MiniserverTime,
    MiniserverVersion,
    PresenceMode,
    PlayerName,
    PlayerOpts,
    Ready,
    Reboot,
    SetConfig,
    SetConfigTimestamp,
    SpeakerType,
    Timezone,
    Volumes,
    Hello,
    InfoPairing,
    Init,
    SecureForget,
}

fn insert_route(router: &mut matchit::Router<Command>, path: &str, cmd: Command) {
    router.insert(path, cmd).unwrap();
}

fn build_miniserver_router() -> matchit::Router<Command> {
    let mut router = matchit::Router::new();
    insert_route(
        &mut router,
        "audio/cfg/bluetooth/{playerid}/visible",
        Command::Bluetooth,
    );
    // Seen from Miniserver 17.3 after `ready` (`audio/cfg/diagcmd/patch:vrfy`); no reference
    // handler, answered like `diagnosis`.
    insert_route(&mut router, "audio/cfg/diagcmd", Command::DiagCmd);
    insert_route(&mut router, "audio/cfg/diagcmd/{*cmd}", Command::DiagCmd);
    insert_route(&mut router, "audio/cfg/diagnosis", Command::Diagnosis);
    insert_route(&mut router, "audio/cfg/forget", Command::Forget);
    insert_route(&mut router, "audio/cfg/getconfig", Command::GetConfig);
    insert_route(
        &mut router,
        "audio/cfg/getroomfavs/all",
        Command::GetRoomFavs,
    );
    insert_route(
        &mut router,
        "audio/cfg/groupopts/{data}",
        Command::GroupOpts,
    );
    insert_route(&mut router, "audio/cfg/identify/off", Command::Identify);
    insert_route(
        &mut router,
        "audio/cfg/miniservertime/{time}",
        Command::MiniserverTime,
    );
    insert_route(
        &mut router,
        "audio/cfg/miniserverip/{*rest}",
        Command::MiniserverIp,
    );
    insert_route(
        &mut router,
        "audio/cfg/miniserverversion/{latest}/{minimum}",
        Command::MiniserverVersion,
    );
    insert_route(
        &mut router,
        "audio/cfg/presencemode/{playerid}/{mode}",
        Command::PresenceMode,
    );
    insert_route(
        &mut router,
        "audio/cfg/playername/{data}",
        Command::PlayerName,
    );
    insert_route(
        &mut router,
        "audio/cfg/playeropts/{data}",
        Command::PlayerOpts,
    );
    insert_route(&mut router, "audio/cfg/ready", Command::Ready);
    insert_route(&mut router, "audio/cfg/reboot", Command::Reboot);
    insert_route(
        &mut router,
        "audio/cfg/setconfig/{data}",
        Command::SetConfig,
    );
    insert_route(
        &mut router,
        "audio/cfg/setconfigtimestamp/{time}",
        Command::SetConfigTimestamp,
    );
    insert_route(
        &mut router,
        "audio/cfg/speakertype/{data}",
        Command::SpeakerType,
    );
    insert_route(
        &mut router,
        "audio/cfg/timezone/{*timezone}",
        Command::Timezone,
    );
    insert_route(&mut router, "audio/cfg/volumes/{data}", Command::Volumes);
    insert_route(&mut router, "secure/hello/{time}/{data}", Command::Hello);
    insert_route(&mut router, "secure/info/pairing", Command::InfoPairing);
    insert_route(&mut router, "secure/init/{jwt}", Command::Init);
    insert_route(&mut router, "secure/forget/{token}", Command::SecureForget);
    router
}

pub(crate) fn miniserver_command_router() -> &'static matchit::Router<Command> {
    static ROUTER: OnceLock<matchit::Router<Command>> = OnceLock::new();
    ROUTER.get_or_init(build_miniserver_router)
}

#[cfg(test)]
mod tests {
    use super::{miniserver_command_router, Command};

    #[test]
    fn pairing_routes_resolve() {
        let router = miniserver_command_router();

        let hello = router.at("secure/hello/1779215299/LS0tY2VydA").unwrap();
        assert_eq!(hello.value, &Command::Hello);
        assert_eq!(hello.params.get("time"), Some("1779215299"));
        assert_eq!(hello.params.get("data"), Some("LS0tY2VydA"));

        let init = router.at("secure/init/eyJhbGciOiJub25lIn0").unwrap();
        assert_eq!(init.value, &Command::Init);
        assert_eq!(init.params.get("jwt"), Some("eyJhbGciOiJub25lIn0"));

        let forget = router.at("secure/forget/token123").unwrap();
        assert_eq!(forget.value, &Command::SecureForget);
        assert_eq!(forget.params.get("token"), Some("token123"));

        assert_eq!(
            router.at("secure/info/pairing").unwrap().value,
            &Command::InfoPairing
        );
    }

    #[test]
    fn config_handshake_routes_resolve() {
        let router = miniserver_command_router();

        assert_eq!(
            router.at("audio/cfg/getconfig").unwrap().value,
            &Command::GetConfig
        );
        let setconfig = router.at("audio/cfg/setconfig/abc123").unwrap();
        assert_eq!(setconfig.value, &Command::SetConfig);
        assert_eq!(setconfig.params.get("data"), Some("abc123"));

        assert_eq!(router.at("audio/cfg/ready").unwrap().value, &Command::Ready);

        let playername = router.at("audio/cfg/playername/dGVzdA").unwrap();
        assert_eq!(playername.value, &Command::PlayerName);
        assert_eq!(playername.params.get("data"), Some("dGVzdA"));

        let volumes = router.at("audio/cfg/volumes/eWVz").unwrap();
        assert_eq!(volumes.value, &Command::Volumes);
        assert_eq!(volumes.params.get("data"), Some("eWVz"));

        let speakertype = router.at("audio/cfg/speakertype/eWVz").unwrap();
        assert_eq!(speakertype.value, &Command::SpeakerType);
    }

    #[test]
    fn overlap_and_param_routes_resolve() {
        let router = miniserver_command_router();

        assert_eq!(
            router.at("audio/cfg/diagnosis").unwrap().value,
            &Command::Diagnosis
        );
        assert_eq!(
            router.at("audio/cfg/reboot").unwrap().value,
            &Command::Reboot
        );

        let time = router.at("audio/cfg/miniservertime/1779215298").unwrap();
        assert_eq!(time.value, &Command::MiniserverTime);
        assert_eq!(time.params.get("time"), Some("1779215298"));

        let tz = router.at("audio/cfg/timezone/Europe/Vienna").unwrap();
        assert_eq!(tz.value, &Command::Timezone);
        assert_eq!(tz.params.get("timezone"), Some("Europe/Vienna"));
    }

    /// One sample path per registered `Command` variant.
    #[test]
    fn all_command_variants_have_route_sample() {
        const ROUTE_SAMPLES: &[(&str, Command)] = &[
            ("audio/cfg/bluetooth/1/visible", Command::Bluetooth),
            ("audio/cfg/diagcmd", Command::DiagCmd),
            ("audio/cfg/diagcmd/patch:vrfy", Command::DiagCmd),
            ("audio/cfg/diagnosis", Command::Diagnosis),
            ("audio/cfg/forget", Command::Forget),
            ("audio/cfg/getconfig", Command::GetConfig),
            ("audio/cfg/getroomfavs/all", Command::GetRoomFavs),
            ("audio/cfg/groupopts/dGVzdA", Command::GroupOpts),
            ("audio/cfg/identify/off", Command::Identify),
            ("audio/cfg/miniservertime/1", Command::MiniserverTime),
            ("audio/cfg/miniserverip/192.168.0.1", Command::MiniserverIp),
            (
                "audio/cfg/miniserverversion/1.0/0.9",
                Command::MiniserverVersion,
            ),
            ("audio/cfg/presencemode/1/0", Command::PresenceMode),
            ("audio/cfg/playername/dGVzdA", Command::PlayerName),
            ("audio/cfg/playeropts/dGVzdA", Command::PlayerOpts),
            ("audio/cfg/ready", Command::Ready),
            ("audio/cfg/reboot", Command::Reboot),
            ("audio/cfg/setconfig/dGVzdA", Command::SetConfig),
            (
                "audio/cfg/setconfigtimestamp/1779215298",
                Command::SetConfigTimestamp,
            ),
            ("audio/cfg/speakertype/dGVzdA", Command::SpeakerType),
            ("audio/cfg/timezone/UTC", Command::Timezone),
            ("audio/cfg/volumes/dGVzdA", Command::Volumes),
            ("secure/hello/1/cert", Command::Hello),
            ("secure/info/pairing", Command::InfoPairing),
            ("secure/init/jwt", Command::Init),
            ("secure/forget/tok", Command::SecureForget),
        ];

        let router = miniserver_command_router();
        for (path, expected) in ROUTE_SAMPLES {
            let matched = router
                .at(path)
                .unwrap_or_else(|_| panic!("route not found: {path}"));
            assert_eq!(matched.value, expected, "path: {path}");
        }
    }
}
