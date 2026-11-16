//! Audioserver identity and crypto material for Loxone protocol handlers.

use crate::net_util::{cover_proxy_base_url, resolve_public_host};
use anyhow::Context;
use rsa::{
    pkcs8::{DecodePrivateKey, EncodePrivateKey, LineEnding},
    PrivateKey as RsaPrivateKey, PublicKey as RsaPublicKey,
};
use std::path::PathBuf;
use std::sync::Arc;
use tracing::info;

pub const DEFAULT_FIRMWARE: &str = "17.1.05.05";
pub const DEFAULT_API_VERSION: &str = "1.6";
/// Stable remote (7091) session token; matches lox-audioserver default.
pub const DEFAULT_REMOTE_SESSION_TOKEN: &str =
    "8WahwAfULwEQce9Yu0qIE9L7QMkXFHbi0M9ch9vKcgYArPPojXHpSiNcq0fT3lqL";
pub const PRIVATE_KEY_FILE: &str = "private.pem";

#[cfg(test)]
static TEST_PRIVATE_KEY_PATH: std::sync::LazyLock<std::sync::RwLock<Option<PathBuf>>> =
    std::sync::LazyLock::new(|| std::sync::RwLock::new(None));

fn private_key_path() -> PathBuf {
    #[cfg(test)]
    if let Ok(guard) = TEST_PRIVATE_KEY_PATH.read() {
        if let Some(path) = guard.as_ref() {
            return path.clone();
        }
    }
    PathBuf::from(PRIVATE_KEY_FILE)
}

#[derive(Clone)]
pub struct AppConfig {
    pub api_version: String,
    pub key_pair: Arc<(RsaPrivateKey, RsaPublicKey)>,
    pub serial: String,
    pub version: String,
    pub display_name: String,
    pub hostname: String,
    /// Host reachable by Loxone UI for HTTP cover proxy (not Miniserver `config.ip`).
    pub public_host: String,
    /// Returned in `audio/cfg/ready` — stable for process lifetime.
    pub session_id: i64,
    /// First-line LWSS greeting on port 7091.
    pub remote_session_token: String,
}

impl AppConfig {
    pub fn new(
        serial: String,
        display_name: Option<String>,
        hostname: Option<String>,
        public_host: Option<String>,
    ) -> anyhow::Result<Self> {
        let (private_key, public_key) = load_keys()?;
        let display_name = display_name.unwrap_or_else(|| "audioserver".to_string());
        let hostname = hostname.unwrap_or_else(|| "audioserver".to_string());
        let public_host = resolve_public_host(public_host);
        let session_id = (rand::random::<u64>() & i64::MAX as u64) as i64;

        Ok(Self {
            api_version: DEFAULT_API_VERSION.to_string(),
            key_pair: Arc::new((private_key, public_key)),
            serial,
            version: DEFAULT_FIRMWARE.to_string(),
            display_name,
            hostname,
            public_host,
            session_id,
            remote_session_token: DEFAULT_REMOTE_SESSION_TOKEN.to_string(),
        })
    }

    /// `http://{public_host}:7091` for `coverurl` / asset URLs.
    pub fn cover_proxy_base(&self) -> String {
        cover_proxy_base_url(&self.public_host)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cover_proxy_base_uses_public_host() {
        with_test_keys(|| {
            let cfg = AppConfig::new(
                "504F94FF1BB3".into(),
                None,
                None,
                Some("192.168.1.10".into()),
            )
            .unwrap();
            assert_eq!(cfg.cover_proxy_base(), "http://192.168.1.10:7091");
        });
    }

    #[test]
    fn new_sets_firmware_and_api_defaults() {
        with_test_keys(|| {
            let cfg = AppConfig::new("abc123".into(), None, None, None).unwrap();
            assert_eq!(cfg.version, DEFAULT_FIRMWARE);
            assert_eq!(cfg.api_version, DEFAULT_API_VERSION);
            assert_eq!(cfg.remote_session_token, DEFAULT_REMOTE_SESSION_TOKEN);
        });
    }
}

pub fn load_keys() -> anyhow::Result<(RsaPrivateKey, RsaPublicKey)> {
    let path = private_key_path();
    let private_key = match RsaPrivateKey::read_pkcs8_pem_file(&path) {
        Ok(key) => key,
        Err(_) => {
            let mut rng = rand::rng();
            let private_key =
                RsaPrivateKey::new(&mut rng, 2048).context("generate RSA key pair")?;
            #[cfg(test)]
            if TEST_PRIVATE_KEY_PATH
                .read()
                .ok()
                .and_then(|g| g.clone())
                .is_none()
            {
                return Ok((private_key.clone(), private_key.to_public_key()));
            }
            if let Ok(pem) = private_key.to_pkcs8_pem(LineEnding::LF) {
                if std::fs::write(&path, pem.as_bytes()).is_ok() {
                    info!(
                        path = %path.display(),
                        "generated and saved new audioserver RSA key pair"
                    );
                }
            }
            private_key
        }
    };
    let public_key = private_key.to_public_key();
    Ok((private_key, public_key))
}

#[cfg(test)]
pub fn with_test_keys<R>(f: impl FnOnce() -> R) -> R {
    static TEST_LOCK: std::sync::LazyLock<std::sync::Mutex<()>> =
        std::sync::LazyLock::new(|| std::sync::Mutex::new(()));
    let _guard = TEST_LOCK.lock().expect("private key test lock");
    let dir = std::env::temp_dir().join(format!(
        "loxone-private-key-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("private key test temp dir");
    let path = dir.join(PRIVATE_KEY_FILE);
    let mut rng = rand::rng();
    let private_key = RsaPrivateKey::new(&mut rng, 2048).expect("test rsa key");
    let pem = private_key.to_pkcs8_pem(LineEnding::LF).expect("pem");
    std::fs::write(&path, pem.as_bytes()).expect("write test private key");
    *TEST_PRIVATE_KEY_PATH
        .write()
        .expect("private key test path lock") = Some(path);
    let out = f();
    *TEST_PRIVATE_KEY_PATH
        .write()
        .expect("private key test path lock") = None;
    let _ = std::fs::remove_dir_all(&dir);
    out
}
