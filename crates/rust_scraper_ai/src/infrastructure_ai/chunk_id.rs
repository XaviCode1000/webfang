/// Unique identifier for a content chunk
///
/// Newtype pattern (`type-newtype-ids` rust-skill) for type safety.
/// Prevents mixing chunk IDs with other u64 values at compile time.
///
/// # Examples
///
/// ```
/// # #[cfg(feature = "ai")]
/// # fn example() {
/// use rust_scraper::infrastructure::ai::ChunkId;
///
/// let id = ChunkId(42);
/// assert_eq!(format!("{}", id), "chunk-42");
/// # }
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChunkId(pub u64);

impl ChunkId {
    /// Create a new ChunkId
    #[must_use]
    pub fn new(id: u64) -> Self {
        Self(id)
    }

    /// Get the underlying u64 value
    #[must_use]
    pub fn inner(&self) -> u64 {
        self.0
    }
}

impl std::fmt::Display for ChunkId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "chunk-{}", self.0)
    }
}

impl From<u64> for ChunkId {
    fn from(id: u64) -> Self {
        Self::new(id)
    }
}

impl From<ChunkId> for u64 {
    fn from(id: ChunkId) -> Self {
        id.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunk_id_creation() {
        let id = ChunkId::new(42);
        assert_eq!(id.inner(), 42);
    }

    #[test]
    fn test_chunk_id_display() {
        let id = ChunkId(42);
        assert_eq!(format!("{}", id), "chunk-42");
    }

    #[test]
    fn test_chunk_id_from_u64() {
        let id: ChunkId = 123u64.into();
        assert_eq!(id.inner(), 123);
    }

    #[test]
    fn test_chunk_id_into_u64() {
        let id = ChunkId(456);
        let value: u64 = id.into();
        assert_eq!(value, 456);
    }

    #[test]
    fn test_chunk_id_equality() {
        let id1 = ChunkId(42);
        let id2 = ChunkId(42);
        let id3 = ChunkId(43);

        assert_eq!(id1, id2);
        assert_ne!(id1, id3);
    }

    #[test]
    fn test_chunk_id_hash() {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let id1 = ChunkId(42);
        let id2 = ChunkId(42);

        let mut hasher1 = DefaultHasher::new();
        id1.hash(&mut hasher1);

        let mut hasher2 = DefaultHasher::new();
        id2.hash(&mut hasher2);

        assert_eq!(hasher1.finish(), hasher2.finish());
    }
}
