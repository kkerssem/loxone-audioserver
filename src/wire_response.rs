//! Manual JSON wire responses with `*_result` before `command` (Loxone UI convention).

use anyhow::Result;
use serde_json::Value;

/// Build a wire JSON object with `{result_key}` first, then optional extras, then `command`.
///
/// `serde_json`'s default map uses alphabetical key order, so `json!({ "command", "..._result" })`
/// still emits `"command"` first unless built manually.
pub fn build_wire_result_response(
    result_key: &str,
    payload: Value,
    command: Option<&str>,
    extra: &[(&str, Value)],
) -> Result<String> {
    let mut parts = vec![format!(
        "\"{result_key}\":{}",
        serde_json::to_string(&payload)?
    )];
    for (key, value) in extra {
        parts.push(format!("\"{key}\":{}", serde_json::to_string(value)?));
    }
    if let Some(command) = command {
        parts.push(format!("\"command\":{}", serde_json::to_string(command)?));
    }
    Ok(format!("{{{}}}", parts.join(",")))
}

/// First key in the wire JSON object (byte order), not serde_json map order.
#[cfg(test)]
pub fn first_wire_key(json: &str) -> Option<String> {
    let rest = json.trim().trim_start_matches('{').trim_start();
    let end = rest.find(':')?;
    let key = rest[..end].trim().trim_matches('"');
    if key.is_empty() {
        None
    } else {
        Some(key.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn result_key_precedes_command() {
        let wire =
            build_wire_result_response("pause_result", json!([]), Some("audio/1/pause"), &[])
                .unwrap();
        assert_eq!(first_wire_key(&wire).as_deref(), Some("pause_result"));
        assert!(wire.starts_with("{\"pause_result\":"));
    }

    #[test]
    fn extras_between_result_and_command() {
        let wire = build_wire_result_response(
            "globalsearch_result",
            json!({ "unique": "abc" }),
            None,
            &[("type", json!("spotify")), ("unique", json!("abc"))],
        )
        .unwrap();
        assert_eq!(
            first_wire_key(&wire).as_deref(),
            Some("globalsearch_result")
        );
    }

    #[test]
    fn servicecfg_result_key_precedes_command() {
        let wire = build_wire_result_response(
            "servicecfg_result",
            json!({ "link": "http://192.168.1.1:7091/qobuz/oauth/start" }),
            Some("audio/cfg/servicecfg/getlink/spotify"),
            &[],
        )
        .unwrap();
        assert_eq!(first_wire_key(&wire).as_deref(), Some("servicecfg_result"));
    }
}
