//! QMC2 cipher implementation
//!
//! QMC2 uses an ekey (encryption key) to decrypt audio data.
//! The ekey is a base64-encoded string that is decoded and then
//! further decrypted using Tencent's TC-TEA algorithm.
//!
//! Two sub-algorithms based on decoded key length:
//! - Key length <= 300: QMC2 Map (simple XOR with scrambled key)
//! - Key length > 300: QMC2 RC4 (modified RC4 stream cipher)
//!
//! Based on the algorithm from jixunmoe/qmc2-rust and bczhc/qmc-decrypt.

use base64::Engine;

/// Errors that can occur during QMC2 operations
#[derive(Debug)]
pub enum Qmc2Error {
    /// Failed to decode the base64 ekey
    Base64DecodeError(base64::DecodeError),
    /// Failed to derive the key from the ekey (TC-TEA decryption failed)
    KeyDeriveError,
    /// The decoded key is too short
    KeyTooShort,
}

impl std::fmt::Display for Qmc2Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Qmc2Error::Base64DecodeError(e) => write!(f, "Base64 decode error: {}", e),
            Qmc2Error::KeyDeriveError => {
                write!(f, "Failed to derive key from ekey (TC-TEA decryption failed)")
            }
            Qmc2Error::KeyTooShort => write!(f, "Decoded key is too short (< 8 bytes)"),
        }
    }
}

impl std::error::Error for Qmc2Error {}

/// Generate a simple key from a seed value (used in the key derivation process)
fn simple_make_key(seed: u8, size: usize) -> Vec<u8> {
    let mut result = vec![0u8; size];
    for (i, byte) in result.iter_mut().enumerate() {
        let value = (seed as f32) + (i as f32) * 0.1;
        *byte = (100.0 * value.tan().abs()) as u8;
    }
    result
}

/// Derive the TEA key from the ekey header (first 8 bytes)
fn derive_tea_key(ekey_header: &[u8]) -> [u8; 16] {
    let simple_key_buf = simple_make_key(106, 8);

    let mut tea_key = [0u8; 16];
    for i in (0..16).step_by(2) {
        tea_key[i] = simple_key_buf[i / 2];
        tea_key[i + 1] = ekey_header[i / 2];
    }
    tea_key
}

/// EncV2 prefix used in newer ekey formats
const QMC2_ENCV2_PREFIX: &[u8] = b"QQMusic EncV2,Key:";
/// EncV2 stage 1 key
const QMC2_ENCV2_STAGE1_KEY: &[u8] = b"386ZJY!@#*$%^&)(";
/// EncV2 stage 2 key
const QMC2_ENCV2_STAGE2_KEY: &[u8] = b"**#!(#$%&^a1cZ,T";

/// Parse an ekey string and derive the raw decryption key
fn parse_ekey(ekey: &str) -> Result<Vec<u8>, Qmc2Error> {
    let ekey_trimmed = ekey.trim_matches(char::from(0));
    let ekey_decoded = base64::engine::general_purpose::STANDARD
        .decode(ekey_trimmed)
        .map_err(Qmc2Error::Base64DecodeError)?;

    if ekey_decoded.is_empty() {
        return Err(Qmc2Error::KeyTooShort);
    }

    // Check for EncV2 prefix
    let ekey_decoded = if ekey_decoded.starts_with(QMC2_ENCV2_PREFIX) {
        let encv2_blob = &ekey_decoded[QMC2_ENCV2_PREFIX.len()..];
        let encv2_stage1 = tc_tea::decrypt(encv2_blob, QMC2_ENCV2_STAGE1_KEY)
            .ok_or(Qmc2Error::KeyDeriveError)?;
        let encv2_stage2 =
            tc_tea::decrypt(&encv2_stage1, QMC2_ENCV2_STAGE2_KEY).ok_or(Qmc2Error::KeyDeriveError)?;
        let encv1_ekey = base64::engine::general_purpose::STANDARD
            .decode(&encv2_stage2)
            .map_err(Qmc2Error::Base64DecodeError)?;
        encv1_ekey
    } else {
        ekey_decoded
    };

    if ekey_decoded.len() < 8 {
        return Err(Qmc2Error::KeyTooShort);
    }

    // Try EncV1 parsing: split into header (8 bytes) and body, decrypt body with TC-TEA
    // If TC-TEA decryption fails, the ekey might be a raw key (e.g., from the QQ Music API)
    // that should be used directly without TC-TEA processing.
    let (header, body) = ekey_decoded.split_at(8);

    if body.is_empty() {
        // No body to decrypt, header is the entire key
        return Ok(header.to_vec());
    }

    // Derive TEA key from header
    let tea_key = derive_tea_key(header);

    // Try TC-TEA decryption (EncV1 format)
    if let Some(decrypted_body) = tc_tea::decrypt(body, &tea_key) {
        // Successfully decrypted EncV1 format
        let mut result = Vec::with_capacity(8 + decrypted_body.len());
        result.extend_from_slice(header);
        result.extend_from_slice(&decrypted_body);
        Ok(result)
    } else {
        // TC-TEA decryption failed — this is likely a raw key (e.g., from the API)
        // Use the entire decoded blob as the key directly
        Ok(ekey_decoded)
    }
}

