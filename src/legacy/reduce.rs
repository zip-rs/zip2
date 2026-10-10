use std::io::{self, Read, Result};

use bitstream_io::{BitRead, BitReader, Endianness, LittleEndian};

/// Number of bits used to represent indices in a follower set of size n.
const fn follower_idx_bitlen(n: u8) -> u8 {
    debug_assert!(n <= 32);
    match n {
        0 => 0,
        1 => 1,
        _ => 8 - (n - 1).leading_zeros() as u8,
    }
}

#[derive(Default, Clone, Copy)]
struct FollowerSet {
    followers: [u8; 32],
    size: u8,
    idx_bitlen: u8,
}

/// Read the follower sets from is into fsets. Returns true on success.
type FollowerSetArray = [FollowerSet; u8::MAX as usize + 1];

fn read_follower_sets<T: Read, E: Endianness>(
    is: &mut BitReader<T, E>,
) -> io::Result<FollowerSetArray> {
    let mut fsets = [FollowerSet::default(); u8::MAX as usize + 1];
    for i in (0..=u8::MAX as usize).rev() {
        let n = is.read::<6, u8>()?;
        if n > 32 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid follower set",
            ));
        }
        fsets[i].size = n;
        fsets[i].idx_bitlen = follower_idx_bitlen(n);

        for j in 0..fsets[i].size as usize {
            fsets[i].followers[j] = is.read::<8, u8>()?;
        }
    }

    Ok(fsets)
}

/// Read the next byte from is, decoded based on prev_byte and the follower sets.
/// The byte is returned in *out_byte.
///
/// # Returns
///
/// * `Ok` with the byte if it was successfully read.
/// * `Err(io::Error)` on bad data or end of input.
fn read_next_byte<T: Read, E: Endianness>(
    is: &mut BitReader<T, E>,
    prev_byte: u8,
    fsets: &mut FollowerSetArray,
) -> io::Result<u8> {
    if fsets[prev_byte as usize].size == 0 // No followers
            || is.read::<1, u8>()? == 1
    // Indicates next symbol is a literal byte
    {
        return is.read::<8, u8>();
    }

    // The bits represent the index of a follower byte.
    let idx_bitlen = fsets[prev_byte as usize].idx_bitlen;
    let follower_idx = is.read_var::<u16>(u32::from(idx_bitlen))? as usize;
    if follower_idx >= fsets[prev_byte as usize].size as usize {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid follower index",
        ));
    }
    Ok(fsets[prev_byte as usize].followers[follower_idx])
}

fn max_len(comp_factor: u8) -> usize {
    debug_assert!((1..=4).contains(&comp_factor));
    let v_len_bits = (8 - comp_factor) as usize;
    // Bits in V + extra len byte + implicit 3.
    ((1 << v_len_bits) - 1) + u8::MAX as usize + 3
}

fn max_dist(comp_factor: u8) -> usize {
    debug_assert!((1..=4).contains(&comp_factor));
    let v_dist_bits = comp_factor as usize;
    // Bits in V * 256 + W byte + implicit 1. */
    1 << (v_dist_bits + 8)
}

const DLE_BYTE: u8 = 0x90;

/// Get the n least significant bits of x.
fn lsb(x: u8, n: u8) -> u8 {
    if n >= 8 {
        return x;
    }
    x & ((1 << n) - 1)
}

struct ReduceState<T> {
    bit_reader: BitReader<T, LittleEndian>,
    fsets: Box<FollowerSetArray>,
    curr_byte: u8,
    uncomp_len: usize,
    comp_factor: u8,
}

impl<T> std::fmt::Debug for ReduceState<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReduceState")
            .field("curr_byte", &self.curr_byte)
            .field("uncomp_len", &self.uncomp_len)
            .field("comp_factor", &self.comp_factor)
            .finish_non_exhaustive()
    }
}

impl<T: Read> ReduceState<T> {
    fn new(src: T, uncomp_len: usize, comp_factor: u8) -> io::Result<Self> {
        let mut is = BitReader::endian(src, LittleEndian);
        let fsets = Box::new(read_follower_sets(&mut is)?);
        Ok(Self {
            bit_reader: is,
            fsets,
            curr_byte: 0, // The first "previous byte" is implicitly zero.
            uncomp_len,
            comp_factor,
        })
    }

