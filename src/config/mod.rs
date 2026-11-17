//! Persisted audioserver configuration (`audioserver-config.json`).

mod crc;
pub mod loxone;
mod mapping;
pub mod zone;

pub use crc::crc32_hex;
pub use loxone::*;
pub use mapping::*;
pub use zone::*;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{LazyLock, RwLock};
use tracing::warn;

pub const CONFIG_FILENAME: &str = "audioserver-config.json";

const CONFIG_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct LoxonePersist {
    pub crc32: String,
    pub state: serde_json::Value,
}

impl Default for LoxonePersist {
    fn default() -> Self {
        Self {
            crc32: crc32_hex(b"[]"),
            state: serde_json::json!([]),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AudioserverConfigFile {
    pub version: u32,
    pub loxone: LoxonePersist,
    pub audioserver: ZoneConfigBody,
}

impl Default for AudioserverConfigFile {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            loxone: LoxonePersist::default(),
            audioserver: ZoneConfigBody::default(),
        }
    }
}

static CACHE: LazyLock<RwLock<AudioserverConfigFile>> =
    LazyLock::new(|| RwLock::new(AudioserverConfigFile::default()));

#[cfg(test)]
static TEST_CONFIG_PATH: LazyLock<RwLock<Option<PathBuf>>> = LazyLock::new(|| RwLock::new(None));

impl AudioserverConfigFile {
    fn disk_path() -> PathBuf {
        #[cfg(test)]
        if let Ok(guard) = TEST_CONFIG_PATH.read() {
            if let Some(path) = guard.as_ref() {
                return path.clone();
            }
        }
        Self::path()
    }

    pub fn path() -> PathBuf {
        PathBuf::from(CONFIG_FILENAME)
    }

    pub fn init() -> Result<()> {
        let cfg = if Self::disk_path().exists() {
            Self::read_from_disk()?
        } else {
            warn!(
                path = CONFIG_FILENAME,
                "audioserver config missing; using defaults"
            );
            Self::default()
        };
        *CACHE.write().expect("config lock") = cfg;
        Ok(())
    }

    pub fn get() -> Self {
        CACHE.read().expect("config lock").clone()
    }

    pub fn update<F, R>(f: F) -> Result<R>
    where
        F: FnOnce(&mut Self) -> Result<R>,
    {
        let on_disk = Self::load_preserved_from_disk();
        let mut guard = CACHE.write().expect("config lock");
        let out = f(&mut guard)?;
        if let Some(ref disk) = on_disk {
            Self::apply_preserved_from_disk(&mut guard, disk);
        }
        guard.save_to_disk()?;
        Ok(out)
    }

    #[allow(dead_code)]
    pub fn save(&self) -> Result<()> {
        let preserved = Self::load_preserved_from_disk();
        *CACHE.write().expect("config lock") = self.clone();
        let mut guard = CACHE.write().expect("config lock");
        if let Some(ref disk) = preserved {
            Self::apply_preserved_from_disk(&mut guard, disk);
        }
        guard.save_to_disk()
    }

    pub fn loxone_crc_hex(&self) -> &str {
        &self.loxone.crc32
    }

    pub fn loxone_state(&self) -> Option<SerialWithConfig> {
        parse_loxone_state(&self.loxone.state)
    }

    pub fn set_loxone_from_setconfig(&mut self, raw: &[u8], parsed: SerialWithConfig) {
        self.loxone.crc32 = crc32_hex(raw);
        self.loxone.state = if raw == b"[]" {
            serde_json::json!([])
        } else {
            serde_json::to_value(&parsed).expect("serialize loxone state")
        };
    }

    pub fn set_loxone_state_pretty(&mut self, parsed: &SerialWithConfig) {
        self.loxone.state = serde_json::to_value(parsed).expect("serialize loxone state");
    }

    pub fn clear_loxone(&mut self) {
        self.loxone.crc32 = crc32_hex(b"[]");
        self.loxone.state = serde_json::json!([]);
    }

    fn read_from_disk() -> Result<Self> {
        let path = Self::disk_path();
        let data =
            std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        let mut cfg: Self =
            serde_json::from_str(&data).with_context(|| format!("parse {}", path.display()))?;
        if cfg.version == 0 {
            cfg.version = CONFIG_VERSION;
        }
        Ok(cfg)
    }

