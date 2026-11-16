//! Cast device id normalization (mDNS TXT `id=` — 32 hex chars, no hyphens).

/// Normalize a Cast device id: lowercase hex, no hyphens, exactly 32 chars.
pub fn normalize_cast_device_id(raw: &str) -> Option<String> {
    let compact: String = raw
        .chars()
        .filter(|c| *c != '-')
        .collect::<String>()
        .to_ascii_lowercase();
    if compact.len() != 32 || !compact.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some(compact)
}

pub fn cast_device_ids_match(configured: &str, discovered: &str) -> bool {
    match (
        normalize_cast_device_id(configured),
        normalize_cast_device_id(discovered),
    ) {
        (Some(a), Some(b)) => a == b,
        _ => configured.eq_ignore_ascii_case(discovered),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_strips_hyphens_and_lowercases() {
        assert_eq!(
            normalize_cast_device_id("C431E9D7-7CBA-7B8A-3C2F-DE5F-CA5EBA96").as_deref(),
            Some("c431e9d77cba7b8a3c2fde5fca5eba96")
        );
        assert_eq!(
            normalize_cast_device_id("c431e9d77cba7b8a3c2fde5fca5eba96").as_deref(),
            Some("c431e9d77cba7b8a3c2fde5fca5eba96")
        );
    }

    #[test]
    fn normalize_rejects_invalid() {
        assert!(normalize_cast_device_id("too-short").is_none());
        assert!(normalize_cast_device_id("zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz").is_none());
    }
}
