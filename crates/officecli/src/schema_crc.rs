include!(concat!(env!("OUT_DIR"), "/schema_entries.rs"));

/// CRC32 fingerprint of the embedded help-schema tree.
pub(crate) fn compute() -> String {
    compute_entries(SCHEMA_ENTRIES)
}

fn compute_entries(entries: &[(&str, &[u8])]) -> String {
    let mut canonical_entries: Vec<(String, &[u8])> = entries
        .iter()
        .map(|(name, bytes)| (name.replace('\\', "/").to_lowercase(), *bytes))
        .collect();
    canonical_entries.sort_by(|left, right| left.0.cmp(&right.0));

    let mut crc = 0xffff_ffff_u32;
    for (canonical, bytes) in canonical_entries {
        crc = append(crc, canonical.as_bytes());
        crc = append(crc, bytes);
    }
    format!("{:08x}", crc ^ 0xffff_ffff)
}

fn append(mut crc: u32, bytes: &[u8]) -> u32 {
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                0xedb8_8320_u32 ^ (crc >> 1)
            } else {
                crc >> 1
            };
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::{append, compute, compute_entries};

    #[test]
    fn crc32_matches_standard_check_vector() {
        let crc = append(0xffff_ffff, b"123456789") ^ 0xffff_ffff;
        assert_eq!(crc, 0xcbf4_3926);
    }

    #[test]
    fn entry_order_and_path_separators_are_canonicalized() {
        let forward = compute_entries(&[
            ("schemas/help/docx/a.json", b"alpha"),
            ("schemas/help/xlsx/b.json", b"beta"),
        ]);
        let reversed = compute_entries(&[
            ("SCHEMAS\\HELP\\XLSX\\B.JSON", b"beta"),
            ("SCHEMAS\\HELP\\DOCX\\A.JSON", b"alpha"),
        ]);
        assert_eq!(forward, reversed);
    }

    #[test]
    fn canonical_name_contributes_to_fingerprint() {
        let first = compute_entries(&[("schemas/help/docx/a.json", b"same")]);
        let second = compute_entries(&[("schemas/help/docx/b.json", b"same")]);
        assert_ne!(first, second);
    }

    #[test]
    fn embedded_schema_fingerprint_is_lowercase_hex() {
        let fingerprint = compute();
        assert_eq!(fingerprint.len(), 8);
        assert!(fingerprint
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
    }
}
