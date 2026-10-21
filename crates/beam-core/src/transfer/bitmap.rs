//! Which chunks the receiver already holds.
//!
//! A bitmap rather than a "resume from index" so that chunks can be re-requested
//! out of order — a chunk that fails its hash on resume is simply cleared and
//! asked for again, wherever it sits in the file.
//!
//! Bit `i` lives in byte `i / 8` at position `i % 8`, least significant bit
//! first. The encoded form is standard base64 of exactly `ceil(count / 8)`
//! bytes, and any bits past the end of the last byte must be zero.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;

/// Why a bitmap from a peer was refused.
///
/// Every one of these is an abort rather than something to repair. A bitmap
/// that does not describe this transfer is either a bug on the other side or an
/// attempt to make the sender skip chunks, and guessing what was meant would
/// turn a detectable problem into a corrupt file.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum BitmapError {
    #[error("the have-bitmap is not valid base64")]
    NotBase64,
    #[error("the have-bitmap is {actual} bytes, but this transfer needs exactly {expected}")]
    WrongLength { expected: usize, actual: usize },
    #[error("the have-bitmap sets bits past the end of the transfer")]
    PaddingBitsSet,
}

/// The set of chunks a receiver already has.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChunkBitmap {
    bits: Vec<u8>,
    len: u32,
}

impl ChunkBitmap {
    /// An empty bitmap for a transfer of `chunk_count` chunks.
    pub fn new(chunk_count: u32) -> Self {
        Self {
            bits: vec![0u8; Self::byte_len(chunk_count)],
            len: chunk_count,
        }
    }

    /// How many bytes a bitmap of `chunk_count` chunks occupies.
    pub fn byte_len(chunk_count: u32) -> usize {
        chunk_count.div_ceil(8) as usize
    }

    /// How many chunks this bitmap describes.
    pub fn len(&self) -> u32 {
        self.len
    }

    /// Whether the transfer has no chunks at all, as an empty file does.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Marks a chunk as held. Out-of-range indices are ignored.
    pub fn set(&mut self, index: u32) {
        if index < self.len {
            self.bits[(index / 8) as usize] |= 1 << (index % 8);
        }
    }

    /// Marks a chunk as not held.
    pub fn clear(&mut self, index: u32) {
        if index < self.len {
            self.bits[(index / 8) as usize] &= !(1 << (index % 8));
        }
    }

    /// Whether a chunk is held.
    pub fn get(&self, index: u32) -> bool {
        index < self.len && self.bits[(index / 8) as usize] & (1 << (index % 8)) != 0
    }

    /// How many chunks are held.
    pub fn count(&self) -> u32 {
        (0..self.len).filter(|i| self.get(*i)).count() as u32
    }

    /// Whether every chunk is held.
    pub fn is_complete(&self) -> bool {
        self.count() == self.len
    }

