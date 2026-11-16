//! CRC32 helpers for Miniserver wire config (matches lox-audioserver / Loxone).

use crc32fast::hash;

/// CRC32 of config bytes as 8-digit lowercase hex.
pub fn crc32_hex(data: &[u8]) -> String {
    format!("{:08x}", hash(data))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_hex_is_zero_padded() {
        assert_eq!(crc32_hex(b"[]").len(), 8);
    }

    #[test]
    fn crc_changes_when_wire_json_differs() {
        let a = br#"[{"504F94FF1BB3":{"master":"504F94000002","shareMode":1}}]"#;
        let b = br#"[{"504F94FF1BB3":{"master":"504F94000002"}}]"#;
        assert_ne!(crc32_hex(a), crc32_hex(b));
    }
}
