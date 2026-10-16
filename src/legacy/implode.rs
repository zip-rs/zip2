use super::huffman::HuffmanDecoder;
use bitstream_io::{BitRead, BitReader, Endianness, LittleEndian};
use std::io::{self, Cursor, Error, Read, Result};

/// Maximum symbol value in the length Huffman table (6 bits)
/// When this value is decoded, an additional byte is read for extended length
const MAX_LEN_SYMBOL: u16 = 63;

/// Maximum number of symbols in a Huffman table (256 for literals, 64 for lengths/distances)
const MAX_HUFFMAN_SYMBOLS: usize = 1 << 8; // 256

/// Maximum code length in bits for Huffman codes (per ZIP specification)
const MAX_CODE_LENGTH: usize = 16;

/// Size of the code length count array (lengths are actually 1..=16)
const CODE_LENGTH_COUNT_SIZE: usize = MAX_CODE_LENGTH;

/// Initialize the Huffman decoder d with num_lens codeword lengths read from is.
/// Returns false if the input is invalid.
fn read_huffman_code<T: std::io::Read, E: Endianness>(
    is: &mut BitReader<T, E>,
    num_lens: usize,
) -> std::io::Result<Box<HuffmanDecoder>> {
    let mut lens = [0; MAX_HUFFMAN_SYMBOLS];
    let mut len_count = [0; CODE_LENGTH_COUNT_SIZE];

    // Number of bytes representing the Huffman code.
    let byte = is.read::<8, u8>()?;
    // Widen before adding: a header byte of 0xff means 256 code-length bytes.
    let num_bytes = usize::from(byte) + 1;

    let mut codeword_idx = 0;
    for _byte_idx in 0..num_bytes {
        let byte = is.read::<8, u16>()?;

        let codeword_len = (byte & 0xf) + 1; /* Low four bits plus one. */
        let run_length = (byte >> 4) + 1; /* High four bits plus one. */

        debug_assert!((1..=16).contains(&codeword_len));
        len_count[codeword_len as usize - 1] += run_length;

        if (codeword_idx + run_length) as usize > num_lens {
            return Err(Error::new(
                io::ErrorKind::InvalidData,
                "too many codeword lengths",
            ));
        }
        for _ in 0..run_length {
            debug_assert!((codeword_idx as usize) < num_lens);
            lens[codeword_idx as usize] = codeword_len as u8;
            codeword_idx += 1;
        }
    }

    debug_assert!(codeword_idx as usize <= num_lens);
    if (codeword_idx as usize) < num_lens {
        return Err(Error::new(
            io::ErrorKind::InvalidData,
            "not enough codeword lengths",
        ));
    }

    // Check that the Huffman tree is full.
    let mut avail_codewords = 1;
    for i in 1..=16 {
        debug_assert!(avail_codewords >= 0);
        avail_codewords *= 2;
        avail_codewords -= i32::from(len_count[i - 1]);
        if avail_codewords < 0 {
            return Err(Error::new(
                io::ErrorKind::InvalidData,
                "huffman tree is not full",
            ));
        }
    }
    if avail_codewords != 0 {
        // Not all codewords were used.
        return Err(Error::new(
            io::ErrorKind::InvalidData,
            "not all codewords were used",
        ));
    }

    let mut d = Box::new(HuffmanDecoder::default());
    d.init(&lens, num_lens)?;
    Ok(d)
}

struct ImplodeState<T> {
    bit_length: u64,
    bit_reader: BitReader<T, LittleEndian>,
    lit_decoder_opt: Option<Box<HuffmanDecoder>>,
    len_decoder: Box<HuffmanDecoder>,
    dist_decoder: Box<HuffmanDecoder>,
    min_len: u16,
    dist_low_bits: u32,
    uncomp_len: usize,
}

impl<T> std::fmt::Debug for ImplodeState<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImplodeState")
            .field("bit_length", &self.bit_length)
            .field("uncomp_len", &self.uncomp_len)
            .finish_non_exhaustive()
    }
}

