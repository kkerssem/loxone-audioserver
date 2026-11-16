//! Voice recordings the Loxone App uploads before `audio/grouped/playuploadedfile/{file}/{zones}`.
//!
//! The App sends `PUT /audio/cfg/upload/audioupload/add/{filename}` (body: `audio/wav`) on 7091.
//! Files are kept under [`UPLOAD_DIR`] relative to the working directory, next to `recents/`,
//! and served back on 7091 at [`SERVE_PREFIX`] so Cast devices can fetch them.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};

/// Directory (relative to the working directory) that holds uploaded voice recordings.
pub const UPLOAD_DIR: &str = "voice-uploads";

/// 7091 HTTP path prefix the stored recordings are served from (`{SERVE_PREFIX}/{file}`).
pub const SERVE_PREFIX: &str = "/voice-uploads";

/// Largest accepted upload. The App caps recordings at 1 minute; even 48 kHz 16-bit stereo WAV
/// (~11.5 MB/min) fits. Axum's 2 MB default would not.
pub const MAX_UPLOAD_BYTES: usize = 20 * 1024 * 1024;

/// Uploads and message files older than this are deleted on the next upload, so files that
/// were never played (or left behind by a restart) do not pile up. The PUT is unauthenticated.
const MAX_AGE: Duration = Duration::from_secs(60 * 60);

/// Bytes read from a message file to find its length (the RIFF header with `fmt ` and the
/// start of `data`); the audio itself is not read.
const HEADER_BYTES: usize = 4096;

/// Prefix of the per-message copy of an upload (see [`claim`]).
const MESSAGE_PREFIX: &str = "msg-";

/// Process-unique suffix for temporary and message file names.
fn unique_suffix() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default();
    format!("{millis}-{}", NEXT.fetch_add(1, Ordering::Relaxed))
}

/// Reduces an App-supplied name to a plain file name: no directories, only `[A-Za-z0-9._-]`.
pub fn sanitize_file_name(name: &str) -> Option<String> {
    let base = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let cleaned: String = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim_start_matches('.');
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Writes `body` to `dir/<sanitized name>`, replacing an earlier upload of the same name. The
/// bytes go to a unique temporary file first (names starting with `.` are never served), so
/// concurrent uploads cannot publish each other's content. Old files are pruned first.
pub async fn store(dir: &Path, name: &str, body: &[u8]) -> Result<PathBuf> {
    let Some(file_name) = sanitize_file_name(name) else {
        bail!("invalid upload file name");
    };
    if body.is_empty() {
        bail!("empty upload");
    }
    tokio::fs::create_dir_all(dir)
        .await
        .with_context(|| format!("create {}", dir.display()))?;
    prune(dir, MAX_AGE).await;
    let path = dir.join(&file_name);
    let partial = dir.join(format!(".{file_name}.{}.part", unique_suffix()));
    if let Err(e) = tokio::fs::write(&partial, body).await {
        let _ = tokio::fs::remove_file(&partial).await;
        return Err(e).with_context(|| format!("write {}", partial.display()));
    }
    tokio::fs::rename(&partial, &path)
        .await
        .with_context(|| format!("rename to {}", path.display()))?;
    Ok(path)
}

/// Delete files in `dir` last modified more than `max_age` ago (best effort).
pub async fn prune(dir: &Path, max_age: Duration) {
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
        return;
    };
    let now = SystemTime::now();
    while let Ok(Some(entry)) = entries.next_entry().await {
        let old = entry
            .metadata()
            .await
            .ok()
            .filter(|m| m.is_file())
            .and_then(|m| m.modified().ok())
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age > max_age);
        if old {
            let _ = tokio::fs::remove_file(entry.path()).await;
        }
    }
}

/// An upload taken for one message: moved to its own file, so the next upload of the same
/// name (the App always sends `audio_recording.wav`) cannot replace it while it plays.
#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    /// Message file, deleted once every zone has played it.
    pub path: PathBuf,
    /// Its name as served under [`SERVE_PREFIX`].
    pub file_name: String,
    /// Playing time from the WAV header, if readable.
    pub length: Option<Duration>,
}

