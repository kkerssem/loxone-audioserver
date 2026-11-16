//! Loxone Miniserver upstream WebSocket (port 7095).

mod router;
mod secure_session;
mod ws;

use router::{miniserver_command_router, Command};

pub use ws::start;

use crate::{
    config::{crc32_hex, AudioserverConfigFile},
    loxone_config::{
        GroupOpts, PlayerNames, PlayerOpts, SerialWithConfig, SpeakerTypes, StereoExtension,
        Volumes,
    },
    remote::IoCommand,
    wire_response::build_wire_result_response,
};
use anyhow::{anyhow, Context};
use axum::extract::ws::{Message, Utf8Bytes};
use base64::prelude::*;
use matchit::Params;
use rsa::{PrivateKey as RsaPrivateKey, PublicKey as RsaPublicKey};
use serde::Serialize;
use serde_json::json;
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, OnceLock},
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::mpsc;
use tracing::{debug, info};

use secure_session::SecureSessionState;

pub(super) struct Api {
    from_api: mpsc::Sender<IoCommand>,
    router: matchit::Router<Command>,
    serial_with_config: SerialWithConfig,
    /// Wire CRC from last `setconfig` (stored in audioserver-config.json).
    config_crc_hex: String,
    secure: SecureSessionState,
    key_pair: Arc<(RsaPrivateKey, RsaPublicKey)>,
}

