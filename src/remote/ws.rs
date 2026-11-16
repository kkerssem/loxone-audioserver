//! 7091 Axum HTTP server and WebSocket session loop.

use anyhow::Context;
use axum::{
    body::{Body, Bytes},
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        DefaultBodyLimit, Path, Query, State,
    },
    http::{header, HeaderMap, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{any, get, put},
    Router,
};
use rand::distr::{Alphanumeric, SampleString};
use reqwest::Client;
use serde_json::json;
use tokio::sync::broadcast::error::RecvError;
use tokio::{
    net::TcpListener,
    sync::{broadcast, mpsc},
};
use tower_http::cors::{Any, CorsLayer};
use tracing::{debug, error, info, warn};

use crate::connect_bootstrap::{resync_connect_audio_events, ConnectSurface};
use crate::radio::{CustomRadioError, CustomRadioStore};
use crate::voice_upload;
use crate::AppConfig;

use super::{Api, AppState, IoCommand, IoEvent};

pub async fn start(
    app_config: AppConfig,
    from_api: mpsc::Sender<IoCommand>,
    from_roon: broadcast::Sender<IoEvent>,
) {
    if let Err(error) = serve(app_config, from_api, from_roon).await {
        error!(port = 7091, api = "remote", error = %error, "remote api stopped");
    }
}

async fn serve(
    app_config: AppConfig,
    from_api: mpsc::Sender<IoCommand>,
    from_roon: broadcast::Sender<IoEvent>,
) -> anyhow::Result<()> {
    debug!(port = 7091, api = "remote", "remote api listening");

    let cors = CorsLayer::new()
        .allow_methods([Method::GET, Method::PUT, Method::OPTIONS])
        .allow_headers([header::CONTENT_TYPE, header::ACCEPT])
        .allow_origin(Any);

    let app_state = AppState {
        app_config,
        client: Client::new(),
        from_roon: from_roon.clone(),
        from_api,
    };

    let app = Router::new()
        .route("/", any(ws_handler))
        .route("/api/image/{image_key}", get(image_handler))
        .route("/cors-proxy", get(cors_proxy_handler))
        .route(
            "/audio/cfg/radios/coverurl/{id}",
            get(radio_cover_get).put(radio_cover_put),
        )
        .route(
            "/audio/cfg/upload/audioupload/add/{file}",
            put(voice_upload_put).layer(DefaultBodyLimit::max(voice_upload::MAX_UPLOAD_BYTES)),
        )
        .route(
            &format!("{}/{{file}}", voice_upload::SERVE_PREFIX),
            get(voice_upload_get),
        )
        .fallback(unmatched_http)
        .layer(cors)
        .with_state(app_state);

    let listener = TcpListener::bind("0.0.0.0:7091")
        .await
        .context("bind 0.0.0.0:7091")?;

    axum::serve(listener, app)
        .await
        .context("axum serve 7091")?;

    Ok(())
}

#[axum::debug_handler]
async fn ws_handler(ws: WebSocketUpgrade, State(app_state): State<AppState>) -> Response {
    ws.protocols(["remotecontrol"])
        .accept_unmasked_frames(true)
        .on_upgrade(|socket| handle_socket(socket, app_state))
}

#[derive(serde::Deserialize)]
struct CorsProxyQuery {
    url: String,
    #[serde(default)]
    id: String,
}

fn is_allowed_proxy_url(url: &str) -> bool {
    let trimmed = url.trim();
    trimmed.starts_with("http://") || trimmed.starts_with("https://")
}

fn cors_error_response(status: StatusCode) -> Response {
    Response::builder()
        .status(status)
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .body(Body::empty())
        .unwrap()
        .into_response()
}

fn guess_image_content_type(url: &str) -> Option<&'static str> {
    let lower = url.to_lowercase();
    if lower.ends_with(".png") {
        Some("image/png")
    } else if lower.ends_with(".webp") {
        Some("image/webp")
    } else if lower.ends_with(".gif") {
        Some("image/gif")
    } else if lower.contains("qobuz.com") || lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        Some("image/jpeg")
    } else {
        None
    }
}