/// Move the upload `name` to a message file and read its length (header only).
pub async fn claim(dir: &Path, name: &str) -> Result<Message> {
    let Some(upload) = sanitize_file_name(name) else {
        bail!("invalid upload file name");
    };
    let file_name = format!("{MESSAGE_PREFIX}{}-{upload}", unique_suffix());
    let path = dir.join(&file_name);
    tokio::fs::rename(dir.join(&upload), &path)
        .await
        .with_context(|| format!("no upload {upload}"))?;
    let length = read_length(&path).await;
    Ok(Message {
        path,
        file_name,
        length,
    })
}

async fn read_length(path: &Path) -> Option<Duration> {
    use tokio::io::AsyncReadExt;
    let mut file = tokio::fs::File::open(path).await.ok()?;
    let total = usize::try_from(file.metadata().await.ok()?.len()).ok()?;
    let mut header = vec![0; HEADER_BYTES.min(total)];
    file.read_exact(&mut header).await.ok()?;
    wav_duration(&header, total)
}

/// Reads a stored upload back (same name sanitizing as [`store`]).
pub async fn read(dir: &Path, name: &str) -> Result<Vec<u8>> {
    let Some(file_name) = sanitize_file_name(name) else {
        bail!("invalid upload file name");
    };
    let path = dir.join(file_name);
    tokio::fs::read(&path)
        .await
        .with_context(|| format!("read {}", path.display()))
}

