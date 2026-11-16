//! 7095 Axum WebSocket server and Miniserver upstream session loop.

use std::{collections::HashMap, sync::Arc};

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::Response,
    routing::any,
    Router,
};
use tokio::sync::broadcast::error::RecvError;
use tokio::{
    net::TcpListener,
    sync::{broadcast, mpsc},
    time::{sleep, Duration},
};
use tracing::{debug, error, info, warn};

use crate::{
    app_config::AppConfig,
    config::AudioserverConfigFile,
    connect_bootstrap::{fetch_connect_player_states, resync_connect_audio_events, ConnectSurface},
    coordinator::HwChannelIds,
    loxone_config::{AudioserverConfig, SerialWithConfig, StereoExtension},
    player_state::PlayerState,
    remote::{format_customurl_changed_event, format_roomfav_changed_event, IoCommand, IoEvent},
};
use serde_json::json;

use super::{build_hw_events, Api, HardwareState};

#[derive(Clone)]
struct AppState {
    app_config: AppConfig,
    extensions: Vec<StereoExtension>,
    from_all: broadcast::Sender<IoEvent>,
    from_api: mpsc::Sender<IoCommand>,
    hw_channels: HwChannelIds,
}

pub async fn start(
    app_config: AppConfig,
    from_api: mpsc::Sender<IoCommand>,
    from_all: broadcast::Sender<IoEvent>,
    hw_channels: HwChannelIds,
) {
    if let Err(error) = serve(app_config, from_api, from_all, hw_channels).await {
        error!(port = 7095, api = "miniserver", error = %error, "miniserver api stopped");
    }
}

async fn serve(
    app_config: AppConfig,
    from_api: mpsc::Sender<IoCommand>,
    from_all: broadcast::Sender<IoEvent>,
    hw_channels: HwChannelIds,
) -> anyhow::Result<()> {
    debug!(port = 7095, api = "miniserver", "miniserver api listening");

    let mut base_serial = app_config.serial.clone();
    let _ = base_serial.pop();
    let mut extensions = vec![];
    for i in 0..9 {
        extensions.push(StereoExtension {
            blinkpos: 0,
            btenable: false,
            mac: format!("{base_serial}{i}"),
            serial: format!("{base_serial}{i}"),
            subtype: 1,
            extension_type: 3,
            version: app_config.version.clone(),
        });
    }

    tokio::spawn(generate_hw_events(from_all.clone(), hw_channels.clone()));

    let app_state = AppState {
        app_config,
        extensions,
        from_all,
        from_api,
        hw_channels,
    };

    let app = Router::new()
        .route("/", any(ws_handler))
        .with_state(app_state);

    let listener = TcpListener::bind("0.0.0.0:7095")
        .await
        .map_err(|e| anyhow::anyhow!("bind 0.0.0.0:7095: {e}"))?;

    axum::serve(listener, app)
        .await
        .map_err(|e| anyhow::anyhow!("axum serve 7095: {e}"))?;

    Ok(())
}

async fn ws_handler(ws: WebSocketUpgrade, State(app_state): State<AppState>) -> Response {
    ws.protocols(["miniserver"])
        .accept_unmasked_frames(true)
        .on_upgrade(|socket| handle_socket(socket, app_state))
}

async fn send_audio_events(socket: &mut WebSocket, states: &[PlayerState]) -> bool {
    for state in states {
        let payload = match serde_json::to_string(&json!({ "audio_event": [state] })) {
            Ok(p) => p,
            Err(e) => {
                warn!(error = %e, "failed to encode audio_event");
                continue;
            }
        };
        if socket.send(Message::from(payload)).await.is_err() {
            return false;
        }
    }
    true
}

fn default_serial_with_config(serial: &str, extensions: &[StereoExtension]) -> SerialWithConfig {
    [HashMap::from([(
        serial.to_string(),
        AudioserverConfig {
            extensions: extensions.to_vec(),
            ..Default::default()
        },
    )])]
}

fn config_extensions_from_serial(
    serial_with_config: &SerialWithConfig,
    serial: &str,
) -> Vec<StereoExtension> {
    serial_with_config
        .first()
        .and_then(|m| m.get(serial))
        .map(|c| c.extensions.clone())
        .unwrap_or_default()
}

async fn generate_hw_events(from_all: broadcast::Sender<IoEvent>, hw_channels: HwChannelIds) {
    loop {
        match build_hw_events(&hw_channels, &[]).await {
            Ok(events) if !events.is_empty() => {
                let _ = from_all.send(IoEvent::HardwareState(HardwareState { hw_event: events }));
            }
            Ok(_) => {}
            Err(e) => warn!(error = %e, "hw_event build failed"),
        }
        sleep(Duration::from_secs(60)).await;
    }
}

