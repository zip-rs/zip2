use std::collections::VecDeque;
use std::io::{self, copy, Error, Read, Result};

use crate::legacy::bitstream::{lsb, ISTREAM_MIN_BITS};
use crate::legacy::lz77::lz77_output_backref;

use super::bitstream::BitStream;
use super::huffman::HuffmanDecoder;
//const COMPRESSED_BYTES_TO_BUFFER: usize = 4096;

/// Initialize the Huffman decoder d with num_lens codeword lengths read from is.
/// Returns false if the input is invalid.
fn read_huffman_code(
    is: &mut BitStream,
    num_lens: usize,
    d: &mut HuffmanDecoder,
) -> std::io::Result<()> {
    let mut lens = [0; 1 << 8];
    let mut len_count = [0; 17];
    // debug_assert!(num_lens <= sizeof(lens) / sizeof(lens[0]));

    // Number of bytes representing the Huffman code.
    let byte = lsb(is.bits(), 8);
    let num_bytes = (byte + 1) as usize;
    is.advance(8)?;

    let mut codeword_idx = 0;
    for _byte_idx in 0..num_bytes {
        let byte = lsb(is.bits(), 8);
        is.advance(8)?;

        let codeword_len = (byte & 0xf) + 1; /* Low four bits plus one. */
        let run_length = (byte >> 4) + 1; /* High four bits plus one. */

        debug_assert!(codeword_len >= 1 && codeword_len <= 16);
        //debug_assert!(codeword_len < sizeof(len_count) / sizeof(len_count[0]));
        len_count[codeword_len as usize] += run_length;

        if (codeword_idx + run_length) as usize > num_lens {
            return Err(Error::new(
                io::ErrorKind::InvalidData,
                "Too many codeword lengths",
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
            "Not enough codeword lengths",
        ));
    }

    // Check that the Huffman tree is full.
    let mut avail_codewords = 1;
    for i in 1..=16 {
        debug_assert!(avail_codewords >= 0);
        avail_codewords *= 2;
        avail_codewords -= len_count[i] as i32;
        if avail_codewords < 0 {
            return Err(Error::new(
                io::ErrorKind::InvalidData,
                "Huffman tree is not full",
            ));
        }
    }
    if avail_codewords != 0 {
        // Not all codewords were used.
        return Err(Error::new(
            io::ErrorKind::InvalidData,
            "Not all codewords were used",
        ));
    }

    let ok = d.init(&lens, num_lens);
    debug_assert!(ok, "The checks above mean the tree should be valid.");
    Ok(())
}

fn hwexplode(
    src: &[u8],
    src_len: usize,
    uncomp_len: usize,
    large_wnd: bool,
    lit_tree: bool,
    pk101_bug_compat: bool,
    src_used: &mut usize,
    dst: &mut VecDeque<u8>,
) -> std::io::Result<()> {
    let mut is = BitStream::new(src, src_len);
    let mut lit_decoder = HuffmanDecoder::default();
    let mut len_decoder = HuffmanDecoder::default();
    let mut dist_decoder = HuffmanDecoder::default();
    if lit_tree {
        read_huffman_code(&mut is, 256, &mut lit_decoder)?;
    }
    read_huffman_code(&mut is, 64, &mut len_decoder)?;
    read_huffman_code(&mut is, 64, &mut dist_decoder)?;
    let min_len = if pk101_bug_compat {
        if large_wnd {
            3
        } else {
            2
        }
    } else {
        if lit_tree {
            3
        } else {
            2
        }
    };

    while dst.len() < uncomp_len {
        let mut bits = is.bits();
        if lsb(bits, 1) == 0x1 {
            // Literal.
            bits >>= 1;
            let sym;
            let mut used = 0;
            if lit_tree {
                sym = lit_decoder.huffman_decode(!bits as u16, &mut used)?;
                is.advance(1 + used)?;
            } else {
                sym = lsb(bits, 8) as u16;
                is.advance(1 + 8)?;
            }
            debug_assert!(sym <= u8::MAX as u16);
            dst.push_back(sym as u8);
            continue;
        }
        // Backref.
        debug_assert!(lsb(bits, 1) == 0x0);
        let mut used_tot = 1;
        bits >>= 1;

        // Read the low dist bits.
        let mut dist;
        if large_wnd {
            dist = lsb(bits, 7) as usize;
            bits >>= 7;
            used_tot += 7;
        } else {
            dist = lsb(bits, 6) as usize;
            bits >>= 6;
            used_tot += 6;
        }

        // Read the Huffman-encoded high dist bits.
        let mut used = 0;
        let sym = dist_decoder.huffman_decode(!bits as u16, &mut used)?;
        used_tot += used;
        bits >>= used;
        dist |= (sym as usize) << if large_wnd { 7 } else { 6 };
        dist += 1;

        // Read the Huffman-encoded len.
        let sym = len_decoder.huffman_decode(!bits as u16, &mut used)?;
        used_tot += used;
        bits >>= used;
        let mut len = (sym + min_len) as usize;

        if sym == 63 {
            // Read an extra len byte.
            len += lsb(bits, 8) as usize;
            used_tot += 8;
            //  bits >>= 8;
        }

        debug_assert!((used_tot as usize) <= ISTREAM_MIN_BITS);
        is.advance(used_tot)?;
        //  let len = len.min(uncomp_len - dst.len());

        if len <= uncomp_len - dst.len() && dist <= dst.len() {
            // Enough room and no implicit zeros; chunked copy.
            lz77_output_backref(dst, dist, len);
        } else {
            // Copy, handling overlap and implicit zeros.
            for _i in 0..len {
                if dist > dst.len() {
                    dst.push_back(0);
                    continue;
                }
                dst.push_back(dst[dst.len() - dist]);
            }
        }
    }

    *src_used = is.bytes_read();
    Ok(())
}

#[derive(Debug)]
pub struct ImplodeDecoder<R> {
    compressed_reader: R,
    uncompressed_size: u64,
    stream_read: bool,
    large_wnd: bool,
    lit_tree: bool,
    stream: VecDeque<u8>,
}

impl<R: Read> ImplodeDecoder<R> {
    pub fn new(inner: R, uncompressed_size: u64, flags: u16) -> Self {
        let large_wnd = (flags & 2) != 0;
        let lit_tree = (flags & 4) != 0;
        ImplodeDecoder {
            compressed_reader: inner,
            uncompressed_size,
            stream_read: false,
            large_wnd,
            lit_tree,
            stream: VecDeque::new(),
        }
    }

    pub fn finish(mut self) -> Result<VecDeque<u8>> {
        copy(&mut self.compressed_reader, &mut self.stream)?;
        Ok(self.stream)
    }
}

impl<R: Read> Read for ImplodeDecoder<R> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        if !self.stream_read {
            self.stream_read = true;
            let mut compressed_bytes = Vec::new();
            if let Err(err) = self.compressed_reader.read_to_end(&mut compressed_bytes) {
                return Err(err.into());
            }
            let mut src_used = 0;
            hwexplode(
                &compressed_bytes,
                compressed_bytes.len(),
                self.uncompressed_size as usize,
                self.large_wnd,
                self.lit_tree,
                false,
                &mut src_used,
                &mut self.stream,
            )?;
        }
        let bytes_read = self.stream.len().min(buf.len());
        buf[..bytes_read].copy_from_slice(&self.stream.drain(..bytes_read).collect::<Vec<u8>>());
        Ok(bytes_read)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::hwexplode;

    const HAMLET_256: &[u8; 249] = include_bytes!("../../tests/implode_hamlet_256.bin");

    #[test]
    fn test_explode_hamlet_256() {
        let mut src_used = HAMLET_256.len();
        let mut dst = VecDeque::new();
        hwexplode(
            HAMLET_256,
            HAMLET_256.len(),
            256,
            false,
            false,
            false,
            &mut src_used,
            &mut dst,
        )
        .unwrap();
        assert_eq!(dst.len(), 256);
    }
}