    fn expand_until(&mut self, target_len: usize, dst: &mut Vec<u8>) -> io::Result<()> {
        debug_assert!((1..=4).contains(&self.comp_factor));
        let target_len = target_len.min(self.uncomp_len);
        if dst.len() < target_len {
            dst.try_reserve(
                target_len
                    .saturating_sub(dst.len())
                    .min(crate::legacy::MAX_PREALLOC),
            )
            .map_err(|e| io::Error::new(io::ErrorKind::OutOfMemory, e))?;
        }

        // Number of bits in V used for backref length.
        let v_len_bits = 8 - self.comp_factor;

        while dst.len() < target_len {
            // Read a literal byte or DLE marker.
            self.curr_byte = read_next_byte(&mut self.bit_reader, self.curr_byte, &mut self.fsets)?;
            if self.curr_byte != DLE_BYTE {
                // Output a literal byte.
                dst.push(self.curr_byte);
                continue;
            }

            // Read the V byte which determines the length.
            self.curr_byte = read_next_byte(&mut self.bit_reader, self.curr_byte, &mut self.fsets)?;
            if self.curr_byte == 0 {
                // Output a literal DLE byte.
                dst.push(DLE_BYTE);
                continue;
            }
            let v = self.curr_byte;
            let mut len = lsb(v, v_len_bits) as usize;
            if len == (1 << v_len_bits) - 1 {
                // Read an extra length byte.
                self.curr_byte =
                    read_next_byte(&mut self.bit_reader, self.curr_byte, &mut self.fsets)?;
                len += self.curr_byte as usize;
            }
            len += 3;

            // Read the W byte, which together with V gives the distance.
            self.curr_byte = read_next_byte(&mut self.bit_reader, self.curr_byte, &mut self.fsets)?;
            let dist = (((v as usize) >> v_len_bits) << 8) + self.curr_byte as usize + 1;

            debug_assert!(len <= max_len(self.comp_factor));
            debug_assert!(dist <= max_dist(self.comp_factor));

            // Output the back reference.
            if dist <= dst.len() {
                // Optimize for non-overlapping copies
                if dist >= len {
                    // No overlap, can use extend_from_within
                    let start = dst.len() - dist;
                    dst.extend_from_within(start..start + len.min(self.uncomp_len - dst.len()));
                } else {
                    // Overlapping copy
                    let copy_len = len.min(self.uncomp_len - dst.len());
                    for _ in 0..copy_len {
                        let byte = dst[dst.len() - dist];
                        dst.push(byte);
                    }
                }
            } else {
                // Copy with implicit zeros
                let copy_len = len.min(self.uncomp_len - dst.len());
                for _ in 0..copy_len {
                    if dist > dst.len() {
                        dst.push(0);
                    } else {
                        let byte = dst[dst.len() - dist];
                        dst.push(byte);
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
fn hwexpand(src: &[u8], uncomp_len: usize, comp_factor: u8, dst: &mut Vec<u8>) -> io::Result<()> {
    let mut state = ReduceState::new(src, uncomp_len, comp_factor)?;
    state.expand_until(uncomp_len, dst)
}

#[derive(Debug)]
pub struct ReduceDecoder<R> {
    compressed_reader: R,
    uncompressed_size: u64,
    state: Option<ReduceState<io::Cursor<Vec<u8>>>>,
    comp_factor: u8,
    stream: Vec<u8>,
    read_pos: usize, // Add read position tracker
}

impl<R: Read> ReduceDecoder<R> {
    pub fn new(inner: R, uncompressed_size: u64, comp_factor: u8) -> Self {
        ReduceDecoder {
            compressed_reader: inner,
            uncompressed_size,
            state: None,
            comp_factor,
            stream: Vec::new(),
            read_pos: 0,
        }
    }

    pub fn into_inner(self) -> R {
        self.compressed_reader
    }

    pub fn inner(&self) -> &R {
        &self.compressed_reader
    }
}

impl<R: Read> Read for ReduceDecoder<R> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let state = match &mut self.state {
            Some(state) => state,
            None => {
                let uncomp_len = usize::try_from(self.uncompressed_size).map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "uncompressed size exceeds usize::MAX",
                    )
                })?;
                let mut compressed_bytes = Vec::new();
                self.compressed_reader.read_to_end(&mut compressed_bytes)?;
                self.state.insert(ReduceState::new(
                    io::Cursor::new(compressed_bytes),
                    uncomp_len,
                    self.comp_factor,
                )?)
            }
        };

        let target_len = self
            .read_pos
            .saturating_add(buf.len())
            .min(state.uncomp_len);
        if self.stream.len() < target_len {
            state.expand_until(target_len, &mut self.stream)?;
        }

        let available = self.stream.len() - self.read_pos;
        let bytes_to_read = available.min(buf.len());
        buf[..bytes_to_read]
            .copy_from_slice(&self.stream[self.read_pos..self.read_pos + bytes_to_read]);
        self.read_pos += bytes_to_read;
        Ok(bytes_to_read)
    }
}

#[cfg(test)]
mod tests {
    use super::hwexpand;
    use crate::legacy::reduce::{follower_idx_bitlen, lsb, max_dist};
    const HAMLET_2048: &[u8; 1285] =
        include_bytes!("../../tests/data/legacy/reduce_hamlet_2048.bin");
    const HAMLET_2048_OUT: &[u8; 2048] =
        include_bytes!("../../tests/data/legacy/implode_hamlet_2048.out");

