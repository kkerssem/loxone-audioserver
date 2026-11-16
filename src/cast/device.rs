//! Google Cast device discovered via `_googlecast._tcp`.

#[derive(Debug, Clone)]
pub struct CastDeviceInfo {
    pub host: String,
    pub port: u16,
    pub friendly_name: String,
    pub device_id: String,
    #[allow(dead_code)]
    pub model: Option<String>,
}

impl CastDeviceInfo {
    pub fn control_addr(&self) -> (String, u16) {
        // Cast control channel is TLS on port 8009; mDNS SRV port is often 8009 too.
        (
            self.host.clone(),
            if self.port == 0 { 8009 } else { self.port },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_addr_defaults_port_when_zero() {
        let dev = CastDeviceInfo {
            host: "192.168.1.10".into(),
            port: 0,
            friendly_name: "Speaker".into(),
            device_id: "abc".into(),
            model: None,
        };
        assert_eq!(dev.control_addr(), ("192.168.1.10".into(), 8009));
    }

    #[test]
    fn control_addr_keeps_nonzero_port() {
        let dev = CastDeviceInfo {
            host: "192.168.1.10".into(),
            port: 8010,
            friendly_name: "Speaker".into(),
            device_id: "abc".into(),
            model: None,
        };
        assert_eq!(dev.control_addr(), ("192.168.1.10".into(), 8010));
    }
}
