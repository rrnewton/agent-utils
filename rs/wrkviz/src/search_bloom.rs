//! Trigram Bloom filters at parity with `py/wrkviz/search_bloom.py`.

use std::collections::BTreeSet;

use serde_json::Value;

/// Bloom algorithm identifier.
pub const TRIGRAM_BLOOM_ALGORITHM: &str = "ascii-lower-utf8-trigram-fnv1a32-double-v1";
/// Hash count.
pub const TRIGRAM_BLOOM_HASH_COUNT: i64 = 7;
const BITS_PER_TRIGRAM: i64 = 10;
const MINIMUM_BIT_COUNT: i64 = 64;
const FNV_OFFSET: u32 = 2_166_136_261;
const FNV_PRIME: u32 = 16_777_619;
const SECOND_HASH_SEED: u32 = 0x9E37_79B9;

fn is_bloom_space(ch: char) -> bool {
    matches!(ch,
        '\u{0009}'..='\u{000d}'
        | '\u{001c}'..='\u{0020}'
        | '\u{0085}'
        | '\u{00a0}'
        | '\u{1680}'
        | '\u{2000}'..='\u{200a}'
        | '\u{2028}'
        | '\u{2029}'
        | '\u{202f}'
        | '\u{205f}'
        | '\u{3000}')
}

/// Apply the transcript searcher's whitespace normalization.
pub fn compact_search_text(value: &str) -> String {
    let mut out = String::new();
    let mut in_run = false;
    for ch in value.chars() {
        if is_bloom_space(ch) {
            if !in_run {
                out.push(' ');
                in_run = true;
            }
        } else {
            out.push(ch);
            in_run = false;
        }
    }
    out.trim().to_string()
}

fn ascii_lower_utf8(value: &str) -> Vec<u8> {
    value
        .bytes()
        .map(|byte| {
            if (65..=90).contains(&byte) {
                byte + 32
            } else {
                byte
            }
        })
        .collect()
}

fn search_text_trigrams(value: &str) -> BTreeSet<[u8; 3]> {
    let encoded = ascii_lower_utf8(&compact_search_text(value));
    let mut out = BTreeSet::new();
    if encoded.len() < 3 {
        return out;
    }
    for index in 0..=(encoded.len() - 3) {
        out.insert([encoded[index], encoded[index + 1], encoded[index + 2]]);
    }
    out
}

fn next_power_of_two(value: i64) -> i64 {
    let value = value.max(1);
    let mut power = 1i64;
    while power < value {
        power <<= 1;
    }
    power
}

fn fnv1a32(value: &[u8], seed: u32) -> u32 {
    let mut digest = seed;
    for byte in value {
        digest ^= *byte as u32;
        digest = digest.wrapping_mul(FNV_PRIME);
    }
    digest
}

fn bit_positions(trigram: &[u8; 3], bit_count: i64, hash_count: i64) -> Vec<i64> {
    let first = fnv1a32(trigram, FNV_OFFSET) as i64;
    let second = (fnv1a32(trigram, FNV_OFFSET ^ SECOND_HASH_SEED) | 1) as i64;
    let mask = bit_count - 1;
    (0..hash_count)
        .map(|index| (first + index * second) & mask)
        .collect()
}

/// One immutable Bloom filter plus its catalog parameters.
pub struct TrigramBloom {
    /// Bit count.
    pub bit_count: i64,
    /// Hash count.
    pub hash_count: i64,
    /// Bits.
    pub bits: Vec<u8>,
    /// Trigram count.
    pub trigram_count: i64,
}

impl TrigramBloom {
    /// Deterministic JSON metadata.
    pub fn catalog_obj(&self) -> Value {
        use base64::Engine as _;
        let mut out = serde_json::Map::new();
        out.insert(
            "algorithm".into(),
            Value::String(TRIGRAM_BLOOM_ALGORITHM.into()),
        );
        out.insert("bit_count".into(), self.bit_count.into());
        out.insert("hash_count".into(), self.hash_count.into());
        out.insert(
            "bits_base64".into(),
            Value::String(base64::engine::general_purpose::STANDARD.encode(&self.bits)),
        );
        out.insert("trigram_count".into(), self.trigram_count.into());
        Value::Object(out)
    }
}

/// Build a deterministic filter without cross-record trigrams.
pub fn build_trigram_bloom(texts: &[String]) -> TrigramBloom {
    let mut trigrams: BTreeSet<[u8; 3]> = BTreeSet::new();
    for text in texts {
        trigrams.extend(search_text_trigrams(text));
    }
    let bit_count =
        next_power_of_two((trigrams.len() as i64 * BITS_PER_TRIGRAM).max(MINIMUM_BIT_COUNT));
    let mut bits = vec![0u8; (bit_count / 8) as usize];
    for trigram in &trigrams {
        for position in bit_positions(trigram, bit_count, TRIGRAM_BLOOM_HASH_COUNT) {
            bits[(position / 8) as usize] |= 1u8 << (position % 8);
        }
    }
    TrigramBloom {
        bit_count,
        hash_count: TRIGRAM_BLOOM_HASH_COUNT,
        bits,
        trigram_count: trigrams.len() as i64,
    }
}