    #[test]
    fn test_lsb() {
        assert_eq!(lsb(0xFF, 8), 0xFF);
        for i in 0..7 {
            assert_eq!(lsb(0xFF, i), (1 << i) - 1);
        }
    }

    #[test]
    fn test_expand_hamlet2048() {
        let mut dst = Vec::new();
        hwexpand(HAMLET_2048, 2048, 4, &mut dst).unwrap();
        assert_eq!(dst.len(), 2048);
        assert_eq!(&dst, &HAMLET_2048_OUT);
    }

    /*
      Put some text first to make PKZIP actually use Reduce compression.
      Target the code path which copies a zero when dist > current position.

      $ curl -O http://cd.textfiles.com/originalsw/25/pkz092.exe
      $ dosbox -c "mount c ." -c "c:" -c "pkz092" -c "exit"
      $ dd if=hamlet.txt bs=1 count=2048 > a
      $ dd if=/dev/zero  bs=1 count=1024 >> a
      $ dosbox -c "mount c ." -c "c:" -c "pkzip -ea4 a.zip a" -c "exit"
      $ xxd -i -s 31 -l $(expr $(find A.ZIP -printf %s) - 100) A.ZIP
    */
    const ZEROS_REDUCED: &[u8; 1297] =
        include_bytes!("../../tests/data/legacy/reduce_zero_reduced.bin");

    #[test]
    fn test_expand_zeros() {
        let mut dst = Vec::new();
        hwexpand(ZEROS_REDUCED, 2048 + 1024, 4, &mut dst).unwrap();
        assert_eq!(dst.len(), 2048 + 1024);
        for i in 0..(1 << 10) {
            assert_eq!(dst[(1 << 11) + i], 0);
        }
    }

    fn orig_follower_idx_bitlen(n: u8) -> u8 {
        if n > 16 {
            return 5;
        }
        if n > 8 {
            return 4;
        }
        if n > 4 {
            return 3;
        }
        if n > 2 {
            return 2;
        }
        if n > 0 {
            return 1;
        }
        0
    }

    #[test]
    fn test_follower_idx_biten() {
        for i in 0..=32 {
            assert_eq!(orig_follower_idx_bitlen(i), follower_idx_bitlen(i));
        }
    }

    #[test]
    fn test_max_dist() {
        for i in 1..=4 {
            let v_dist_bits = i as usize;
            let c = 1 << (v_dist_bits + 8);
            assert_eq!(max_dist(i), c);
        }
    }

    #[test]
    fn test_reduce_decoder_incremental_take() {
        use super::ReduceDecoder;
        use std::io::Read;

        let mut decoder = ReduceDecoder::new(HAMLET_2048.as_slice(), 2048, 4);
        let mut prefix = Vec::new();
        (&mut decoder).take(16).read_to_end(&mut prefix).unwrap();
        assert_eq!(prefix, &HAMLET_2048_OUT[..16]);
        // Only the bytes needed to satisfy the 16-byte read should be buffered so far:
        assert!(decoder.stream.len() < 512);

        let mut rest = Vec::new();
        decoder.read_to_end(&mut rest).unwrap();
        prefix.extend_from_slice(&rest);
        assert_eq!(prefix.as_slice(), HAMLET_2048_OUT);
    }
}