/// Upstream URL for the cors-proxy: `url` with `_=<cache_bust>` appended to its query.
/// Stores a voice recording the App uploads before `audio/grouped/playuploadedfile/...`.
/// `Path` has already percent-decoded `file`; the play command decodes its name the same way.
async fn voice_upload_put(Path(file): Path<String>, body: Bytes) -> StatusCode {
    let dir = std::path::Path::new(voice_upload::UPLOAD_DIR);
    match voice_upload::store(dir, &file, &body).await {
        Ok(path) => {
            info!(
                port = 7091,
                api = "remote",
                file = %path.display(),
                bytes = body.len(),
                "remote api voice upload stored"
            );
            StatusCode::OK
        }
        Err(error) => {
            warn!(port = 7091, api = "remote", file = %file, error = %error, "remote api voice upload rejected");
            StatusCode::BAD_REQUEST
        }
    }
}

/// Serves a stored voice recording to the Cast device that plays it.
async fn voice_upload_get(Path(file): Path<String>) -> Response {
    let dir = std::path::Path::new(voice_upload::UPLOAD_DIR);
    match voice_upload::read(dir, &file).await {
        Ok(bytes) => ([(header::CONTENT_TYPE, "audio/wav")], bytes).into_response(),
        Err(error) => {
            debug!(port = 7091, api = "remote", file = %file, error = %error, "voice upload not served");
            StatusCode::NOT_FOUND.into_response()
        }
    }
}

/// Logs HTTP requests no 7091 route handles (at debug: browsers and LAN scanners hit this) and
/// answers 404, so an unsupported App upload can be found with `RUST_LOG=…=debug`.
async fn unmatched_http(method: Method, uri: Uri, headers: HeaderMap) -> StatusCode {
    let header_str = |name: header::HeaderName| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string()
    };
    debug!(
        port = 7091,
        api = "remote",
        method = %method,
        path = %uri.path(),
        has_query = uri.query().is_some(),
        content_type = %header_str(header::CONTENT_TYPE),
        content_length = %header_str(header::CONTENT_LENGTH),
        "remote api unhandled http request"
    );
    StatusCode::NOT_FOUND
}

fn proxy_fetch_url(url: &str, cache_bust: &str) -> String {
    let separator = if url.contains('?') { '&' } else { '?' };
    format!("{url}{separator}_={}", urlencoding::encode(cache_bust))
}

#[axum::debug_handler]
async fn cors_proxy_handler(
    Query(query): Query<CorsProxyQuery>,
    State(app_state): State<AppState>,
) -> Response {
    if query.url.is_empty() || !is_allowed_proxy_url(&query.url) {
        return cors_error_response(StatusCode::BAD_REQUEST);
    }
    let cache_bust = if query.id.is_empty() {
        "x".to_string()
    } else {
        query.id
    };
    let fetch_url = proxy_fetch_url(&query.url, &cache_bust);
    debug!(%fetch_url, "cors-proxy fetch");
    let reqwest_response = match app_state.client.get(&fetch_url).send().await {
        Ok(response) => response,
        Err(error) => {
            error!(error = %error, "cors-proxy request failed");
            return cors_error_response(StatusCode::BAD_GATEWAY);
        }
    };
    let status = reqwest_response.status();
    if !status.is_success() {
        warn!(%status, %fetch_url, "cors-proxy upstream non-success");
    }
    let mut response_builder = Response::builder().status(status);
    if let Some(headers) = response_builder.headers_mut() {
        *headers = reqwest_response.headers().clone();
        headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*".parse().unwrap());
        if !headers.contains_key(header::CONTENT_TYPE) {
            if let Some(ct) = guess_image_content_type(&query.url) {
                headers.insert(header::CONTENT_TYPE, ct.parse().unwrap());
            }
        }
    }
    response_builder
        .body(Body::from_stream(reqwest_response.bytes_stream()))
        .unwrap()
        .into_response()
}

