mod huffman;

/// Upper bound on the speculative pre-allocation made from an entry's declared
/// uncompressed size. That size comes from the archive, so it is not a fact:
/// reserving it outright panics with "capacity overflow" past `isize::MAX` and
/// aborts on allocation failure below it. The decoders append to the buffer as
/// they go, so the reservation is only a hint and clamping it costs nothing.
pub(crate) const MAX_PREALLOC: usize = 1 << 20;

pub(crate) mod implode;
pub(crate) mod reduce;
pub(crate) mod shrink;