    /// The chunks that are still needed, in order.
    pub fn missing(&self) -> impl Iterator<Item = u32> + '_ {
        (0..self.len).filter(|i| !self.get(*i))
    }

    /// Standard base64 of the raw bytes.
    pub fn encode(&self) -> String {
        BASE64.encode(&self.bits)
    }

    /// Parses a bitmap, insisting that it describes exactly this transfer.
    pub fn decode(text: &str, chunk_count: u32) -> Result<Self, BitmapError> {
        let bits = BASE64.decode(text).map_err(|_| BitmapError::NotBase64)?;

        let expected = Self::byte_len(chunk_count);
        if bits.len() != expected {
            return Err(BitmapError::WrongLength {
                expected,
                actual: bits.len(),
            });
        }

        // Bits past the last chunk carry no meaning, so a peer setting them is
        // saying something it cannot mean.
        let spare = (expected * 8) as u32 - chunk_count;
        if spare > 0 {
            let mask = !0u8 << (8 - spare);
            if bits[expected - 1] & mask != 0 {
                return Err(BitmapError::PaddingBitsSet);
            }
        }

        Ok(Self {
            bits,
            len: chunk_count,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_bitmap_holds_nothing() {
        let bitmap = ChunkBitmap::new(20);
        assert_eq!(bitmap.len(), 20);
        assert_eq!(bitmap.count(), 0);
        assert!(!bitmap.is_complete());
        assert_eq!(
            bitmap.missing().collect::<Vec<_>>(),
            (0..20).collect::<Vec<_>>()
        );
    }

    #[test]
    fn setting_and_clearing_one_chunk_leaves_the_rest_alone() {
        let mut bitmap = ChunkBitmap::new(20);
        bitmap.set(7);
        assert!(bitmap.get(7));
        assert_eq!(bitmap.count(), 1);
        for i in (0..20).filter(|i| *i != 7) {
            assert!(!bitmap.get(i), "chunk {i} was set too");
        }

        bitmap.clear(7);
        assert!(!bitmap.get(7));
        assert_eq!(bitmap.count(), 0);
    }

    #[test]
    fn every_index_can_be_set_independently() {
        for len in [1u32, 7, 8, 9, 16, 17, 100] {
            for index in 0..len {
                let mut bitmap = ChunkBitmap::new(len);
                bitmap.set(index);
                assert_eq!(bitmap.count(), 1, "len={len} index={index}");
                assert!(bitmap.get(index), "len={len} index={index}");
            }
        }
    }

    #[test]
    fn out_of_range_indices_are_ignored_rather_than_panicking() {
        let mut bitmap = ChunkBitmap::new(8);
        bitmap.set(8);
        bitmap.set(u32::MAX);
        assert_eq!(bitmap.count(), 0);
        assert!(!bitmap.get(8));
        bitmap.clear(1000);
    }

    #[test]
    fn a_full_bitmap_is_complete_and_misses_nothing() {
        let mut bitmap = ChunkBitmap::new(10);
        for i in 0..10 {
            bitmap.set(i);
        }
        assert!(bitmap.is_complete());
        assert_eq!(bitmap.missing().count(), 0);
    }

    #[test]
    fn an_empty_transfer_is_complete_immediately() {
        let bitmap = ChunkBitmap::new(0);
        assert!(bitmap.is_empty());
        assert!(bitmap.is_complete());
        assert_eq!(bitmap.encode(), "");
    }

    #[test]
    fn a_bitmap_round_trips_through_its_encoding() {
        for len in [0u32, 1, 8, 9, 63, 64, 65, 1000] {
            let mut bitmap = ChunkBitmap::new(len);
            for i in (0..len).step_by(3) {
                bitmap.set(i);
            }
            let text = bitmap.encode();
            let back = ChunkBitmap::decode(&text, len).expect("decode");
            assert_eq!(back, bitmap, "len={len}");
        }
    }

    #[test]
    fn the_encoded_length_is_exactly_one_bit_per_chunk() {
        for (chunks, bytes) in [(0u32, 0usize), (1, 1), (8, 1), (9, 2), (16, 2), (17, 3)] {
            assert_eq!(ChunkBitmap::byte_len(chunks), bytes, "chunks={chunks}");
        }
    }

    #[test]
    fn decoding_refuses_a_bitmap_of_the_wrong_length() {
        // Ten chunks need two bytes; one byte and three bytes are both wrong.
        let short = BASE64.encode([0u8; 1]);
        let long = BASE64.encode([0u8; 3]);
        for text in [short, long] {
            assert!(matches!(
                ChunkBitmap::decode(&text, 10),
                Err(BitmapError::WrongLength { expected: 2, .. })
            ));
        }
    }

    #[test]
    fn decoding_refuses_bits_past_the_end_of_the_transfer() {
        // Ten chunks occupy bits 0..=9, so bits 10..=15 of the second byte must
        // be zero. A peer setting one of them is claiming a chunk that does not
        // exist.
        for spare_bit in 2..8u32 {
            let mut raw = [0u8; 2];
            raw[1] |= 1 << spare_bit;
            let text = BASE64.encode(raw);
            assert_eq!(
                ChunkBitmap::decode(&text, 10),
                Err(BitmapError::PaddingBitsSet),
                "bit {spare_bit} of the last byte"
            );
        }

        // The bits that do belong to chunks are fine.
        for real_bit in 0..2u32 {
            let mut raw = [0u8; 2];
            raw[1] |= 1 << real_bit;
            let text = BASE64.encode(raw);
            assert!(ChunkBitmap::decode(&text, 10).is_ok(), "bit {real_bit}");
        }
    }

    #[test]
    fn decoding_refuses_something_that_is_not_base64() {
        assert_eq!(
            ChunkBitmap::decode("not base64!!", 10),
            Err(BitmapError::NotBase64)
        );
    }

    #[test]
    fn a_bitmap_for_a_different_transfer_is_refused() {
        // The realistic attack: a bitmap that is valid for some other transfer
        // is offered for this one, to make the sender skip chunks.
        let mut other = ChunkBitmap::new(64);
        for i in 0..64 {
            other.set(i);
        }
        assert!(matches!(
            ChunkBitmap::decode(&other.encode(), 100),
            Err(BitmapError::WrongLength { .. })
        ));
    }
}
