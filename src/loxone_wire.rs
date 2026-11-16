//! Shared Loxone wire reply helpers (7091 + 7095 adapters).

use anyhow::{Context, Result};
use base64::prelude::*;
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::wire_response::build_wire_result_response;
use tracing::warn;

/// Standard stub for unimplemented wire commands (`error: -1`).
pub fn not_implemented_reply(command: &str) -> Result<String> {
    warn!(command, "wire command not implemented");
    Ok(format!(
        "{{\"error\":-1,\"command\":{}}}",
        serde_json::to_string(command)?
    ))
}

/// Strip a trailing `/{param}` segment from the echoed command path.
pub fn strip_param<'a>(command: &'a str, param: &'a str) -> &'a str {
    let mut pat = String::from("/");
    pat.push_str(param);
    command.trim_end_matches(&pat)
}

/// Decode standard base64 (no pad) JSON payload from a `{data}` route segment.
pub fn decode_base64_json<T: DeserializeOwned>(data: &str) -> Result<T> {
    let decoded = BASE64_STANDARD_NO_PAD
        .decode(data)
        .context("Invalid base64 data segment")?;
    serde_json::from_slice(&decoded).context("Invalid JSON in base64 data segment")
}

/// Build a config command reply with stripped command echo.
pub fn config_data_reply<T: Serialize>(
    result_key: &str,
    payload: T,
    command: &str,
    param: &str,
) -> Result<String> {
    build_wire_result_response(
        result_key,
        serde_json::to_value(payload)?,
        Some(strip_param(command, param)),
        &[],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Deserialize, Serialize, PartialEq)]
    struct Sample {
        value: i32,
    }

    #[test]
    fn not_implemented_reply_shape() {
        let wire = not_implemented_reply("audio/cfg/forget").unwrap();
        let body: serde_json::Value = serde_json::from_str(&wire).unwrap();
        assert_eq!(body["error"], -1);
        assert_eq!(body["command"], "audio/cfg/forget");
    }

    #[test]
    fn strip_param_removes_trailing_segment() {
        assert_eq!(
            strip_param("audio/cfg/volumes/abc123", "abc123"),
            "audio/cfg/volumes"
        );
        assert_eq!(
            strip_param("audio/cfg/miniservertime/1779215298", "1779215298"),
            "audio/cfg/miniservertime"
        );
    }

    #[test]
    fn decode_base64_json_roundtrip() {
        let encoded = BASE64_STANDARD_NO_PAD.encode(br#"{"value":42}"#);
        let parsed: Sample = decode_base64_json(&encoded).unwrap();
        assert_eq!(parsed, Sample { value: 42 });
    }

    #[test]
    fn decode_base64_json_rejects_invalid() {
        assert!(decode_base64_json::<Sample>("!!!").is_err());
    }

    #[test]
    fn config_data_reply_puts_result_first() {
        let wire = config_data_reply(
            "groupopts_result",
            vec!["g1"],
            "audio/cfg/groupopts/abc",
            "abc",
        )
        .unwrap();
        assert_eq!(
            crate::wire_response::first_wire_key(&wire).as_deref(),
            Some("groupopts_result")
        );
        let body: serde_json::Value = serde_json::from_str(&wire).unwrap();
        assert_eq!(body["command"], "audio/cfg/groupopts");
    }
}
