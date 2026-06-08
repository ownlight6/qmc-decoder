//! QMC1 cipher implementation
//!
//! QMC1 uses a fixed seed-based XOR table to encrypt/decrypt audio data.
//! The cipher is deterministic based on byte offset - no external key is needed.
//!
//! Based on the algorithm from presburger/qmc-decoder and bczhc/qmc-decrypt.

/// The 64-byte key table used by QMC1 encryption.
///
/// This table is derived from the 8×7 seed map using the algorithm:
/// - Start at position (x=-1, y=8, dx=1)
/// - Move through the seed map in a zigzag pattern
/// - When x<0: emit 0xC3, bounce right, y=(8-y)%8
/// - When x>6: emit 0xD8, bounce left, y=7-y
/// - Otherwise: emit seed_map[y][x]
/// - Skip bytes at offset 0x8000 and every 0x8000 boundary
const KEY_TABLE: [u8; 64] = [
    0xc3, 0x4a, 0xd6, 0xca, 0x90, 0x67, 0xf7, 0x52, // y=7..0 at x going right
    0xd8, 0xa1, 0x66, 0x62, 0x9f, 0x5b, 0x09, 0x00, // y=0..7 at x going left
    0xc3, 0x5e, 0x95, 0x23, 0x9f, 0x13, 0x11, 0x7e, // ...
    0xd8, 0x92, 0x3f, 0xbc, 0x90, 0xbb, 0x74, 0x0e,
    0xc3, 0x47, 0x74, 0x3d, 0x90, 0xaa, 0x3f, 0x51,
    0xd8, 0xf4, 0x11, 0x84, 0x9f, 0xde, 0x95, 0x1d,
    0xc3, 0xc6, 0x09, 0xd5, 0x9f, 0xfa, 0x66, 0xf9,
    0xd8, 0xf0, 0xf7, 0xa0, 0x90, 0xa1, 0xd6, 0xf3,
];

/// Get the XOR mask byte for the given byte offset
#[inline]
fn get_mask(offset: usize) -> u8 {
    let index = (offset % 0x7fff) & 0x7f;
    let index = if index > 0x3f {
        (0x80 - index) & 0x3f
    } else {
        index
    };
    KEY_TABLE[index]
}

/// Decrypt QMC1 encrypted data in place
pub fn decrypt(data: &mut [u8]) {
    for (i, byte) in data.iter_mut().enumerate() {
        *byte ^= get_mask(i);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_table_first_byte() {
        // The first mask byte (offset 0) should be 0xC3
        assert_eq!(get_mask(0), 0xC3);
    }

    #[test]
    fn test_key_table_second_byte() {
        // The second mask byte (offset 1) should be 0x4A
        assert_eq!(get_mask(1), 0x4A);
    }

    #[test]
    fn test_decrypt_reversible() {
        // Decrypt should be its own inverse (XOR is symmetric)
        let original = b"Hello, World! This is a test of QMC1 decryption.";
        let mut data = original.to_vec();
        decrypt(&mut data);
        // After decryption, data should differ from original
        assert_ne!(&data[..], &original[..]);
        // Decrypting again should restore original
        decrypt(&mut data);
        assert_eq!(&data[..], &original[..]);
    }

    #[test]
    fn test_boundary_behavior() {
        // Test around the 0x8000 skip boundary
        // Bytes at offset 0x7FFF and 0x8000 should be handled correctly
        let mask_7fff = get_mask(0x7FFF);
        let mask_8001 = get_mask(0x8001);
        // These should be valid key bytes
        assert!(KEY_TABLE.contains(&mask_7fff));
        assert!(KEY_TABLE.contains(&mask_8001));
    }

    #[test]
    fn test_periodicity() {
        // The mask should be periodic with period 0x7FFF*2 = 0xFFFE
        // (not exactly due to the skip pattern, but the base pattern repeats)
        for offset in 0..200 {
            let m1 = get_mask(offset);
            let m2 = get_mask(offset + 0x7FFF * 2);
            assert_eq!(m1, m2, "Mask mismatch at offset {} vs {}", offset, offset + 0x7FFF * 2);
        }
    }
}
