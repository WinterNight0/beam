//! Splitting a file into chunks, and hashing them.

use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt};

use crate::hex;

/// How a file divides into chunks.
///
/// A chunk is the unit of hashing and, from M3, of resume. It is not the unit
/// of transmission; see [`super::frame`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChunkPlan {
    size: u64,
    chunk_size: u32,
}

impl ChunkPlan {
    /// Plans a file of `size` bytes into chunks of at most `chunk_size`.
    ///
    /// # Panics
    ///
    /// Panics if `chunk_size` is zero, which is a programming error rather than
    /// a peer's doing — a request from a peer goes through
    /// [`ChunkPlan::try_new`] instead.
    pub fn new(size: u64, chunk_size: u32) -> Self {
        assert!(chunk_size > 0, "chunk size must be positive");
        Self { size, chunk_size }
    }

    /// Plans a file described by a peer, rejecting values that do not add up.
    ///
    /// The `chunk_count` a peer announces has to match what `size` and
    /// `chunk_size` imply; a mismatch means the request is malformed, or the
    /// peer is trying to make the receiver allocate for chunks that will never
    /// arrive.
    ///
    /// The chunk size is capped at [`MAX_CHUNK_SIZE`](super::message::MAX_CHUNK_SIZE):
    /// the receiver holds one chunk in memory while it checks the hash, so an
    /// uncapped size would let a paired peer make it allocate 4 GiB. And the
    /// number of chunks must fit the `u32` index, or it would silently wrap.
    pub fn try_new(size: u64, chunk_size: u32, chunk_count: u32) -> Result<Self, PlanError> {
        if chunk_size == 0 {
            return Err(PlanError::ZeroChunkSize);
        }
        if chunk_size > super::message::MAX_CHUNK_SIZE {
            return Err(PlanError::ChunkTooLarge(chunk_size));
        }
        // Computed in u64, so a count that would wrap a u32 is caught too.
        if size.div_ceil(u64::from(chunk_size)) > u64::from(super::message::MAX_CHUNK_COUNT) {
            return Err(PlanError::TooManyChunks);
        }
        let plan = Self::new(size, chunk_size);
        if plan.chunk_count() != chunk_count {
            return Err(PlanError::CountMismatch {
                declared: chunk_count,
                implied: plan.chunk_count(),
            });
        }
        Ok(plan)
    }

    /// The total size in bytes.
    pub fn size(self) -> u64 {
        self.size
    }

    /// The nominal chunk size; every chunk but the last is exactly this long.
    pub fn chunk_size(self) -> u32 {
        self.chunk_size
    }

    /// How many chunks the file divides into. An empty file has none.
    pub fn chunk_count(self) -> u32 {
        self.size.div_ceil(u64::from(self.chunk_size)) as u32
    }

    /// The byte offset at which a chunk starts.
    pub fn offset_of(self, index: u32) -> u64 {
        u64::from(index) * u64::from(self.chunk_size)
    }

    /// How many bytes a chunk holds. The last one is usually short.
    pub fn len_of(self, index: u32) -> u32 {
        let remaining = self.size.saturating_sub(self.offset_of(index));
        remaining.min(u64::from(self.chunk_size)) as u32
    }
}

/// Why a peer's chunk description was rejected.
#[derive(Debug, thiserror::Error)]
pub enum PlanError {
    #[error("chunk size must be positive")]
    ZeroChunkSize,
    #[error("peer declared {declared} chunks, but the size implies {implied}")]
    CountMismatch { declared: u32, implied: u32 },
    #[error("chunk size {0} is over the limit of {limit}", limit = super::message::MAX_CHUNK_SIZE)]
    ChunkTooLarge(u32),
    #[error(
        "the size implies more than {limit} chunks",
        limit = super::message::MAX_CHUNK_COUNT
    )]
    TooManyChunks,
}

/// SHA-256 of a buffer, as lowercase hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(&Sha256::digest(bytes))
}