    fn save_to_disk(&self) -> Result<()> {
        #[cfg(test)]
        if !Self::test_disk_redirect_active() {
            return Ok(());
        }
        let path = Self::disk_path();
        let data = serde_json::to_string_pretty(self).context("serialize audioserver config")?;
        std::fs::write(&path, data).with_context(|| format!("write {}", path.display()))
    }

    /// Read the on-disk config before acquiring the write lock (player mappings).
    fn load_preserved_from_disk() -> Option<AudioserverConfigFile> {
        #[cfg(test)]
        if !Self::test_disk_redirect_active() {
            return None;
        }
        let path = Self::disk_path();
        if !path.exists() {
            return None;
        }
        Self::read_from_disk().ok()
    }

    /// When in-memory player rows lost device mappings, restore from disk before saving.
    fn apply_preserved_from_disk(file: &mut AudioserverConfigFile, disk: &AudioserverConfigFile) {
        if file.audioserver.players.is_empty() {
            if disk
                .audioserver
                .players
                .iter()
                .any(|p| p.has_preserved_mapping_fields())
            {
                warn!(
                    players = disk.audioserver.players.len(),
                    "restoring player rows from disk — in-memory config omitted players on save"
                );
                file.audioserver.players = disk.audioserver.players.clone();
            }
            return;
        }

        for disk_player in &disk.audioserver.players {
            if let Some(mem_player) = file
                .audioserver
                .players
                .iter_mut()
                .find(|p| p.player_id == disk_player.player_id)
            {
                if mem_player.restore_preserved_fields_from(disk_player) {
                    warn!(
                        player_id = mem_player.player_id,
                        "restoring device mapping from disk — in-memory row omitted device ids on save"
                    );
                }
            }
        }
    }
}

#[cfg(test)]
impl AudioserverConfigFile {
    pub fn reset_cache_for_test(config: AudioserverConfigFile) {
        *CACHE.write().expect("config lock") = config;
    }

    fn test_disk_redirect_active() -> bool {
        TEST_CONFIG_PATH
            .read()
            .ok()
            .and_then(|guard| guard.as_ref().map(|_| ()))
            .is_some()
    }