#[axum::debug_handler]
async fn radio_cover_put(
    Path(id): Path<String>,
    State(app_state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let id = urlencoding::decode(&id)
        .map(|s| s.into_owned())
        .unwrap_or(id);
    if body.is_empty() {
        return radio_cover_error(
            StatusCode::BAD_REQUEST,
            CustomRadioError::MissingPayload,
            "empty cover payload",
        );
    }
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("image/jpeg");
    let store = CustomRadioStore::new(None);
    match store
        .set_cover(
            &id,
            &body,
            content_type,
            &app_state.app_config.cover_proxy_base(),
        )
        .await
    {
        Ok(Some(_)) => {
            let _ = app_state.from_roon.send(IoEvent::CustomUrlChanged);
            (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "application/json")],
                json!({}).to_string(),
            )
                .into_response()
        }
        Ok(None) => radio_cover_error(
            StatusCode::NOT_FOUND,
            CustomRadioError::FileNotFound,
            "station not found",
        ),
        Err(error) => {
            warn!(error = %error, station_id = %id, "custom radio cover upload failed");
            radio_cover_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                CustomRadioError::FileCreationFailed,
                &error.to_string(),
            )
        }
    }
}

#[axum::debug_handler]
async fn radio_cover_get(Path(id): Path<String>, State(_app_state): State<AppState>) -> Response {
    let id = urlencoding::decode(&id)
        .map(|s| s.into_owned())
        .unwrap_or(id);
    let store = CustomRadioStore::new(None);
    match store.read_cover(&id).await {
        Ok(Some((bytes, content_type))) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, content_type)],
            bytes,
        )
            .into_response(),
        Ok(None) => radio_cover_error(
            StatusCode::NOT_FOUND,
            CustomRadioError::FileNotFound,
            "cover not found",
        ),
        Err(error) => {
            warn!(error = %error, station_id = %id, "custom radio cover read failed");
            radio_cover_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                CustomRadioError::Other,
                &error.to_string(),
            )
        }
    }
}

fn radio_cover_error(status: StatusCode, error: CustomRadioError, error_info: &str) -> Response {
    (
        status,
        [(header::CONTENT_TYPE, "application/json")],
        json!({
            "error": error.code(),
            "errorInfo": error_info,
        })
        .to_string(),
    )
        .into_response()
}

#[axum::debug_handler]
async fn image_handler(
    Path(image_key): Path<String>,
    State(app_state): State<AppState>,
) -> Response {
    /*
     * http://IP:PORT/api/image/image_key?scale=XXX&width=XXX&height=XXX&format=XXX</tt>
     *
     * The rules for parameter types and optional/required follow the same as for the options parameter.
     *
     * @param {string} image_key - the key, as given by another api
     * @param {('fit'|'fill'|'stretch')} scale - If this is not set, the image will be returned at original size. Warning: that could be very large!
     * @param {number} width - The image width to be returned. Required if the scale property is set
     * @param {number} height - The image height to be returned. Required if the scale property is set
     * @param {('image/jpeg'|'image/png')} format - If you don't specify, Roon will choose for you.
     *
     * Take the original url and serve it using the cors-proxy. This prevents cors issues.
     * Also, airplay image names dont change, so Loxone clients think its the same image and never reload the image.
     * We are using a unique querystring (cacheBusting), to make sure the image is updated every time the track changes.
     * coverurl: `http://${config.audioserver?.ip}:7091/cors-proxy?url=${encodeURIComponent(data.trackImage?.[0]?.url || '')}&id=${encodeURIComponent(data.album || 'x')}`
     */
    debug!(
        port = 7091,
        api = "remote",
        image_key,
        "cover image request"
    );

    let url = format!(
        "http://192.168.70.22:9330/api/image/{}?scale=fit&width=400&height=400",
        image_key
    );
    let reqwest_response = match app_state.client.get(url).send().await {
        Ok(response) => response,
        Err(error) => {
            error!(error = %error, image_key, "cover image request failed");
            return (StatusCode::BAD_REQUEST, Body::empty()).into_response();
        }
    };

    let mut response_builder = Response::builder().status(reqwest_response.status());
    *response_builder.headers_mut().unwrap() = reqwest_response.headers().clone();
    response_builder
        .body(Body::from_stream(reqwest_response.bytes_stream()))
        // This unwrap is fine because the body is empty here
        .unwrap()
}