async fn handle_socket(mut socket: WebSocket, app_state: AppState) {
    info!(
        port = 7095,
        api = "miniserver",
        serial = %app_state.app_config.serial,
        "miniserver websocket connected"
    );
    let session_token = app_state.app_config.remote_session_token.clone();
    let msg = Message::from(format!(
        "MINISERVER V LWSS V {} {} | ~API:{}~ | Session-Token: {}",
        app_state.app_config.version,
        app_state.app_config.serial,
        app_state.app_config.api_version,
        session_token
    ));
    if socket.send(msg).await.is_err() {
        return;
    }

    let persisted = AudioserverConfigFile::get();
    let config_crc_hex = persisted.loxone_crc_hex().to_string();
    let serial_with_config = persisted.loxone_state().unwrap_or_else(|| {
        default_serial_with_config(&app_state.app_config.serial, &app_state.extensions)
    });
    let config_extensions =
        config_extensions_from_serial(&serial_with_config, &app_state.app_config.serial);

    // Initial hardware-event burst so outputs appear online immediately.
    if let Ok(events) = build_hw_events(&app_state.hw_channels, &config_extensions).await {
        let payload = serde_json::to_string(&HardwareState { hw_event: events }).unwrap();
        if socket.send(Message::from(payload)).await.is_err() {
            return;
        }
    }
    let connect_states = fetch_connect_player_states(
        &app_state.from_api,
        &app_state.app_config,
        ConnectSurface::Miniserver7095,
    )
    .await;
    if !send_audio_events(&mut socket, &connect_states).await {
        return;
    }
    let mut receiver = app_state.from_all.subscribe();
    let from_api = app_state.from_api.clone();
    let mut api = Api::new(
        app_state.from_api,
        serial_with_config,
        config_crc_hex,
        session_token.clone(),
        Arc::clone(&app_state.app_config.key_pair),
    );
    loop {
        tokio::select! {
            option = socket.recv() => {
                if let Some(result) = option {
                    if let Ok(message) = result {
                        match message {
                            Message::Text(utf8_bytes) => {
                                crate::command_audit::log_inbound(
                                    crate::command_audit::PORT_MINISERVER,
                                    &session_token,
                                    utf8_bytes.trim(),
                                );
                                match api.process_message(utf8_bytes).await {
                                    Ok(message) => {
                                        if socket.send(message).await.is_err() {
                                            return;
                                        }
                                    }
                                    Err(error) => {
                                        error!(port = 7095, api = "miniserver", error = %error, session_token, "miniserver command handler failed");
                                    },
                                }
                            },
                            Message::Close(close_frame) => {
                                info!(port = 7095, api = "miniserver", ?close_frame, session_token, "miniserver websocket closed");
                            },
                            _ => ()
                        }
                    } else {
                        info!(port = 7095, api = "miniserver", session_token, "miniserver connection dropped");
                        return;
                    }
                } else {
                    info!(port = 7095, api = "miniserver", session_token, "miniserver connection closed");
                    return;
                }
            },
            result = receiver.recv() => {
                match result {
                    Ok(io_event) => match io_event {
                        IoEvent::HardwareState(hardware_state) => {
                            let broadcast = serde_json::to_string(&hardware_state).unwrap();
                            if socket.send(Message::from(broadcast)).await.is_err() {
                                return;
                            }
                        },
                        IoEvent::ConfigUpdated(_state) => {},
                        IoEvent::State(state) => {
                            let broadcast = serde_json::to_string(&json!({
                                "audio_event": [state.clone()],
                            }))
                            .unwrap();
                            if socket.send(Message::from(broadcast)).await.is_err() {
                                return;
                            }
                        },
                        IoEvent::CustomUrlChanged => {
                            let broadcast = format_customurl_changed_event();
                            if socket.send(Message::from(broadcast)).await.is_err() {
                                return;
                            }
                        },
                        IoEvent::RoomFavChanged { playerid, count } => {
                            let broadcast = format_roomfav_changed_event(playerid, count);
                            if socket.send(Message::from(broadcast)).await.is_err() {
                                return;
                            }
                        },
                        _ => {}
                    },
                    Err(RecvError::Lagged(skipped)) => {
                        warn!(
                            skipped,
                            port = crate::command_audit::PORT_MINISERVER,
                            "state broadcast lagged; resyncing from registry via Status"
                        );
                        let payloads = resync_connect_audio_events(
                            &from_api,
                            &app_state.app_config,
                            ConnectSurface::Miniserver7095,
                        )
                        .await;
                        for broadcast in payloads {
                            if socket.send(Message::from(broadcast)).await.is_err() {
                                return;
                            }
                        }
                    }
                    Err(error) => {
                        warn!(port = 7095, api = "miniserver", error = %error, session_token, "miniserver event receiver failed");
                    },
                }
            }
        }
    }
}
