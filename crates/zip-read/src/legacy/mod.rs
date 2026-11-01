mod huffman;

/// Upper bound on the speculative pre-allocation made from an entry's declared
/// uncompressed size. That size comes from the archive, so it is not a fact:
/// reserving it outright panics with "capacity overflow" past `isize::MAX` and
/// aborts on allocation failure below it. Entries larger than this still decode
/// in full; the buffer just grows geometrically past the cap instead of in one step.
pub const MAX_PREALLOC: usize = 1 << 20;

pub mod implode;
pub mod reduce;
pub mod shrink;
