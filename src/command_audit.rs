//! Inbound command logging for 7095 vs 7091 traffic-path audits.

use std::io::Write;
use tracing::info;

pub const PORT_MINISERVER: u16 = 7095;
pub const PORT_REMOTE: u16 = 7091;

/// Log every inbound wire command with explicit port and api role (info level).
pub fn log_inbound(port: u16, session_token: &str, command: &str) {
    let api = if port == PORT_REMOTE {
        "remote"
    } else {
        "miniserver"
    };
    info!(port, api, session_token, command, "loxone inbound command");
    if audit_enabled() {
        audit_append(port, command);
    }
}

fn audit_enabled() -> bool {
    std::env::var("LOXONE_AUDIT")
        .ok()
        .is_some_and(|v| matches!(v.as_str(), "1" | "true" | "yes"))
}

fn audit_append(port: u16, command: &str) {
    let line = format!("{port}\t{command}\n");
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("command-audit.log")
    {
        let _ = f.write_all(line.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static AUDIT_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn port_constants() {
        assert_eq!(PORT_MINISERVER, 7095);
        assert_eq!(PORT_REMOTE, 7091);
    }

    #[test]
    fn log_inbound_writes_audit_log_when_enabled() {
        let _guard = AUDIT_TEST_LOCK.lock().expect("audit test lock");
        let dir = std::env::temp_dir().join(format!(
            "loxone-audit-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("create temp audit dir");
        let log_path = dir.join("command-audit.log");
        let orig_dir = std::env::current_dir().expect("cwd");
        std::env::set_current_dir(&dir).expect("chdir temp");
        // SAFETY: serialized by AUDIT_TEST_LOCK; restored before return.
        unsafe {
            std::env::set_var("LOXONE_AUDIT", "1");
        }

        log_inbound(PORT_REMOTE, "sess-1", "audio/1/play");

        let contents = std::fs::read_to_string(&log_path).expect("audit log");
        assert!(contents.contains("7091\taudio/1/play"));

        unsafe {
            std::env::remove_var("LOXONE_AUDIT");
        }
        std::env::set_current_dir(&orig_dir).expect("restore cwd");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
