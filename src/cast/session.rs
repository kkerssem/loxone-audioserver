//! Async Google Cast session ops (oxicast).

use std::time::Duration;

use anyhow::{Context, Result};
use oxicast::{CastApp, CastClient, MediaInfo, MediaMetadata};
use tracing::debug;

use super::device::CastDeviceInfo;
use super::matcher::{forget_known_cast_devices, known_cast_device, resolve_cast_device};

/// How long to try a remembered Cast address before browsing again.
const KNOWN_DEVICE_CONNECT: Duration = Duration::from_secs(3);

/// Connect to the zone's Cast device. The device found by an earlier browse is tried first, so
/// no mDNS browse is needed; if it cannot be reached (address changed, device gone) the
/// remembered devices are dropped and the LAN is browsed for `discovery`.
pub async fn connect_zone_cast(
    player_id: usize,
    discovery: Duration,
) -> Result<(CastDeviceInfo, CastClient)> {
    if let Some(device) = known_cast_device(player_id) {
        let (host, port) = device.control_addr();
        match tokio::time::timeout(KNOWN_DEVICE_CONNECT, connect_cast(&host, port)).await {
            Ok(Ok(client)) => {
                debug!(player_id, cast = %device.friendly_name, "cast device reused without browse");
                return Ok((device, client));
            }
            Ok(Err(e)) => {
                debug!(player_id, error = %e, "known cast device unreachable; browsing again")
            }
            Err(_) => debug!(player_id, "known cast device timed out; browsing again"),
        }
        forget_known_cast_devices();
    }
    let device = resolve_cast_device(player_id, &[], discovery).await?;
    let (host, port) = device.control_addr();
    let client = connect_cast(&host, port).await?;
    Ok((device, client))
}

pub async fn connect_cast(host: &str, port: u16) -> Result<CastClient> {
    CastClient::builder(host, port)
        .verify_tls(false)
        .auto_reconnect(true)
        .connect()
        .await
        .with_context(|| format!("Cast connect {host}:{port}"))
        .map_err(|e| anyhow::anyhow!("{e}"))
}

pub async fn cast_load_stream(
    client: &CastClient,
    stream_url: &str,
    title: &str,
    content_type: &str,
) -> Result<()> {
    client
        .launch_app(&CastApp::DefaultMediaReceiver)
        .await
        .context("Cast launch default media receiver")
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let mut media = MediaInfo::live(stream_url, content_type);
    if !title.is_empty() {
        media = media.metadata(MediaMetadata::Generic {
            title: Some(title.to_string()),
            subtitle: None,
            images: vec![],
        });
    }

    client
        .load_media(&media, true, 0.0, None)
        .await
        .context("Cast load media")
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(())
}

pub async fn cast_play(client: &CastClient) -> Result<()> {
    client
        .play()
        .await
        .context("Cast play")
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(())
}

pub async fn cast_pause(client: &CastClient) -> Result<()> {
    client
        .pause()
        .await
        .context("Cast pause")
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(())
}

pub async fn cast_stop(client: &CastClient) -> Result<()> {
    let _ = client.stop_media().await;
    client
        .disconnect()
        .await
        .context("Cast disconnect")
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(())
}

pub async fn cast_set_volume(client: &CastClient, level_0_100: u8) -> Result<()> {
    let level = (level_0_100 as f32 / 100.0).clamp(0.0, 1.0);
    client
        .set_volume(level)
        .await
        .context("Cast set volume")
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(())
}
