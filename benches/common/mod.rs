// Deterministic seeded randomness helper (SplitMix64; no external dependencies, no syscalls).
// Why we use this instead of the getrandom crate:
// 1. Deterministic — always the same bytes.
// 2. No syscalls — the getrandom crate calls the OS/platform RNG.
// 3. No dependency here — getrandom is used elsewhere but remains optional and may change.
pub fn seeded_random_bytes(size: usize) -> Box<[u8]> {
    let mut x: u64 = 0xdead_beef_cafe_babe;
    let mut out = vec![0u8; size].into_boxed_slice();

    for chunk in out.chunks_mut(8) {
        x = x.wrapping_add(0x9E3779B97F4A7C15);

        let mut z = x;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^= z >> 31;

        let bytes = z.to_le_bytes();
        for (i, b) in chunk.iter_mut().enumerate() {
            *b = bytes[i];
        }
    }

    out
}