    fn test_temp_dir() -> PathBuf {
        std::env::temp_dir().join(format!(
            "loxone-config-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ))
    }

    /// Temp config file on disk + in-memory cache; never touches production `audioserver-config.json`.
    pub fn with_test_cache<R>(config: AudioserverConfigFile, f: impl FnOnce() -> R) -> R {
        Self::with_test_lock(|| {
            let dir = Self::test_temp_dir();
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("config test temp dir");
            let path = dir.join(CONFIG_FILENAME);
            Self::set_test_disk_path(Some(path));
            Self::reset_cache_for_test(config);
            let out = f();
            Self::set_test_disk_path(None);
            let _ = std::fs::remove_dir_all(&dir);
            out
        })
    }

    /// Temp config path only; caller manages cache/init (disk round-trip tests).
    pub fn with_test_disk<R>(f: impl FnOnce(PathBuf) -> R) -> R {
        Self::with_test_lock(|| {
            let dir = Self::test_temp_dir();
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("config test temp dir");
            let path = dir.join(CONFIG_FILENAME);
            Self::set_test_disk_path(Some(path.clone()));
            let out = f(path);
            Self::set_test_disk_path(None);
            let _ = std::fs::remove_dir_all(&dir);
            out
        })
    }

    pub fn with_test_lock<R>(f: impl FnOnce() -> R) -> R {
        let _guard = test_lock();
        f()
    }

    /// Guard form of [`Self::with_test_cache`] for async tests: holds the config test lock with a
    /// fresh, empty temp config until dropped.
    ///
    /// Limits:
    /// - The lock is not reentrant: calling `with_test_cache` / `with_test_disk` /
    ///   `with_test_lock` (or a second `lock_for_test`) while holding the guard deadlocks.
    /// - Holding it across `.await` is only safe on `#[tokio::test]`'s default single-threaded
    ///   runtime; a `multi_thread` test must not hold it across an `.await`.
    /// - Dropping it removes the temp file and test path but does not restore the previous
    ///   cache contents (neither does `with_test_cache`); the next locked test resets it.
    pub fn lock_for_test() -> TestConfigGuard {
        let lock = test_lock();
        let dir = Self::test_temp_dir();
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("config test temp dir");
        Self::set_test_disk_path(Some(dir.join(CONFIG_FILENAME)));
        Self::reset_cache_for_test(Self::default());
        TestConfigGuard { _lock: lock, dir }
    }

    pub fn set_test_disk_path(path: Option<PathBuf>) {
        *TEST_CONFIG_PATH.write().expect("config test path lock") = path;
    }
}

/// One lock serializes every test that touches the process-wide config cache. A panicking test
/// must not poison it for all later tests, so poisoning is ignored.
#[cfg(test)]
fn test_lock() -> std::sync::MutexGuard<'static, ()> {
    static TEST_LOCK: LazyLock<std::sync::Mutex<()>> = LazyLock::new(|| std::sync::Mutex::new(()));
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
pub struct TestConfigGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
    dir: PathBuf,
}

#[cfg(test)]
impl Drop for TestConfigGuard {
    fn drop(&mut self) {
        AudioserverConfigFile::set_test_disk_path(None);
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_empty_crc_matches_empty_array() {
        let cfg = AudioserverConfigFile::default();
        assert_eq!(cfg.loxone_crc_hex(), crc32_hex(b"[]"));
        assert!(cfg.loxone_state().is_none());
    }

    #[test]
    fn setconfig_updates_crc_without_raw_storage() {
        let raw = br#"[{"504F94FF1BB3":{"master":"504F94000002","ip":"x","port":80,"language":"ENU","blinkpos":0,"autoUpdate":120,"unitTemperature":"C","name":"AS","errortts":false,"uuid":"srv","pwHash":"","players":[],"extensions":[],"groups":[]}}]"#;
        let parsed: SerialWithConfig = serde_json::from_slice(raw).unwrap();
        let mut cfg = AudioserverConfigFile::default();
        cfg.set_loxone_from_setconfig(raw, parsed);
        assert_eq!(cfg.loxone_crc_hex(), crc32_hex(raw));
        assert!(cfg.loxone_state().is_some());
    }

    #[test]
    fn round_trip_fixture() {
        let json = include_str!("../../tests/fixtures/audioserver-config-v1.json");
        let cfg: AudioserverConfigFile = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.version, 1);
        assert!(!cfg.audioserver.players.is_empty());
    }

    #[test]
    fn clear_loxone_resets_crc_and_state() {
        let mut cfg = AudioserverConfigFile::default();
        let raw = br#"[{"504F94FF1BB3":{"master":"x","ip":"x","port":80,"language":"ENU","blinkpos":0,"autoUpdate":120,"unitTemperature":"C","name":"AS","errortts":false,"uuid":"srv","pwHash":"","players":[],"extensions":[],"groups":[]}}]"#;
        let parsed: SerialWithConfig = serde_json::from_slice(raw).unwrap();
        cfg.set_loxone_from_setconfig(raw, parsed);
        cfg.clear_loxone();
        assert_eq!(cfg.loxone_crc_hex(), crc32_hex(b"[]"));
        assert_eq!(cfg.loxone.state, serde_json::json!([]));
        assert!(cfg.loxone_state().is_none());
    }

    #[test]
    fn update_without_test_disk_redirect_does_not_write_production_path() {
        AudioserverConfigFile::with_test_lock(|| {
            let prod = AudioserverConfigFile::path();
            let before = prod.exists().then(|| std::fs::read(&prod).ok()).flatten();
            AudioserverConfigFile::reset_cache_for_test(AudioserverConfigFile::default());
            AudioserverConfigFile::update(|file| {
                file.audioserver.players.push(crate::config::PlayerMapping {
                    player_id: 99,
                    loxone_uuid: None,
                    name: "test-only".into(),
                    cast_device_id: None,
                    volume_level: None,
                    ..Default::default()
                });
                Ok(())
            })
            .unwrap();
            let after = prod.exists().then(|| std::fs::read(&prod).ok()).flatten();
            assert_eq!(
                before, after,
                "cargo test must not mutate {CONFIG_FILENAME}"
            );
        });
    }