// ============================================================
// QMC2 Map cipher (for key length <= 300)
// ============================================================

/// QMC2 Map-based cipher (for short keys, <= 300 bytes)
struct Qmc2MapCrypto {
    key: Vec<u8>,
}

impl Qmc2MapCrypto {
    fn new(key: &[u8]) -> Self {
        Qmc2MapCrypto {
            key: key.to_vec(),
        }
    }

    /// Scramble a key byte by its index (bit rotation)
    #[inline]
    fn scramble_by_index(value: u8, index: usize) -> u8 {
        let rotation = ((index as u32).wrapping_add(4)) & 0b111;
        let left = value.wrapping_shl(rotation);
        let right = value.wrapping_shr(rotation);
        left | right
    }

    /// Get the XOR mask byte for the given offset
    #[inline]
    fn map_l(&self, offset: usize) -> u8 {
        let mut offset_local = offset;
        if offset_local > 0x7FFF {
            offset_local %= 0x7FFF;
        }
        let index = (offset_local * offset_local + 71214) % self.key.len();
        Self::scramble_by_index(self.key[index], index)
    }

    /// Decrypt data in place starting at the given offset
    fn decrypt(&self, offset: usize, buf: &mut [u8]) {
        for (i, byte) in buf.iter_mut().enumerate() {
            *byte ^= self.map_l(offset + i);
        }
    }
}

// ============================================================
// QMC2 RC4 cipher (for key length > 300)
// ============================================================

const FIRST_SEGMENT_SIZE: usize = 0x80;
const OTHER_SEGMENT_SIZE: usize = 0x1400;

/// QMC2 RC4-based cipher (for long keys, > 300 bytes)
struct Qmc2Rc4Crypto {
    /// RC4 seed box (S-box)
    s: Vec<u8>,
    /// Hash base for segment key calculation
    hash: u32,
    /// RC4 key
    rc4_key: Vec<u8>,
}

impl Qmc2Rc4Crypto {
    fn new(rc4_key: &[u8]) -> Self {
        let n = rc4_key.len();

        // Initialize S-box
        // QMC2 RC4 uses a variable-size S-box equal to the key length.
        // For key lengths > 255, we use the full key length as the S-box size.
        // Standard RC4 uses 256, but QMC2 uses the key length.
        let mut s: Vec<u8> = (0..n as u8).collect();
        // For keys longer than 255 bytes, we need a larger S-box
        // and the values wrap modulo 256 (standard byte range)
        if n > 256 {
            s = (0..=255u8).collect();
            s.extend((0..=255u8).cycle().take(n - 256));
        }

        // KSA (Key Scheduling Algorithm)
        let mut j = 0usize;
        for i in 0..n {
            j = (j + s[i] as usize + rc4_key[i] as usize) % n;
            s.swap(i, j);
        }

        Qmc2Rc4Crypto {
            s,
            hash: Self::calc_hash_base(rc4_key),
            rc4_key: rc4_key.to_vec(),
        }
    }

    /// Calculate hash base from RC4 key
    fn calc_hash_base(data: &[u8]) -> u32 {
        let mut hash: u32 = 1;
        for &value in data.iter() {
            let value = u32::from(value);
            if value == 0 {
                continue;
            }
            let next_hash = hash.wrapping_mul(value);
            if next_hash == 0 || next_hash <= hash {
                break;
            }
            hash = next_hash;
        }
        hash
    }

