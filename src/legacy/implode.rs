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
    let mut lens = [0; 256];
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

    d.init(&lens, num_lens)
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

    const HAMLET_256: [u8; 249] = [
        0x0d, 0x02, 0x01, 0x12, 0x23, 0x14, 0x15, 0x36, 0x37, 0x68, 0x89, 0x9a, 0xdb, 0x3c, 0x05,
        0x06, 0x12, 0x13, 0x44, 0xc5, 0xf6, 0x96, 0xf7, 0xdf, 0xef, 0xfe, 0xdd, 0x50, 0x21, 0x54,
        0xb9, 0x6f, 0xd5, 0x96, 0x1d, 0x4b, 0x17, 0xe4, 0xd1, 0xba, 0x74, 0xcb, 0xba, 0x15, 0x5b,
        0x56, 0xee, 0x59, 0x90, 0x45, 0x85, 0xbe, 0x7d, 0xbb, 0x16, 0xe4, 0x5b, 0xb3, 0x20, 0x91,
        0x86, 0x6d, 0xcb, 0xb6, 0x2c, 0x5d, 0x96, 0x20, 0xc5, 0xe6, 0x05, 0x79, 0x35, 0x2d, 0x5b,
        0xb6, 0x69, 0x9c, 0x37, 0xc8, 0xa9, 0x68, 0xc3, 0xae, 0x2d, 0x3b, 0x17, 0x6e, 0xd9, 0xb0,
        0x72, 0xcb, 0xe8, 0xaf, 0xe0, 0x4d, 0x15, 0x6d, 0xda, 0xb9, 0x20, 0xcb, 0xbc, 0x37, 0xe4,
        0x37, 0xfb, 0x56, 0x2e, 0x48, 0xba, 0x68, 0xcb, 0x82, 0xac, 0x3b, 0xb7, 0x8c, 0xff, 0x0c,
        0xeb, 0x36, 0xef, 0x5b, 0xb7, 0x65, 0x8c, 0xe7, 0x1d, 0xea, 0xf5, 0xbe, 0xc2, 0xb7, 0x9b,
        0xee, 0x5e, 0xd5, 0x6d, 0x9a, 0x74, 0x4d, 0x26, 0x59, 0xd3, 0x0d, 0x63, 0xbc, 0xe7, 0x74,
        0x3f, 0x19, 0x63, 0xdd, 0xf6, 0xed, 0x1c, 0xa0, 0xfb, 0x0d, 0xf7, 0xfd, 0x6f, 0x38, 0xd9,
        0x9a, 0xee, 0x9c, 0xfe, 0xa1, 0x3e, 0xef, 0x40, 0x6b, 0x36, 0xe9, 0xeb, 0x7c, 0x83, 0x74,
        0xfb, 0x16, 0xe4, 0x98, 0xf1, 0xd1, 0x7e, 0xd4, 0xcb, 0x7f, 0xa3, 0x41, 0xde, 0x6c, 0xe6,
        0xdb, 0xf5, 0xe2, 0x5f, 0xd9, 0x0a, 0x79, 0xcb, 0x4d, 0x13, 0x54, 0xa7, 0x61, 0x57, 0xf8,
        0x2b, 0x5d, 0xb5, 0xef, 0xb9, 0x6f, 0xcb, 0xda, 0x49, 0xd6, 0x2e, 0x41, 0x82, 0xcc, 0xfa,
        0xb6, 0x2e, 0xc8, 0xb6, 0x61, 0xf3, 0xe8, 0x3f, 0x1c, 0xe2, 0x9d, 0x06, 0xa9, 0x9f, 0x4d,
        0x6b, 0xc7, 0xe8, 0x19, 0xfb, 0x9d, 0xea, 0x63, 0xbb,
    ];

    #[test]
    fn test_explode_hamlet_256() {
        let mut src_used = HAMLET_256.len();
        let mut dst = VecDeque::new();
        hwexplode(
            &HAMLET_256,
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
