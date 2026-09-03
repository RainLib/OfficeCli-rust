use crate::HcdError;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::Path;

pub fn hash_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex(&hasher.finalize())
}

pub fn hash_reader(reader: &mut impl Read) -> Result<String, HcdError> {
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex(&hasher.finalize()))
}

pub fn hash_file(path: impl AsRef<Path>) -> Result<String, HcdError> {
    hash_reader(&mut std::fs::File::open(path)?)
}

pub fn stable_node_id(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part.as_bytes());
    }
    let encoded = hex(&hasher.finalize());
    format!("n_{}", &encoded[..32])
}

pub fn node_bloom<'a>(ids: impl IntoIterator<Item = &'a str>) -> String {
    let mut bits = [0u8; 32];
    for id in ids {
        let digest = Sha256::digest(id.as_bytes());
        for offset in [0usize, 2, 4, 6] {
            let bit = u16::from_be_bytes([digest[offset], digest[offset + 1]]) as usize % 256;
            bits[bit / 8] |= 1 << (bit % 8);
        }
    }
    hex(&bits)
}

pub fn node_bloom_might_contain(bloom: &str, id: &str) -> bool {
    let Some(bits) = decode_hex_32(bloom) else {
        return true;
    };
    let digest = Sha256::digest(id.as_bytes());
    [0usize, 2, 4, 6].into_iter().all(|offset| {
        let bit = u16::from_be_bytes([digest[offset], digest[offset + 1]]) as usize % 256;
        bits[bit / 8] & (1 << (bit % 8)) != 0
    })
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn decode_hex_32(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (index, slot) in out.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bloom_rejects_no_inserted_values() {
        let ids = ["n_a", "n_b", "n_c"];
        let bloom = node_bloom(ids);
        for id in ids {
            assert!(node_bloom_might_contain(&bloom, id));
        }
    }

    #[test]
    fn stable_ids_are_repeatable() {
        assert_eq!(
            stable_node_id(&["doc", "word/document.xml", "1"]),
            stable_node_id(&["doc", "word/document.xml", "1"])
        );
    }
}