/// Push live zone state immediately after 7091 connect (authoritative registry via Status).
async fn push_initial_audio_events(
    socket: &mut WebSocket,
    app_config: &AppConfig,
    from_api: mpsc::Sender<IoCommand>,
) {
    use crate::connect_bootstrap::{fetch_connect_player_states, ConnectSurface};
    use crate::zone_registry::encode_audio_event_payload;

    let states =
        fetch_connect_player_states(&from_api, app_config, ConnectSurface::Remote7091).await;
    for state in states {
        let player_id = state.playerid.0;
        match encode_audio_event_payload(&state) {
            Ok(broadcast) => {
                if socket.send(Message::from(broadcast)).await.is_err() {
                    break;
                }
            }
            Err(e) => warn!(player_id, error = %e, "failed to encode initial audio_event"),
        }
    }
}

async fn handle_socket(mut socket: WebSocket, app_state: AppState) {
    let session_token = Alphanumeric.sample_string(&mut rand::rng(), 64);
    let msg = Message::from(format!(
        "LWSS V {} | ~API:{}~ | Session-Token: {}",
        app_state.app_config.version,
        app_state.app_config.api_version,
        app_state.app_config.remote_session_token,
    ));
    info!(
        port = 7091,
        api = "remote",
        session_token,
        "remote websocket connected"
    );
    socket.send(msg).await.unwrap();

    push_initial_audio_events(
        &mut socket,
        &app_state.app_config,
        app_state.from_api.clone(),
    )
    .await;

    let app_config = app_state.app_config.clone();
    let from_api = app_state.from_api.clone();
    let (async_outbound, mut async_outbound_rx) = mpsc::unbounded_channel();
    let api = Api::new(
        app_config.clone(),
        session_token.clone(),
        app_state.from_api,
        app_state.from_roon.clone(),
        async_outbound,
    );
    let mut receiver = app_state.from_roon.subscribe();
    let mut inbound_log_count: u32 = 0;

    loop {
        tokio::select! {
            Some(message) = async_outbound_rx.recv() => {
                if socket.send(message).await.is_err() {
                    return;
                }
            },
            option = socket.recv() => {
                if let Some(result) = option {
                    if let Ok(message) = result {
                        match message {
                            Message::Text(utf8_bytes) => {
                                inbound_log_count += 1;
                                crate::command_audit::log_inbound(
                                    crate::command_audit::PORT_REMOTE,
                                    &session_token,
                                    utf8_bytes.trim(),
                                );
                                match api.process_message(utf8_bytes).await {
                                    Ok(message) => {
                                        socket.send(message).await.unwrap(); // ? anyhow
                                    }
                                    Err(error) => {
                                        error!(port = 7091, api = "remote", error = %error, session_token, "remote command handler failed");
                                    },
                                }
                            },
                            Message::Close(close_frame) => {
                                info!(
                                    port = 7091,
                                    api = "remote",
                                    ?close_frame,
                                    session_token,
                                    commands = inbound_log_count,
                                    "remote websocket closed by client"
                                );
                                return;
                            },
                            _ => ()
                        }
                    } else {
                        info!(
                            port = 7091,
                            api = "remote",
                            session_token,
                            commands = inbound_log_count,
                            "remote connection dropped by client"
                        );
                        return;
                    }
                } else {
                    info!(
                        port = 7091,
                        api = "remote",
                        session_token,
                        commands = inbound_log_count,
                        "remote connection closed by client"
                    );
                    return;
                }
            },
            result = receiver.recv() => {
                match result {
                    Ok(io_event) => match io_event {
                        IoEvent::Seek(playerid, seconds) => {
                            let broadcast = serde_json::to_string(&json!({
                                "audio_event": [{"playerid": playerid, "time": seconds}],
                            })).unwrap();
                            socket.send(Message::from(broadcast)).await.unwrap(); // ? anyhow
                        },
                        IoEvent::State(state) => {
                            let broadcast = serde_json::to_string(&json!({
                                "audio_event": [state.clone()],
                            })).unwrap(); // ? anyhow
                            socket.send(Message::from(broadcast)).await.unwrap(); // ? anyhow
                        },
                        IoEvent::ConfigUpdated(_) => {}
                        // Reference: hw_event heartbeats on 7095 (Miniserver) only, not 7091 Web UI.
                        IoEvent::HardwareState(_) => {}
                        IoEvent::CustomUrlChanged => {
                            let broadcast = format_customurl_changed_event();
                            if socket.send(Message::from(broadcast)).await.is_err() {
                                return;
                            }
                        }
                        IoEvent::RoomFavChanged { playerid, count } => {
                            let broadcast = format_roomfav_changed_event(playerid, count);
                            if socket.send(Message::from(broadcast)).await.is_err() {
                                return;
                            }
                        }
                    },
                    Err(RecvError::Lagged(skipped)) => {
                        warn!(
                            skipped,
                            port = crate::command_audit::PORT_REMOTE,
                            "state broadcast lagged; resyncing from registry via Status"
                        );
                        let payloads = resync_connect_audio_events(
                            &from_api,
                            &app_config,
                            ConnectSurface::Remote7091,
                        )
                        .await;
                        for broadcast in payloads {
                            if socket.send(Message::from(broadcast)).await.is_err() {
                                return;
                            }
                        }
                    }
                    Err(error) => {
                        warn!(port = 7091, api = "remote", error = %error, session_token, "remote event receiver failed");
                    },
                }
                // {"audio_sync_event":[{"group":"fe78dcce-e931-095d-0eff-018e010d95d8","mastervolume":25,"players":[{"id":"${zone[15].player.uuid}","playerid":${zone[15].player.playerid}},{"id":"${zone[14].player.uuid}","playerid":${zone[14].player.playerid}}],"type":"dynamic"}]}
            }
        }
    }
}