/// Playing time of a PCM WAV file from its `fmt ` byte rate and `data` chunk size, clamped to
/// `total_len` (the file size). `header` is the start of the file, enough to reach the `data`
/// chunk header. `None` when it is not a usable RIFF/WAVE header.
pub fn wav_duration(header: &[u8], total_len: usize) -> Option<Duration> {
    if header.len() < 12 || &header[0..4] != b"RIFF" || &header[8..12] != b"WAVE" {
        return None;
    }
    let u32_at = |at: usize| -> Option<u32> {
        header
            .get(at..at + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let mut byte_rate = None;
    let mut at = 12;
    while at + 8 <= header.len() {
        let id = &header[at..at + 4];
        let size = u32_at(at + 4)? as usize;
        let body = at + 8;
        if id == b"fmt " {
            byte_rate = u32_at(body + 8);
        } else if id == b"data" {
            let rate = byte_rate.filter(|&r| r > 0)?;
            let len = size.min(total_len.saturating_sub(body));
            return Some(Duration::from_secs_f64(len as f64 / f64::from(rate)));
        }
        at = body.checked_add(size)?.checked_add(size % 2)?;
    }
    None
}

/// Zone ids from the `playuploadedfile` zone list (`4` or `3,4`); invalid entries are skipped.
pub fn parse_zone_list(zones: &str) -> Vec<usize> {
    let mut ids: Vec<usize> = Vec::new();
    for id in zones.split(',').filter_map(|z| z.trim().parse().ok()) {
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    ids
}

/// Volume for a message: the zone's TTS level is a minimum, a louder current volume is kept
/// (Loxone Audio Player: "If the current playback volume is higher, it is used instead").
pub fn message_volume(current: u8, tts_minimum: Option<u8>) -> u8 {
    current.max(tts_minimum.unwrap_or(0)).min(100)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_keeps_plain_names_and_strips_paths() {
        assert_eq!(
            sanitize_file_name("audio_recording.wav").as_deref(),
            Some("audio_recording.wav")
        );
        assert_eq!(
            sanitize_file_name("../../etc/passwd").as_deref(),
            Some("passwd")
        );
        assert_eq!(
            sanitize_file_name("..\\x\\my rec.wav").as_deref(),
            Some("my_rec.wav")
        );
        assert_eq!(sanitize_file_name(".hidden").as_deref(), Some("hidden"));
        assert_eq!(sanitize_file_name(".."), None);
        assert_eq!(sanitize_file_name(""), None);
    }

    fn wav(byte_rate: u32, data_len: u32, present: usize) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + data_len).to_le_bytes());
        b.extend_from_slice(b"WAVE");
        b.extend_from_slice(b"fmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes()); // PCM
        b.extend_from_slice(&1u16.to_le_bytes()); // mono
        b.extend_from_slice(&(byte_rate / 2).to_le_bytes()); // sample rate
        b.extend_from_slice(&byte_rate.to_le_bytes());
        b.extend_from_slice(&2u16.to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&data_len.to_le_bytes());
        b.resize(b.len() + present, 0);
        b
    }

    #[test]
    fn wav_duration_reads_the_header() {
        // 48 kHz mono 16-bit, 5 s (the App's format, captured 2026-11-15).
        let five_s = wav(96_000, 480_000, 480_000);
        assert_eq!(
            wav_duration(&five_s, five_s.len()),
            Some(Duration::from_secs(5))
        );
        // Only the header is read; the length comes from the file size.
        assert_eq!(
            wav_duration(&five_s[..44], five_s.len()),
            Some(Duration::from_secs(5))
        );
        // A truncated file only counts the bytes present.
        let cut = wav(96_000, 480_000, 96_000);
        assert_eq!(wav_duration(&cut, cut.len()), Some(Duration::from_secs(1)));
        assert_eq!(wav_duration(b"not a wav file", 14), None);
        assert_eq!(wav_duration(&wav(0, 10, 10), 54), None);
    }

    #[test]
    fn zone_list_and_message_volume() {
        assert_eq!(parse_zone_list("4"), vec![4]);
        assert_eq!(parse_zone_list("3,4"), vec![3, 4]);
        assert_eq!(parse_zone_list("3, x,4,3"), vec![3, 4]);
        assert!(parse_zone_list("").is_empty());

        assert_eq!(message_volume(10, Some(20)), 20);
        assert_eq!(message_volume(36, Some(20)), 36);
        assert_eq!(message_volume(15, None), 15);
    }

    #[tokio::test]
    async fn read_returns_the_stored_bytes() {
        let dir =
            std::env::temp_dir().join(format!("loxone-voice-read-test-{}", std::process::id()));
        let _ = tokio::fs::remove_dir_all(&dir).await;
        store(&dir, "msg.wav", b"RIFF").await.unwrap();
        assert_eq!(read(&dir, "../msg.wav").await.unwrap(), b"RIFF");
        assert!(read(&dir, "missing.wav").await.is_err());
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn claim_moves_the_upload_to_a_message_file() {
        let dir =
            std::env::temp_dir().join(format!("loxone-voice-claim-test-{}", std::process::id()));
        let _ = tokio::fs::remove_dir_all(&dir).await;
        store(&dir, "audio_recording.wav", &wav(96_000, 192_000, 192_000))
            .await
            .unwrap();

        let message = claim(&dir, "audio_recording.wav").await.unwrap();
        assert!(message.file_name.starts_with(MESSAGE_PREFIX));
        assert!(message.file_name.ends_with("-audio_recording.wav"));
        assert_eq!(message.length, Some(Duration::from_secs(2)));
        assert!(message.path.exists());
        assert!(
            !dir.join("audio_recording.wav").exists(),
            "the name is free for the next upload"
        );
        assert!(claim(&dir, "audio_recording.wav").await.is_err());
        assert_eq!(
            read(&dir, &message.file_name).await.unwrap().len(),
            44 + 192_000
        );
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn prune_removes_only_old_files() {
        let dir =
            std::env::temp_dir().join(format!("loxone-voice-prune-test-{}", std::process::id()));
        let _ = tokio::fs::remove_dir_all(&dir).await;
        store(&dir, "fresh.wav", b"RIFF").await.unwrap();
        prune(&dir, Duration::from_secs(3600)).await;
        assert!(dir.join("fresh.wav").exists());
        prune(&dir, Duration::ZERO).await;
        assert!(!dir.join("fresh.wav").exists());
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn store_writes_and_replaces_the_file() {
        let dir =
            std::env::temp_dir().join(format!("loxone-voice-upload-test-{}", std::process::id()));
        let _ = tokio::fs::remove_dir_all(&dir).await;

        let path = store(&dir, "audio_recording.wav", b"RIFF1").await.unwrap();
        assert_eq!(path, dir.join("audio_recording.wav"));
        let path = store(&dir, "audio_recording.wav", b"RIFF2").await.unwrap();
        assert_eq!(tokio::fs::read(&path).await.unwrap(), b"RIFF2");
        let mut leftovers = tokio::fs::read_dir(&dir).await.unwrap();
        while let Some(entry) = leftovers.next_entry().await.unwrap() {
            let name = entry.file_name().into_string().unwrap();
            assert!(!name.ends_with(".part"), "temporary file left: {name}");
        }

        assert!(store(&dir, "audio_recording.wav", b"").await.is_err());
        assert!(store(&dir, "..", b"RIFF").await.is_err());

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
}