impl<T: std::io::Read + std::io::Seek> ImplodeState<T> {
    fn new(
        src: T,
        bit_length: u64,
        uncomp_len: usize,
        large_wnd: bool,
        lit_tree: bool,
        pk101_bug_compat: bool,
    ) -> std::io::Result<Self> {
        let mut bit_reader = BitReader::endian(src, LittleEndian);
        let lit_decoder_opt = if lit_tree {
            Some(read_huffman_code(&mut bit_reader, 256)?)
        } else {
            None
        };
        let len_decoder = read_huffman_code(&mut bit_reader, 64)?;
        let dist_decoder = read_huffman_code(&mut bit_reader, 64)?;
        let min_len = if (pk101_bug_compat && large_wnd) || (!pk101_bug_compat && lit_tree) {
            3
        } else {
            2
        };
        let dist_low_bits = if large_wnd { 7 } else { 6 };

        Ok(Self {
            bit_length,
            bit_reader,
            lit_decoder_opt,
            len_decoder,
            dist_decoder,
            min_len,
            dist_low_bits,
            uncomp_len,
        })
    }

    fn explode_until(&mut self, target_len: usize, dst: &mut Vec<u8>) -> std::io::Result<()> {
        let target_len = target_len.min(self.uncomp_len);
        if dst.len() < target_len {
            dst.try_reserve(
                target_len
                    .saturating_sub(dst.len())
                    .min(crate::legacy::MAX_PREALLOC),
            )
            .map_err(|e| Error::new(io::ErrorKind::OutOfMemory, e))?;
        }

        while dst.len() < target_len {
            let is_literal = self.bit_reader.read_bit()?;
            if is_literal {
                // Literal.
                let sym = if let Some(lit_decoder) = &mut self.lit_decoder_opt {
                    lit_decoder.huffman_decode(self.bit_length, &mut self.bit_reader)?
                } else {
                    u16::from(self.bit_reader.read::<8, u8>()?)
                };
                debug_assert!(sym <= u16::from(u8::MAX));
                dst.push(sym as u8);
                continue;
            }

            // Read the low dist bits.
            let mut dist = self.bit_reader.read_var::<u16>(self.dist_low_bits)?;
            // Read the Huffman-encoded high dist bits.
            let sym = self
                .dist_decoder
                .huffman_decode(self.bit_length, &mut self.bit_reader)?;
            dist |= sym << self.dist_low_bits;
            dist += 1;

            // Read the Huffman-encoded len.
            let sym = self
                .len_decoder
                .huffman_decode(self.bit_length, &mut self.bit_reader)?;
            let mut len = (sym + self.min_len) as usize;

            if sym == MAX_LEN_SYMBOL {
                // Read an extra len byte.
                len += self.bit_reader.read::<8, u16>()? as usize;
            }
            let len = len.min(self.uncomp_len - dst.len());

            // Optimize the copy loop using extend_from_within when possible
            if dist as usize <= dst.len() {
                if dist as usize >= len {
                    // No overlap in copy, can use extend_from_within
                    let start = dst.len() - dist as usize;
                    dst.extend_from_within(start..start + len);
                } else {
                    // Overlapping copy
                    for _ in 0..len {
                        let byte = dst[dst.len() - dist as usize];
                        dst.push(byte);
                    }
                }
            } else {
                // Copy with implicit zeros
                for _ in 0..len {
                    if dist as usize > dst.len() {
                        dst.push(0);
                    } else {
                        let byte = dst[dst.len() - dist as usize];
                        dst.push(byte);
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
fn hwexplode(
    src: &[u8],
    uncomp_len: usize,
    large_wnd: bool,
    lit_tree: bool,
    pk101_bug_compat: bool,
    dst: &mut Vec<u8>,
) -> std::io::Result<()> {
    let mut state = ImplodeState::new(
        Cursor::new(&src),
        src.len() as u64 * 8,
        uncomp_len,
        large_wnd,
        lit_tree,
        pk101_bug_compat,
    )?;
    state.explode_until(uncomp_len, dst)
}

#[derive(Debug)]
pub struct ImplodeDecoder<R> {
    compressed_reader: R,
    uncompressed_size: u64,
    state: Option<ImplodeState<Cursor<Vec<u8>>>>,
    large_wnd: bool,
    lit_tree: bool,
    stream: Vec<u8>,
    read_pos: usize, // Add read position tracker
}

impl<R: Read> ImplodeDecoder<R> {
    pub fn new(inner: R, uncompressed_size: u64, flags: u16) -> Self {
        let large_wnd = (flags & 2) != 0;
        let lit_tree = (flags & 4) != 0;
        ImplodeDecoder {
            compressed_reader: inner,
            uncompressed_size,
            state: None,
            large_wnd,
            lit_tree,
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

impl<R: Read> Read for ImplodeDecoder<R> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let state = match &mut self.state {
            Some(state) => state,
            None => {
                let uncomp_len = usize::try_from(self.uncompressed_size).map_err(|_| {
                    Error::new(
                        io::ErrorKind::InvalidData,
                        "uncompressed size exceeds usize::MAX",
                    )
                })?;
                let mut compressed_bytes = Vec::new();
                self.compressed_reader.read_to_end(&mut compressed_bytes)?;

                let bit_length = compressed_bytes.len() as u64 * 8;
                self.state.insert(ImplodeState::new(
                    Cursor::new(compressed_bytes),
                    bit_length,
                    uncomp_len,
                    self.large_wnd,
                    self.lit_tree,
                    false,
                )?)
            }
        };

        let target_len = self
            .read_pos
            .saturating_add(buf.len())
            .min(state.uncomp_len);
        if self.stream.len() < target_len {
            state.explode_until(target_len, &mut self.stream)?;
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
    use super::hwexplode;

    const HAMLET_256: &[u8; 249] = include_bytes!("../../tests/data/legacy/implode_hamlet_256.bin");
    const HAMLET_256_OUT: &[u8; 256] =
        include_bytes!("../../tests/data/legacy/implode_hamlet_256.out");

    #[test]
    fn test_explode_hamlet_256() {
        let mut dst = Vec::new();
        hwexplode(HAMLET_256, 256, false, false, false, &mut dst).unwrap();
        assert_eq!(dst.len(), 256);
        assert_eq!(&dst, &HAMLET_256_OUT);
    }

    /// A Huffman table header byte of 0xff means 256 code-length bytes, which is valid. The count
    /// was computed as `byte + 1` in u8, which overflows (a panic in debug builds).
    #[test]
    fn code_length_table_with_256_bytes() {
        use bitstream_io::{BitWrite, BitWriter, LittleEndian};

        // 256 one-byte runs of length 8 (0x07: run 1, length 7 + 1) describe 256 codewords of
        // 8 bits each: a full tree for the literal table.
        let mut src = Vec::new();
        {
            let mut w = BitWriter::endian(&mut src, LittleEndian);
            w.write::<8, u8>(0xff).unwrap();
            for _ in 0..256 {
                w.write::<8, u8>(0x07).unwrap();
            }
            w.byte_align().unwrap();
        }
        let mut is = super::BitReader::endian(super::Cursor::new(&src), super::LittleEndian);
        super::read_huffman_code(&mut is, 256).expect("a 256-entry code is valid");
    }

    /// Truncating a valid stream anywhere must give an error, never a panic: the Huffman decoder
    /// computed the bits left as `length - position`, which underflowed once a symbol had moved
    /// the reader past the end of the data.
    #[test]
    fn truncated_stream_is_an_error() {
        for cut in 0..HAMLET_256.len() {
            let r = std::panic::catch_unwind(|| {
                let mut dst = Vec::new();
                let r = hwexplode(&HAMLET_256[..cut], 256, false, false, false, &mut dst);
                (r.is_ok(), dst)
            });
            assert!(
                r.is_ok(),
                "hwexplode panicked on {cut} of {} bytes",
                HAMLET_256.len()
            );
            let Ok((ok, dst)) = r else { continue };
            // A prefix can only decode to a prefix of the output.
            assert!(
                !ok || HAMLET_256_OUT.starts_with(&dst),
                "cut {cut}: wrong output"
            );
        }
    }

    #[test]
    fn test_implode_decoder_incremental_take() {
        use super::ImplodeDecoder;
        use std::io::Read;

        let decoder = ImplodeDecoder::new(HAMLET_256.as_slice(), 256, 0);
        let mut limited = decoder.take(16);
        let mut out = Vec::new();
        limited.read_to_end(&mut out).unwrap();
        assert_eq!(out, &HAMLET_256_OUT[..16]);
        assert!(limited.get_ref().stream.len() <= 32);
    }
}