    /// Calculate segment key
    #[inline]
    fn calc_segment_key(&self, id: usize, seed: u8) -> usize {
        let dividend = f64::from(self.hash);
        let divisor = ((id + 1) * usize::from(seed)) as f64;
        let key = dividend / divisor * 100.0;
        key as u64 as usize
    }

    /// RC4 PRGA - derive one byte
    #[inline]
    fn rc4_derive(n: usize, s: &mut Vec<u8>, j: &mut usize, k: &mut usize) -> u8 {
        *j = (*j + 1) % n;
        *k = (usize::from(s[*j]) + *k) % n;
        s.swap(*j, *k);
        let index = usize::from(s[*j]) + usize::from(s[*k]);
        s[index % n]
    }

    /// Encrypt/decrypt first segment (offset < 0x80)
    fn encode_first_segment(&self, offset: usize, buf: &mut [u8]) {
        let n = self.rc4_key.len();
        let mut offset = offset;
        for byte in buf.iter_mut() {
            let key1 = self.rc4_key[offset % n];
            let key2 = self.calc_segment_key(offset, key1);
            *byte ^= self.rc4_key[key2 % n];
            offset += 1;
        }
    }

    /// Encrypt/decrypt other segments (offset >= 0x80)
    fn encode_other_segment(&self, offset: usize, buf: &mut [u8]) {
        let seg_id = offset / OTHER_SEGMENT_SIZE;
        let seg_id_small = seg_id & 0x1FF;

        let mut discard_count = self.calc_segment_key(seg_id, self.rc4_key[seg_id_small]) & 0x1FF;
        discard_count += offset % OTHER_SEGMENT_SIZE;

        let n = self.rc4_key.len();
        let mut s = self.s.clone();
        let mut j = 0usize;
        let mut k = 0usize;
        for _ in 0..discard_count {
            Self::rc4_derive(n, &mut s, &mut j, &mut k);
        }

        for byte in buf.iter_mut() {
            *byte ^= Self::rc4_derive(n, &mut s, &mut j, &mut k);
        }
    }

    /// Decrypt data in place starting at the given offset
    fn decrypt(&self, offset: usize, buf: &mut [u8]) {
        let mut offset = offset;
        let mut len = buf.len();
        let mut i = 0usize;

        // First segment has a different algorithm
        if offset < FIRST_SEGMENT_SIZE {
            let len_processed = std::cmp::min(len, FIRST_SEGMENT_SIZE - offset);
            self.encode_first_segment(offset, &mut buf[i..i + len_processed]);
            i += len_processed;
            len -= len_processed;
            offset += len_processed;
        }

        // Align to segment boundary
        let to_align = offset % OTHER_SEGMENT_SIZE;
        if to_align != 0 {
            let len_processed = std::cmp::min(len, OTHER_SEGMENT_SIZE - to_align);
            self.encode_other_segment(offset, &mut buf[i..i + len_processed]);
            i += len_processed;
            len -= len_processed;
            offset += len_processed;
        }

        // Process full segments
        while len > OTHER_SEGMENT_SIZE {
            self.encode_other_segment(offset, &mut buf[i..i + OTHER_SEGMENT_SIZE]);
            i += OTHER_SEGMENT_SIZE;
            len -= OTHER_SEGMENT_SIZE;
            offset += OTHER_SEGMENT_SIZE;
        }

        // Remaining bytes
        if len > 0 {
            self.encode_other_segment(offset, &mut buf[i..i + len]);
        }
    }
}

// ============================================================
// Public API
// ============================================================

/// QMC2 crypto implementation supporting both Map and RC4 ciphers
pub struct Qmc2Crypto {
    inner: Qmc2CryptoInner,
}

enum Qmc2CryptoInner {
    Map(Qmc2MapCrypto),
    Rc4(Qmc2Rc4Crypto),
}

impl Qmc2Crypto {
    /// Create a QMC2 crypto instance from a base64-encoded ekey
    pub fn from_ekey(ekey: &str) -> Result<Self, Qmc2Error> {
        let key = parse_ekey(ekey)?;
        if key.len() > 300 {
            Ok(Qmc2Crypto {
                inner: Qmc2CryptoInner::Rc4(Qmc2Rc4Crypto::new(&key)),
            })
        } else {
            Ok(Qmc2Crypto {
                inner: Qmc2CryptoInner::Map(Qmc2MapCrypto::new(&key)),
            })
        }
    }