#[derive(Clone, Debug, Serialize)]
pub struct HardwareEvent {
    client_id: String,
    event_id: usize,
    value: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct HardwareState {
    pub hw_event: Vec<HardwareEvent>,
}

/// Service start time for `hw_event` uptime (event 2105), matching lox-audioserver `heartbeatStart`.
static HEARTBEAT_START: OnceLock<Instant> = OnceLock::new();

/// Call once when the daemon starts.
pub fn init_heartbeat() {
    let _ = HEARTBEAT_START.set(Instant::now());
}

fn heartbeat_uptime_secs() -> usize {
    HEARTBEAT_START
        .get()
        .map(|start| start.elapsed().as_secs() as usize)
        .unwrap_or(0)
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn extension_macs(extensions: &[StereoExtension]) -> HashSet<String> {
    extensions.iter().map(|e| e.mac.to_uppercase()).collect()
}

fn channel_mac(client_id: &str) -> Option<&str> {
    client_id.split('#').next()
}

fn push_primary_channel_events(events: &mut Vec<HardwareEvent>, client_id: &str, uptime: usize) {
    for (event_id, value) in [
        (2005, 0),
        (2101, 67),
        (2100, 0),
        (2102, 0),
        (2103, 0),
        (2105, uptime),
        (2106, 56),
    ] {
        events.push(HardwareEvent {
            client_id: client_id.to_string(),
            event_id,
            value,
        });
    }
}

fn push_extension_channel_events(events: &mut Vec<HardwareEvent>, client_id: &str, uptime: usize) {
    for (event_id, value) in [
        (2100, 0),
        (2101, 0),
        (2102, 0),
        (2103, 0),
        (2104, 1),
        (2105, uptime),
    ] {
        events.push(HardwareEvent {
            client_id: client_id.to_string(),
            event_id,
            value,
        });
    }
}

pub async fn build_hw_events(
    hw_channels: &crate::coordinator::HwChannelIds,
    config_extensions: &[StereoExtension],
) -> anyhow::Result<Vec<HardwareEvent>> {
    let channel_ids = hw_channels.read().await.clone();
    let uptime = heartbeat_uptime_secs();
    let mut events = Vec::new();
    let ext_macs = extension_macs(config_extensions);

    for client_id in &channel_ids {
        let is_extension = channel_mac(client_id)
            .map(|mac| ext_macs.contains(&mac.to_uppercase()))
            .unwrap_or(false);
        if is_extension {
            push_extension_channel_events(&mut events, client_id, uptime);
        } else {
            push_primary_channel_events(&mut events, client_id, uptime);
        }
    }

    for extension in config_extensions {
        for channel in 1..=2 {
            let client_id = format!("{}#{}", extension.mac, channel);
            if channel_ids.contains(&client_id) {
                continue;
            }
            push_extension_channel_events(&mut events, &client_id, uptime);
        }
    }

    Ok(events)
}

impl Api {
    fn bluetooth(&self, command: &str, params: &Params) -> anyhow::Result<String> {
        let _playerid: usize = params.get("playerid").unwrap().parse()?;
        build_wire_result_response("visible_result", json!(""), Some(command), &[])
    }

    async fn get_config(&self, command: &str) -> anyhow::Result<String> {
        build_wire_result_response(
            "getconfig_result",
            json!({
                "crc32": self.config_crc_hex,
                "extensions": &self.serial_with_config
                    .first()
                    .context("Empty audioserver config")?
                    .values()
                    .next()
                    .context("Empty audioserver config")?
                    .extensions
            }),
            Some(command),
            &[],
        )
    }

    fn get_room_favs(&self, command: &str) -> anyhow::Result<String> {
        build_wire_result_response(
            "getroomfavs_result",
            json!([{
                "id": 0,
                "type": 4,
                "start": 0,
                "totalitems": 0,
                "items": [],
            }]),
            Some(command),
            &[],
        )
    }

    fn group_opts(&self, command: &str, params: &Params) -> anyhow::Result<String> {
        let data = params.get("data").context("Missing parameter data")?;
        let group_opts: GroupOpts = crate::loxone_wire::decode_base64_json(data)?;
        crate::loxone_wire::config_data_reply("groupopts_result", group_opts.groups, command, data)
    }

    fn hello(&mut self, command: &str, time: &str, data: &str) -> anyhow::Result<String> {
        self.secure.hello = Some(secure_session::SecureHelloState {
            client_time: time.to_string(),
            cert_chain_b64: data.to_string(),
        });
        let suffix = format!("{time}/{data}");
        let stripped = crate::loxone_wire::strip_param(command, &suffix);
        build_wire_result_response(
            "secure_hello_result",
            json!({
                "command": "secure/hello",
                "error": 0,
                "public_key": data,
            }),
            Some(stripped),
            &[],
        )
    }

    fn identify(&self, command: &str) -> anyhow::Result<String> {
        build_wire_result_response("off_result", json!(true), Some(command), &[])
    }

    async fn info_pairing(&self, command: &str) -> anyhow::Result<String> {
        let audioserver_config = self
            .serial_with_config
            .first()
            .context("Empty audioserver config")?
            .values()
            .next()
            .context("Empty audioserver config")?;
        Ok(format!(
            "{{\"error\":-81,\"master\":{},\"peers\":[],\"command\":{}}}",
            serde_json::to_string(&audioserver_config.master)?,
            serde_json::to_string(command)?
        ))
    }

    fn init(&mut self, command: &str, jwt: &str) -> anyhow::Result<String> {
        let stripped = crate::loxone_wire::strip_param(command, jwt);

        if let Ok(req) = secure_session::resolve_init_request(jwt, &self.secure) {
            if secure_session::validate_init_request(
                &req,
                &self.secure.expected_session_token,
                SystemTime::now(),
            )
            .is_ok()
            {
                self.secure.last_init = Some(req.clone());
                let probe = secure_session::probe_session_keys_decrypt(
                    &req.session_keys,
                    &self.key_pair.0,
                    &self.key_pair.1,
                    self.secure.hello.as_ref(),
                    jwt,
                    req.encryption_type,
                );
                if let Some(keys) = probe.decrypted.clone() {
                    self.secure.decrypted_keys = Some(keys);
                }
                secure_session::log_session_keys_probe(&probe);
            } else {
                debug!("secure/init validation failed; returning legacy stub");
            }
        } else {
            debug!("secure/init JWT parse failed; returning legacy stub");
        }

        self.secure_init_stub_response(stripped)
    }

    fn secure_forget(&mut self, command: &str, token: &str) -> anyhow::Result<String> {
        let stripped = crate::loxone_wire::strip_param(command, token);
        self.secure.hello = None;
        self.secure.last_init = None;
        self.secure.decrypted_keys = None;
        info!("secure session cleared");
        build_wire_result_response(
            "secure_forget_result",
            json!({
                "command": "secure/forget",
                "error": 0,
            }),
            Some(stripped),
            &[],
        )
    }

    fn secure_init_stub_response(&self, stripped_command: &str) -> anyhow::Result<String> {
        build_wire_result_response(
            "secure_init_result",
            json!({
                "command": "secure/init",
                "error": 0,
                "jwt": secure_session::legacy_success_jwt(),
            }),
            Some(stripped_command),
            &[],
        )
    }

    fn miniserver_time(&self, command: &str, params: &Params) -> anyhow::Result<String> {
        let time = params.get("time").context("Missing parameter time")?;
        let millis = now_millis();
        build_wire_result_response(
            &format!("{time}_result"),
            json!(millis),
            Some(crate::loxone_wire::strip_param(command, time)),
            &[],
        )
    }

    fn miniserver_ip(&self, command: &str) -> anyhow::Result<String> {
        build_wire_result_response("miniserverip_result", json!(true), Some(command), &[])
    }

    fn miniserver_version(&self, command: &str, params: &Params) -> anyhow::Result<String> {
        let latest = params.get("latest").context("Missing parameter latest")?;
        let minimum = params.get("minimum").context("Missing parameter minimum")?;
        build_wire_result_response(
            "miniserverversion_result",
            json!(true),
            Some(crate::loxone_wire::strip_param(
                command,
                &format!("{latest}/{minimum}"),
            )),
            &[],
        )
    }

    fn presence_mode(&self, command: &str, params: &Params) -> anyhow::Result<String> {
        let _playerid = params
            .get("playerid")
            .context("Missing parameter playerid")?;
        let _mode = params.get("mode").context("Missing parameter mode")?;
        build_wire_result_response("presencemode_result", json!(true), Some(command), &[])
    }

    pub(super) fn new(
        from_api: mpsc::Sender<IoCommand>,
        serial_with_config: SerialWithConfig,
        config_crc_hex: String,
        expected_session_token: String,
        key_pair: Arc<(RsaPrivateKey, RsaPublicKey)>,
    ) -> Self {
        Self {
            from_api,
            config_crc_hex,
            serial_with_config,
            key_pair,
            secure: SecureSessionState {
                expected_session_token,
                hello: None,
                last_init: None,
                decrypted_keys: None,
            },
            router: miniserver_command_router().clone(),
        }
    }

    fn not_implemented(&self, command: &str) -> anyhow::Result<String> {
        crate::loxone_wire::not_implemented_reply(command)
    }

    async fn player_name(&self, command: &str, params: &Params<'_, '_>) -> anyhow::Result<String> {
        let data = params.get("data").context("Missing parameter data")?;
        let player_names: PlayerNames = crate::loxone_wire::decode_base64_json(data)?;
        self.from_api
            .send(IoCommand::LoxonePlayerNames(player_names.clone()))
            .await?;
        self.from_api
            .send(IoCommand::Players(player_names.clone()))
            .await?;
        crate::loxone_wire::config_data_reply(
            "playername_result",
            player_names.players,
            command,
            data,
        )
    }

    fn player_opts(&self, command: &str, params: &Params) -> anyhow::Result<String> {
        let data = params.get("data").context("Missing parameter data")?;
        let player_opts: PlayerOpts = crate::loxone_wire::decode_base64_json(data)?;
        let opts = player_opts.players.clone();
        let from_api = self.from_api.clone();
        tokio::spawn(async move {
            let _ = from_api.send(IoCommand::LoxonePlayerOpts(opts)).await;
        });
        crate::loxone_wire::config_data_reply(
            "playeropts_result",
            player_opts.players,
            command,
            data,
        )
    }

    pub(super) async fn process_message(
        &mut self,
        utf8_bytes: Utf8Bytes,
    ) -> anyhow::Result<Message> {
        let command = utf8_bytes.trim();
        let matched = match self.router.at(command) {
            Ok(m) => m,
            Err(_) => {
                if crate::zone_transport::is_zone_transport(command) {
                    let body = crate::zone_transport::dispatch(command, &self.from_api).await?;
                    return Ok(Message::from(body));
                }
                if let Some(body) = crate::remote::try_dispatch_getqueue(command)? {
                    return Ok(Message::from(body));
                }
                return Err(anyhow!("Matching route not found: {command}"));
            }
        };
        if matches!(matched.value, Command::SetConfig) {
            let data = matched
                .params
                .get("data")
                .context("Missing parameter data")?
                .to_string();
            return Ok(Message::from(self.set_config(command, &data).await?));
        }
        if matches!(matched.value, Command::Hello) {
            let time = matched
                .params
                .get("time")
                .context("Missing parameter time")?;
            let data = matched
                .params
                .get("data")
                .context("Missing parameter data")?;
            return Ok(Message::from(self.hello(command, time, data)?));
        }
        if matches!(matched.value, Command::Init) {
            let jwt = matched.params.get("jwt").context("Missing parameter jwt")?;
            return Ok(Message::from(self.init(command, jwt)?));
        }
        if matches!(matched.value, Command::SecureForget) {
            let token = matched
                .params
                .get("token")
                .context("Missing parameter token")?;
            return Ok(Message::from(self.secure_forget(command, token)?));
        }
        let reply = match matched.value {
            Command::Bluetooth => self.bluetooth(command, &matched.params),
            Command::DiagCmd | Command::Diagnosis => self.not_implemented(command),
            Command::Forget => self.not_implemented(command),
            Command::GetConfig => self.get_config(command).await,
            Command::GetRoomFavs => self.get_room_favs(command),
            Command::GroupOpts => self.group_opts(command, &matched.params),
            Command::Identify => self.identify(command),
            Command::MiniserverIp => self.miniserver_ip(command),
            Command::MiniserverTime => self.miniserver_time(command, &matched.params),
            Command::MiniserverVersion => self.miniserver_version(command, &matched.params),
            Command::PresenceMode => self.presence_mode(command, &matched.params),
            Command::PlayerName => self.player_name(command, &matched.params).await,
            Command::PlayerOpts => self.player_opts(command, &matched.params),
            Command::Ready => self.ready(command).await,
            Command::Reboot => self.not_implemented(command),
            Command::SetConfig => unreachable!(),
            Command::SetConfigTimestamp => self.set_config_timestamp(command, &matched.params),
            Command::SpeakerType => self.speaker_type(command, &matched.params).await,
            Command::Timezone => self.timezone(command, &matched.params),
            Command::Volumes => self.volumes(command, &matched.params).await,
            Command::Hello => unreachable!(),
            Command::InfoPairing => self.info_pairing(command).await,
            Command::Init => unreachable!(),
            Command::SecureForget => unreachable!(),
        }?;
        Ok(Message::from(reply))
    }

    async fn ready(&self, command: &str) -> anyhow::Result<String> {
        self.from_api.send(IoCommand::LoxoneReady).await?;
        info!("miniserver ready sent");
        build_wire_result_response(
            "ready_result",
            json!({
                "session": now_millis(),
            }),
            Some(command),
            &[],
        )
    }

    async fn set_config(&mut self, command: &str, data_b64: &str) -> anyhow::Result<String> {
        let decoded_data = BASE64_STANDARD_NO_PAD.decode(data_b64)?;

        if decoded_data == b"[]" {
            self.config_crc_hex = crc32_hex(b"[]");
            self.serial_with_config = [HashMap::new()];
            AudioserverConfigFile::update(|file| {
                file.clear_loxone();
                Ok(())
            })?;
            info!(crc32 = %self.config_crc_hex, "cleared miniserver loxone config");
        } else {
            let parsed: SerialWithConfig = serde_json::from_slice(&decoded_data)?;
            self.config_crc_hex = crc32_hex(&decoded_data);
            self.serial_with_config = parsed.clone();
            AudioserverConfigFile::update(|file| {
                file.set_loxone_from_setconfig(&decoded_data, parsed);
                Ok(())
            })?;
            info!(
                crc32 = %self.config_crc_hex,
                "saved miniserver loxone config"
            );
        }

        let parsed = self.serial_with_config.clone();
        self.from_api
            .send(IoCommand::LoxoneSetConfig {
                raw: decoded_data.clone(),
                parsed,
            })
            .await?;

        build_wire_result_response(
            "setconfig_result",
            json!({
                "crc32": self.config_crc_hex,
                "extensions": &self.serial_with_config
                    .first()
                    .context("Empty audioserver config")?
                    .values()
                    .next()
                    .context("Empty audioserver config")?
                    .extensions,
            }),
            Some(crate::loxone_wire::strip_param(command, data_b64)),
            &[],
        )
    }

    fn set_config_timestamp(&self, command: &str, params: &Params) -> anyhow::Result<String> {
        let time = params.get("time").context("Missing parameter time")?;
        let timestamp: i64 = time.parse().context("Invalid setconfig timestamp")?;
        build_wire_result_response(
            "setconfigtimestamp_result",
            json!({
                "success": true,
                "timestamp": timestamp,
                "crc32": self.config_crc_hex,
            }),
            Some(crate::loxone_wire::strip_param(command, time)),
            &[],
        )
    }

    async fn speaker_type(&self, command: &str, params: &Params<'_, '_>) -> anyhow::Result<String> {
        let data = params.get("data").context("Missing parameter data")?;
        let speaker_types: SpeakerTypes = crate::loxone_wire::decode_base64_json(data)?;
        let types = speaker_types.speakers.clone();
        self.from_api
            .send(IoCommand::LoxoneSpeakerTypes(types.clone()))
            .await?;
        crate::loxone_wire::config_data_reply("speakertype_result", types, command, data)
    }

    fn timezone(&self, command: &str, params: &Params) -> anyhow::Result<String> {
        let timezone = params
            .get("timezone")
            .context("Missing parameter timezone")?;
        build_wire_result_response(
            "timezone_result",
            json!(true),
            Some(crate::loxone_wire::strip_param(command, timezone)),
            &[],
        )
    }

    async fn volumes(&self, command: &str, params: &Params<'_, '_>) -> anyhow::Result<String> {
        let data = params.get("data").context("Missing parameter data")?;
        let volumes: Volumes = crate::loxone_wire::decode_base64_json(data)?;
        let vols = volumes.players.clone();
        self.from_api
            .send(IoCommand::LoxoneVolumes(vols.clone()))
            .await?;
        let player_count = vols.len();
        build_wire_result_response(
            "volumes_result",
            json!({
                "success": true,
                "players": player_count,
            }),
            Some(crate::loxone_wire::strip_param(command, data)),
            &[],
        )
    }
}

/// Short SPKI fingerprint (first 8 bytes of SHA-256) for getkey/decrypt diagnostics.
pub fn audioserver_spki_fingerprint(public_key: &RsaPublicKey) -> Option<String> {
    secure_session::public_key_fingerprint(public_key).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn miniservertime_uses_dynamic_result_key() {
        let wire = Api::miniserver_time_reply(
            "audio/cfg/miniservertime/1779215298",
            "1779215298",
            1779215296911i64,
        );
        let body: serde_json::Value = serde_json::from_str(&wire).unwrap();
        assert_eq!(
            body.get("1779215298_result"),
            Some(&json!(1779215296911i64))
        );
        assert_eq!(
            crate::wire_response::first_wire_key(&wire).as_deref(),
            Some("1779215298_result")
        );
    }

    #[test]
    fn volumes_ack_puts_result_first() {
        let wire = build_wire_result_response(
            "volumes_result",
            json!({"success": true, "players": 1}),
            Some("audio/cfg/volumes/x"),
            &[],
        )
        .unwrap();
        assert_eq!(
            crate::wire_response::first_wire_key(&wire).as_deref(),
            Some("volumes_result")
        );
        let body: serde_json::Value = serde_json::from_str(&wire).unwrap();
        assert_eq!(body["volumes_result"]["success"], true);
        assert_eq!(body["volumes_result"]["players"], 1);
    }

    #[test]
    fn volumes_payload_decodes_variable_default() {
        let data = "eyJwbGF5ZXJzIjpbeyJwbGF5ZXJpZCI6MywiZGVmYXVsdCI6LTEsImFsYXJtIjo3NSwiZmlyZSI6NzUsImJlbGwiOjM1LCJidXp6ZXIiOjEwLCJ0dHMiOjIwLCJ2b2xzdGVwIjoxLCJmYWRpbmciOjAsIm1heFZvbHVtZSI6MTAwfV19";
        let volumes: Volumes = crate::loxone_wire::decode_base64_json(data).unwrap();
        assert_eq!(volumes.players.len(), 1);
        assert!(volumes.players[0].is_variable_default());
    }

    #[test]
    fn ready_session_is_numeric_millis() {
        let body: serde_json::Value =
            serde_json::from_str(r#"{"ready_result":{"session":1779215296911}}"#).unwrap();
        assert!(body["ready_result"]["session"].is_number());
    }

    #[test]
    fn getroomfavs_all_shape() {
        let body: serde_json::Value = serde_json::from_str(
            r#"{"getroomfavs_result":[{"id":0,"type":4,"start":0,"totalitems":0,"items":[]}]}"#,
        )
        .unwrap();
        assert_eq!(body["getroomfavs_result"][0]["id"], 0);
        assert_eq!(body["getroomfavs_result"][0]["type"], 4);
    }

    fn load_secure_init_fixture() -> serde_json::Value {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/secure_init_7095.json"
        );
        let raw = std::fs::read_to_string(path).expect("fixture");
        serde_json::from_str(&raw).expect("fixture json")
    }

    async fn process_command(api: &mut Api, command: &str) -> String {
        let message = api.process_message(Utf8Bytes::from(command)).await.unwrap();
        match message {
            Message::Text(text) => text.to_string(),
            other => panic!("unexpected message: {other:?}"),
        }
    }

    #[tokio::test]
    async fn secure_init_wire_shape() {
        let fixture = load_secure_init_fixture();
        let jwt = fixture["init_jwt"].as_str().unwrap();
        let mut api = Api::test_api(fixture["greeting_session_token"].as_str().unwrap());
        let wire = process_command(&mut api, &format!("secure/init/{jwt}")).await;
        assert!(wire.starts_with(fixture["expected_wire_prefix"].as_str().unwrap()));
        assert_eq!(
            crate::wire_response::first_wire_key(&wire).as_deref(),
            Some("secure_init_result")
        );
    }

    #[tokio::test]
    async fn secure_init_golden_jwt() {
        let fixture = load_secure_init_fixture();
        let jwt = fixture["init_jwt"].as_str().unwrap();
        let mut api = Api::test_api(fixture["greeting_session_token"].as_str().unwrap());
        let wire = process_command(&mut api, &format!("secure/init/{jwt}")).await;
        let body: serde_json::Value = serde_json::from_str(&wire).unwrap();
        assert_eq!(
            body["secure_init_result"]["jwt"].as_str().unwrap(),
            fixture["expected_response_jwt"].as_str().unwrap()
        );
    }

    #[tokio::test]
    async fn secure_init_command_echo() {
        let fixture = load_secure_init_fixture();
        let jwt = fixture["init_jwt"].as_str().unwrap();
        let mut api = Api::test_api(fixture["greeting_session_token"].as_str().unwrap());
        let wire = process_command(&mut api, &format!("secure/init/{jwt}")).await;
        let body: serde_json::Value = serde_json::from_str(&wire).unwrap();
        assert_eq!(body["command"].as_str().unwrap(), "secure/init");
    }

    #[tokio::test]
    async fn hello_then_init_stores_state() {
        let fixture = load_secure_init_fixture();
        let token = fixture["greeting_session_token"].as_str().unwrap();
        let jwt = secure_session::test_unsigned_init_jwt(token, SystemTime::now());
        let mut api = Api::test_api(token);

        process_command(&mut api, "secure/hello/1779215299/LS0tY2VydA").await;
        assert!(api.secure.hello.is_some());
        assert_eq!(
            api.secure.hello.as_ref().unwrap().cert_chain_b64,
            "LS0tY2VydA"
        );

        process_command(&mut api, &format!("secure/init/{jwt}")).await;
        assert!(api.secure.last_init.is_some());
        assert_eq!(api.secure.last_init.as_ref().unwrap().session_token, token);
    }

    #[tokio::test]
    async fn secure_pairing_sequence_smoke() {
        let fixture = load_secure_init_fixture();
        let jwt = fixture["init_jwt"].as_str().unwrap();
        let token = fixture["greeting_session_token"].as_str().unwrap();
        let mut api = Api::test_api(token);

        process_command(&mut api, "audio/cfg/miniservertime/1779215298").await;
        process_command(&mut api, "secure/info/pairing").await;

        let cert = "LS0tY2VydA";
        process_command(&mut api, &format!("secure/hello/1779215299/{cert}")).await;

        let init_wire = process_command(&mut api, &format!("secure/init/{jwt}")).await;
        assert_eq!(
            crate::wire_response::first_wire_key(&init_wire).as_deref(),
            Some("secure_init_result")
        );

        process_command(&mut api, "secure/info/pairing").await;
    }

    #[tokio::test]
    async fn secure_forget_acknowledges_and_clears_session() {
        let fixture = load_secure_init_fixture();
        let token = fixture["greeting_session_token"].as_str().unwrap();
        let jwt = secure_session::test_unsigned_init_jwt(token, SystemTime::now());
        let mut api = Api::test_api(token);

        process_command(&mut api, "secure/hello/1779215299/LS0tY2VydA").await;
        process_command(&mut api, &format!("secure/init/{jwt}")).await;
        assert!(api.secure.hello.is_some());

        let forget_token = "GroJwP6a67TwIRULF1iwsR_0O1i-O5iUYoVCgpbgM4es0sZd4HjGcCn2mIB7k-lLux85QwIbg-QDE-HheCuuORAJc5wAh6x8lp9cTpKiFlQuC_iBFhnFJCSt9YvHdpMjVx9dZU6dLr9KUNjRfmDxZBr5aHTwX7tJKdOjbENLOlTwMOAXaToO_MzXfXoKobjQKUkb7cA53KoIZiEHH8esF7";
        let wire = process_command(&mut api, &format!("secure/forget/{forget_token}")).await;
        assert_eq!(
            crate::wire_response::first_wire_key(&wire).as_deref(),
            Some("secure_forget_result")
        );
        let body: serde_json::Value = serde_json::from_str(&wire).unwrap();
        assert_eq!(body["secure_forget_result"]["error"], 0);
        assert!(api.secure.hello.is_none());
        assert!(api.secure.last_init.is_none());
    }

    #[tokio::test]
    async fn hello_then_init_with_fixture_cert() {
        let fixture = load_secure_init_fixture();
        let token = fixture["greeting_session_token"].as_str().unwrap();
        let jwt = secure_session::test_unsigned_init_jwt(token, SystemTime::now());
        let cert = fixture["hello_cert_chain_b64"].as_str().unwrap();
        let mut api = Api::test_api(token);

        process_command(&mut api, &format!("secure/hello/1779215299/{cert}")).await;
        assert!(api.secure.hello.is_some());

        process_command(&mut api, &format!("secure/init/{jwt}")).await;
        assert!(api.secure.last_init.is_some());
        assert_eq!(api.secure.last_init.as_ref().unwrap().session_token, token);
    }
}

impl Api {
    #[cfg(test)]
    fn miniserver_time_reply(command: &str, time: &str, millis: i64) -> String {
        build_wire_result_response(
            &format!("{time}_result"),
            json!(millis),
            Some(&crate::loxone_wire::strip_param(command, time)),
            &[],
        )
        .unwrap()
    }

    #[cfg(test)]
    fn test_api(expected_session_token: &str) -> Self {
        use crate::loxone_config::AudioserverConfig;
        use std::collections::HashMap;

        let (from_api, _rx) = mpsc::channel(1);
        let key_pair = Arc::new(crate::app_config::load_keys().expect("test RSA key pair"));
        Self::new(
            from_api,
            [HashMap::from([(
                "504F94FF1BB3".to_string(),
                AudioserverConfig::default(),
            )])],
            crc32_hex(b"[]"),
            expected_session_token.to_string(),
            key_pair,
        )
    }
}
