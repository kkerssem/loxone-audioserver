//! Generic mDNS browse loop used by Google Cast discovery.

use anyhow::{Context, Result};
use mdns_sd::ResolvedService;
use std::collections::HashMap;
use std::time::Duration;
use tracing::warn;

pub async fn browse_mdns_resolved(
    service_type: &str,
    timeout: Duration,
) -> Result<Vec<ResolvedService>> {
    let mdns = mdns_sd::ServiceDaemon::new().context("create mDNS daemon")?;
    let receiver = mdns
        .browse(service_type)
        .with_context(|| format!("browse {service_type}"))?;

    let mut pending: HashMap<String, ResolvedService> = HashMap::new();
    let deadline = tokio::time::Instant::now() + timeout;

    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }

        let event = match tokio::time::timeout(remaining, receiver.recv_async()).await {
            Ok(Ok(event)) => event,
            Ok(Err(e)) => {
                warn!(error = %e, "mdns browse channel closed");
                break;
            }
            Err(_) => break,
        };

        match event {
            mdns_sd::ServiceEvent::ServiceResolved(resolved) => {
                pending.insert(resolved.get_fullname().to_string(), *resolved);
            }
            mdns_sd::ServiceEvent::ServiceRemoved(_, fullname) => {
                pending.remove(&fullname);
            }
            _ => {}
        }
    }

    let list: Vec<ResolvedService> = pending.into_values().collect();
    drop(receiver);
    if let Ok(status_rx) = mdns.shutdown() {
        let _ = status_rx.recv_timeout(Duration::from_millis(500));
    }
    Ok(list)
}

/// Prefer IPv4 from resolved addresses; fall back to hostname without trailing dot.
pub fn resolved_primary_host(service: &ResolvedService) -> String {
    service
        .get_addresses_v4()
        .into_iter()
        .next()
        .map(|ip| ip.to_string())
        .unwrap_or_else(|| service.get_hostname().trim_end_matches('.').to_string())
}

pub fn resolved_property_str(service: &ResolvedService, key: &str) -> Option<String> {
    service
        .get_properties()
        .get(key)
        .map(|v| v.val_str().to_string())
}