/// Reads a stream to the end and returns its SHA-256 and its length.
///
/// The sender hashes the whole file before it sends the request, so that
/// `file_sha256` binds it to exact contents from the outset. On a large file
/// that pass takes a while with nothing else happening, so `progress` is called
/// as it goes and the caller can say so rather than looking frozen.
pub async fn hash_stream<R, F>(
    reader: &mut R,
    total: u64,
    mut progress: F,
) -> std::io::Result<(String, u64)>
where
    R: AsyncRead + Unpin,
    F: FnMut(u64, u64),
{
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 256 * 1024];
    let mut read_so_far = 0u64;

    loop {
        let n = reader.read(&mut buffer).await?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
        read_so_far += n as u64;
        progress(read_so_far, total);
    }

    Ok((hex::encode(&hasher.finalize()), read_so_far))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHUNK: u32 = 4;

    #[test]
    fn an_empty_file_has_no_chunks() {
        let plan = ChunkPlan::new(0, CHUNK);
        assert_eq!(plan.chunk_count(), 0);
    }

    #[test]
    fn a_file_smaller_than_one_chunk_has_one_short_chunk() {
        let plan = ChunkPlan::new(3, CHUNK);
        assert_eq!(plan.chunk_count(), 1);
        assert_eq!(plan.offset_of(0), 0);
        assert_eq!(plan.len_of(0), 3);
    }

    #[test]
    fn an_exact_multiple_has_no_short_chunk() {
        let plan = ChunkPlan::new(8, CHUNK);
        assert_eq!(plan.chunk_count(), 2);
        assert_eq!(plan.len_of(0), CHUNK);
        assert_eq!(plan.len_of(1), CHUNK);
        assert_eq!(plan.offset_of(1), 4);
    }

    #[test]
    fn one_byte_over_a_multiple_adds_a_one_byte_chunk() {
        let plan = ChunkPlan::new(9, CHUNK);
        assert_eq!(plan.chunk_count(), 3);
        assert_eq!(plan.len_of(2), 1);
        assert_eq!(plan.offset_of(2), 8);
    }

    #[test]
    fn the_chunk_lengths_always_sum_to_the_file_size() {
        for size in [0u64, 1, 3, 4, 5, 8, 9, 100, 4095, 4096, 4097] {
            for chunk_size in [1u32, 2, 4, 64, 4096] {
                let plan = ChunkPlan::new(size, chunk_size);
                let total: u64 = (0..plan.chunk_count())
                    .map(|i| u64::from(plan.len_of(i)))
                    .sum();
                assert_eq!(total, size, "size={size} chunk_size={chunk_size}");
            }
        }
    }

    #[test]
    fn a_realistic_plan() {
        let plan = ChunkPlan::new(9_000_000, super::super::message::CHUNK_SIZE);
        assert_eq!(plan.chunk_count(), 3);
        assert_eq!(plan.len_of(0), 4 * 1024 * 1024);
        assert_eq!(plan.len_of(2), 9_000_000 - 2 * 4 * 1024 * 1024);
    }

    #[test]
    fn a_chunk_size_over_the_cap_is_refused() {
        use super::super::message::MAX_CHUNK_SIZE;
        assert!(ChunkPlan::try_new(MAX_CHUNK_SIZE as u64, MAX_CHUNK_SIZE, 1).is_ok());
        assert!(matches!(
            ChunkPlan::try_new(u64::from(MAX_CHUNK_SIZE) + 1, MAX_CHUNK_SIZE + 1, 1),
            Err(PlanError::ChunkTooLarge(_))
        ));
        assert!(matches!(
            ChunkPlan::try_new(u64::from(u32::MAX), u32::MAX, 1),
            Err(PlanError::ChunkTooLarge(_))
        ));
    }

    /// Without the check, `u64::MAX` bytes in 1-byte chunks would wrap the
    /// `u32` chunk count to a small number a peer could simply declare.
    #[test]
    fn a_chunk_count_that_would_wrap_is_refused() {
        let wrapped = (u64::MAX.div_ceil(1)) as u32;
        assert!(matches!(
            ChunkPlan::try_new(u64::MAX, 1, wrapped),
            Err(PlanError::TooManyChunks)
        ));
        let just_over = u64::from(u32::MAX) + 1;
        assert!(matches!(
            ChunkPlan::try_new(just_over, 1, 0),
            Err(PlanError::TooManyChunks)
        ));
    }

    /// A tiny chunk size would otherwise let a modest file describe a
    /// billion chunks, and the receiver keeps state per chunk.
    #[test]
    fn the_chunk_count_is_capped() {
        use super::super::message::MAX_CHUNK_COUNT;
        let at = u64::from(MAX_CHUNK_COUNT);
        assert!(ChunkPlan::try_new(at, 1, MAX_CHUNK_COUNT).is_ok());
        assert!(matches!(
            ChunkPlan::try_new(at + 1, 1, MAX_CHUNK_COUNT + 1),
            Err(PlanError::TooManyChunks)
        ));
        // 1 GiB in one-byte chunks.
        assert!(matches!(
            ChunkPlan::try_new(1 << 30, 1, 1 << 30),
            Err(PlanError::TooManyChunks)
        ));
        // The default chunk size reaches 16 TiB.
        let tib16 = u64::from(MAX_CHUNK_COUNT) * u64::from(super::super::message::CHUNK_SIZE);
        assert!(
            ChunkPlan::try_new(tib16, super::super::message::CHUNK_SIZE, MAX_CHUNK_COUNT).is_ok()
        );
    }

    #[test]
    fn try_new_rejects_a_count_that_does_not_match_the_size() {
        assert!(matches!(
            ChunkPlan::try_new(9, CHUNK, 3),
            Ok(plan) if plan.chunk_count() == 3
        ));
        assert!(matches!(
            ChunkPlan::try_new(9, CHUNK, 99),
            Err(PlanError::CountMismatch {
                declared: 99,
                implied: 3
            })
        ));
        assert!(matches!(
            ChunkPlan::try_new(9, 0, 1),
            Err(PlanError::ZeroChunkSize)
        ));
    }

    #[test]
    fn sha256_matches_the_known_vector_for_the_empty_input() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[tokio::test]
    async fn hashing_a_stream_matches_hashing_a_buffer() {
        let data: Vec<u8> = (0..1_000_000u32).map(|i| (i % 251) as u8).collect();
        let mut reader = data.as_slice();

        let mut seen = Vec::new();
        let (digest, len) = hash_stream(&mut reader, data.len() as u64, |done, total| {
            seen.push((done, total))
        })
        .await
        .expect("hash");

        assert_eq!(digest, sha256_hex(&data));
        assert_eq!(len, data.len() as u64);
        assert!(!seen.is_empty(), "progress was never reported");
        assert_eq!(
            seen.last().copied(),
            Some((data.len() as u64, data.len() as u64))
        );
    }

    #[tokio::test]
    async fn hashing_an_empty_stream_reports_the_empty_digest() {
        let mut reader: &[u8] = &[];
        let (digest, len) = hash_stream(&mut reader, 0, |_, _| {}).await.expect("hash");
        assert_eq!(digest, sha256_hex(b""));
        assert_eq!(len, 0);
    }
}