    #[test]
    fn update_restores_player_mappings_when_cache_omits_device_ids() {
        use crate::config::PlayerMapping;

        AudioserverConfigFile::with_test_disk(|path| {
            let mut disk_cfg: AudioserverConfigFile = serde_json::from_str(include_str!(
                "../../tests/fixtures/audioserver-config-v1.json"
            ))
            .unwrap();
            disk_cfg.audioserver.players = vec![PlayerMapping {
                player_id: 3,
                loxone_uuid: Some("zone-uuid-3".into()),
                name: "Bedroom".into(),
                cast_device_id: Some("c431e9d77cba7b8a3c2fde5fca5eba96".into()),
                volume_level: Some(40),
                ..Default::default()
            }];
            std::fs::write(&path, serde_json::to_string_pretty(&disk_cfg).unwrap()).unwrap();
            AudioserverConfigFile::init().unwrap();
            {
                let mut guard = CACHE.write().expect("config lock");
                guard.audioserver.players = vec![PlayerMapping {
                    player_id: 3,
                    loxone_uuid: Some("zone-uuid-3".into()),
                    name: "Zone 3".into(),
                    cast_device_id: None,
                    volume_level: Some(80),
                    ..Default::default()
                }];
            }
            AudioserverConfigFile::update(|file| {
                file.audioserver.players[0].name = "Balcony".into();
                Ok(())
            })
            .unwrap();
            let loaded = AudioserverConfigFile::get();
            let player = &loaded.audioserver.players[0];
            assert_eq!(player.name, "Balcony");
            assert_eq!(player.volume_level, Some(80));
            assert_eq!(
                player.cast_device_id.as_deref(),
                Some("c431e9d77cba7b8a3c2fde5fca5eba96")
            );
            let on_disk = std::fs::read_to_string(&path).unwrap();
            assert!(on_disk.contains("c431e9d77cba7b8a3c2fde5fca5eba96"));
        });
    }

    #[test]
    fn disk_round_trip_via_test_path() {
        AudioserverConfigFile::with_test_disk(|path| {
            let fixture = include_str!("../../tests/fixtures/audioserver-config-v1.json");
            std::fs::write(&path, fixture).unwrap();
            AudioserverConfigFile::init().unwrap();
            AudioserverConfigFile::update(|file| {
                file.audioserver.players[0].name = "Updated".into();
                Ok(())
            })
            .unwrap();
            AudioserverConfigFile::init().unwrap();
            assert_eq!(
                AudioserverConfigFile::get().audioserver.players[0].name,
                "Updated"
            );
        });
    }

    #[test]
    fn save_restores_player_rows_from_disk_when_cache_has_none() {
        use crate::config::PlayerMapping;

        AudioserverConfigFile::with_test_disk(|path| {
            let mut disk_cfg = AudioserverConfigFile::default();
            disk_cfg.audioserver.players = vec![PlayerMapping {
                player_id: 4,
                name: "Kitchen".into(),
                cast_device_id: Some("c431e9d77cba7b8a3c2fde5fca5eba96".into()),
                ..Default::default()
            }];
            std::fs::write(&path, serde_json::to_string_pretty(&disk_cfg).unwrap()).unwrap();
            AudioserverConfigFile::init().unwrap();

            let mut stale = AudioserverConfigFile::default();
            let empty: SerialWithConfig = [Default::default()];
            stale.set_loxone_from_setconfig(b"[]", empty.clone());
            assert_eq!(stale.loxone.state, serde_json::json!([]));
            stale.set_loxone_state_pretty(&empty);
            stale.save().unwrap();

            let loaded = AudioserverConfigFile::get();
            assert_eq!(loaded.audioserver.players.len(), 1);
            assert_eq!(loaded.audioserver.players[0].player_id, 4);
            let on_disk = std::fs::read_to_string(&path).unwrap();
            assert!(on_disk.contains("c431e9d77cba7b8a3c2fde5fca5eba96"));
        });
    }

    #[test]
    fn init_missing_file_uses_defaults() {
        AudioserverConfigFile::with_test_disk(|path| {
            assert!(!path.exists());
            AudioserverConfigFile::init().unwrap();
            let cfg = AudioserverConfigFile::get();
            assert_eq!(cfg.loxone_crc_hex(), crc32_hex(b"[]"));
        });
    }
}