/// Loxone `customurl_changed_event` — custom radio browse reload (must be `[]`, not `{}`).
pub fn format_customurl_changed_event() -> String {
    r#"{"customurl_changed_event":[]}"#.to_string()
}

/// Loxone `roomfavchanged_event` — room favorites list changed for a zone.
pub fn format_roomfav_changed_event(playerid: usize, count: usize) -> String {
    serde_json::json!({
        "roomfavchanged_event": [{ "playerid": playerid, "count": count }]
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::{proxy_fetch_url, unmatched_http};
    use axum::http::{HeaderMap, Method, StatusCode, Uri};

    #[tokio::test]
    async fn unmatched_http_answers_not_found() {
        let status = unmatched_http(
            Method::PUT,
            Uri::from_static("/upload/audio_recording.wav"),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[test]
    fn proxy_fetch_url_appends_the_cache_bust() {
        assert_eq!(
            proxy_fetch_url("https://media.focal-naim.com/s/2822.png", "x"),
            "https://media.focal-naim.com/s/2822.png?_=x"
        );
        assert_eq!(
            proxy_fetch_url("https://static.qobuz.com/c.jpg?w=600", "The Lioness"),
            "https://static.qobuz.com/c.jpg?w=600&_=The%20Lioness"
        );
    }
}