    /// Decrypt data in place starting at the given file offset
    pub fn decrypt(&self, offset: usize, buf: &mut [u8]) {
        match &self.inner {
            Qmc2CryptoInner::Map(crypto) => crypto.decrypt(offset, buf),
            Qmc2CryptoInner::Rc4(crypto) => crypto.decrypt(offset, buf),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_make_key() {
        let result = simple_make_key(106, 8);
        assert_eq!(
            result,
            vec![0x69, 0x56, 0x46, 0x38, 0x2b, 0x20, 0x15, 0x0b]
        );
    }

    #[test]
    fn test_derive_tea_key() {
        let ekey_header = [0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8];
        let tea_key = derive_tea_key(&ekey_header);
        assert_eq!(
            tea_key,
            [
                0x69, 0xf1, 0x56, 0xf2, 0x46, 0xf3, 0x38, 0xf4, 0x2b, 0xf5, 0x20, 0xf6, 0x15,
                0xf7, 0x0b, 0xf8,
            ]
        );
    }

    #[test]
    fn test_parse_ekey() {
        let ekey = "VGhpcyBpcyBHFWEh4cjZ1Vi7rJ56XeoPlqGM1sxBGPg7mt89umKclFBr9iqfmFdS";
        let key = parse_ekey(ekey).unwrap();
        assert_eq!(
            std::str::from_utf8(&key).unwrap(),
            "This is a test key for test purpose :D"
        );
    }

    #[test]
    fn test_parse_ekey_roundtrip() {
        // Test that we can encrypt and decrypt with tc_tea
        let test_key = b"12345678...test data by Jixun";
        let (header, body) = test_key.split_at(8);
        let tea_key = derive_tea_key(header);

        let encrypted_body = tc_tea::encrypt(body, &tea_key).unwrap();
        let ekey_encoded = [&header as &[u8], &*encrypted_body].concat();
        let ekey = base64::engine::general_purpose::STANDARD.encode(&ekey_encoded);

        let parsed_key = parse_ekey(&ekey).unwrap();
        assert_eq!(&parsed_key, test_key);
    }

    #[test]
    fn test_map_l() {
        let key: [u8; 16] = [
            0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4A, 0x4B, 0x4C, 0x4D, 0x4E,
            0x4F, 0x50,
        ];
        let crypto = Qmc2MapCrypto::new(&key);
        let mut data = [0u8; 16];
        crypto.decrypt(0, &mut data);
        assert_eq!(
            data,
            [
                0x3F, 0x8A, 0xC1, 0x49, 0x3F, 0x49, 0xC1, 0x8A, 0x3F, 0x8A, 0xC1, 0x49, 0x3F,
                0x49, 0xC1, 0x8A
            ]
        );
    }

    #[test]
    fn test_map_l_boundary() {
        let key: [u8; 16] = [
            0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4A, 0x4B, 0x4C, 0x4D, 0x4E,
            0x4F, 0x50,
        ];
        let crypto = Qmc2MapCrypto::new(&key);
        let mut data = [0u8; 16];
        crypto.decrypt(0x7FFF - 8, &mut data);
        assert_eq!(
            data,
            [
                0x8A, 0x3F, 0x8A, 0xC1, 0x49, 0x3F, 0x49, 0xC1, 0x8A, 0x8A, 0xC1, 0x49, 0x3F,
                0x49, 0xC1, 0x8A
            ]
        );
    }

    #[test]
    fn test_rc4_hash_base() {
        let hash = Qmc2Rc4Crypto::calc_hash_base(&[1u8, 99]);
        assert_eq!(hash, 1);

        let hash = Qmc2Rc4Crypto::calc_hash_base(&[
            0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
            0xff, 0xff,
        ]);
        assert_eq!(hash, 0xfc05fc01);
    }

    #[test]
    fn test_rc4_first_segment() {
        let mut rc4_key = [0u8; 255];
        for (i, p) in rc4_key.iter_mut().enumerate() {
            *p = i as u8;
        }
        let crypto = Qmc2Rc4Crypto::new(&rc4_key);
        let mut data = [0u8; 16];
        crypto.decrypt(0, &mut data);
        assert_eq!(
            data,
            [0, 50, 16, 8, 5, 3, 2, 1, 1, 1, 0, 0, 0, 0, 0, 0]
        );
    }
}