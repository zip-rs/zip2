use std::io::{self, Error};

use crate::legacy::bitstream::reverse_lsb;

use super::bitstream::lsb;

#[derive(Default, Clone, Copy)]
pub struct TableEntry {
    /// Wide enough to fit the max symbol nbr.
    pub sym: u16,
    /// 0 means no symbol.
    pub len: u8,
}

/// Deflate uses max 288 symbols.
const MAX_HUFFMAN_SYMBOLS: usize = 288;
/// Implode uses max 16-bit codewords.
const MAX_HUFFMAN_BITS: usize = 16;
/// Seems a good trade-off.
const HUFFMAN_LOOKUP_TABLE_BITS: u8 = 8;

pub struct HuffmanDecoder {
    /// Lookup table for fast decoding of short codewords.
    pub table: [TableEntry; 1 << HUFFMAN_LOOKUP_TABLE_BITS],
    /// "Sentinel bits" value for each codeword length.
    pub sentinel_bits: [u32; MAX_HUFFMAN_BITS],
    /// First symbol index minus first codeword mod 2**16 for each length.
    pub offset_first_sym_idx: [u16; MAX_HUFFMAN_BITS],
    /// Map from symbol index to symbol.
    pub syms: [u16; MAX_HUFFMAN_SYMBOLS],
    // num_syms:usize
}

impl Default for HuffmanDecoder {
    fn default() -> Self {
        let syms = [0; MAX_HUFFMAN_SYMBOLS];
        let table = [TableEntry::default(); 1 << HUFFMAN_LOOKUP_TABLE_BITS];
        Self {
            table,
            sentinel_bits: Default::default(),
            offset_first_sym_idx: Default::default(),
            syms,
        }
    }
}

/// Initialize huffman decoder d for a code defined by the n codeword lengths.
/// Returns false if the codeword lengths do not correspond to a valid prefix
/// code.
impl HuffmanDecoder {
    pub fn init(&mut self, lengths: &[u8], n: usize) -> std::io::Result<()> {
        let mut count = [0; MAX_HUFFMAN_BITS];
        let mut code = [0; MAX_HUFFMAN_BITS];
        let mut sym_idx: [u16; 16] = [0; MAX_HUFFMAN_BITS];
        // Zero-initialize the lookup table.
        for t in &mut self.table {
            t.len = 0;
        }

        // Count the number of codewords of each length.
        for sym in 0..n {
            let len = lengths[sym] as usize;
            // Ignore zero-length codewords.
            if len == 0 {
                continue;
            }
            debug_assert!(len < MAX_HUFFMAN_BITS);
            count[len] += 1;
        }

        for len in 1..MAX_HUFFMAN_BITS {
            // First canonical codeword of this length.
            code[len] = (code[len - 1] + count[len - 1]) << 1;

            if count[len] != 0 && code[len] as u32 + count[len] as u32 - 1 > (1u32 << len) - 1 {
                return Err(Error::new(
                    io::ErrorKind::InvalidData,
                    "The last codeword is longer than len bits",
                ));
            }

            let s = ((code[len] as u32 + count[len] as u32) << (MAX_HUFFMAN_BITS - len)) as u32;
            self.sentinel_bits[len] = s;
            debug_assert!(self.sentinel_bits[len] >= code[len] as u32, "No overflow!");
            sym_idx[len] = sym_idx[len - 1] + count[len - 1];
            self.offset_first_sym_idx[len] = sym_idx[len].wrapping_sub(code[len]);
        }

        // Build mapping from index to symbol and populate the lookup table.
        for sym in 0..n {
            let len = lengths[sym] as usize;
            if len == 0 {
                continue;
            }

            self.syms[sym_idx[len] as usize] = sym as u16;
            sym_idx[len] += 1;

            if len < HUFFMAN_LOOKUP_TABLE_BITS as usize {
                self.table_insert(sym, len, code[len]);
                code[len] += 1;
            }
        }

        Ok(())
    }

    pub fn table_insert(&mut self, sym: usize, len: usize, codeword: u16) {
        debug_assert!(len < HUFFMAN_LOOKUP_TABLE_BITS as usize);

        let codeword = reverse_lsb(codeword, len); // Make it LSB-first.
        let pad_len = HUFFMAN_LOOKUP_TABLE_BITS as usize - len;

        // Pad the pad_len upper bits with all bit combinations.
        for padding in 0..(1 << pad_len) {
            let index = (codeword | (padding << len)) as usize;
            debug_assert!(sym <= u16::MAX as usize);
            self.table[index].sym = sym as u16;
            debug_assert!(len <= u8::MAX as usize);
            self.table[index].len = len as u8;
        }
    }

    /// Use the decoder d to decode a symbol from the LSB-first zero-padded bits.
    /// Returns the decoded symbol number or an error if no symbol could be decoded.
    /// *num_used_bits will be set to the number of bits used to decode the symbol,
    /// or zero if no symbol could be decoded.
    pub fn huffman_decode(&mut self, bits: u16, num_used_bits: &mut u8) -> std::io::Result<u16> {
        // First try the lookup table.
        let lookup_bits = lsb(bits as u64, HUFFMAN_LOOKUP_TABLE_BITS) as usize;
        debug_assert!(lookup_bits < self.table.len());

        if self.table[lookup_bits].len != 0 {
            debug_assert!(self.table[lookup_bits].len < HUFFMAN_LOOKUP_TABLE_BITS);
            //  debug_assert!(self.table[lookup_bits].sym < self.num_syms);
            *num_used_bits = self.table[lookup_bits].len;
            return Ok(self.table[lookup_bits].sym);
        }

        // Then do canonical decoding with the bits in MSB-first order.
        let mut bits = reverse_lsb(bits, MAX_HUFFMAN_BITS);
        for l in HUFFMAN_LOOKUP_TABLE_BITS as usize + 1..MAX_HUFFMAN_BITS {
            if self.sentinel_bits[l] > bits as u32 {
                bits >>= MAX_HUFFMAN_BITS - l;

                let sym_idx = self.offset_first_sym_idx[l] + bits;
                // debug_assert(sym_idx < self.num_syms);

                *num_used_bits = l as u8;
                return Ok(self.syms[sym_idx as usize]);
            }
        }
        *num_used_bits = 0;
        Err(Error::new(
            io::ErrorKind::InvalidData,
            "huffman decode failed",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::HuffmanDecoder;

    #[test]
    fn test_huffman_decode_basic() {
        let lens = [
            3, // sym 0:  000
            3, // sym 1:  001
            3, // sym 2:  010
            3, // sym 3:  011
            3, // sym 4:  100
            3, // sym 5:  101
            4, // sym 6:  1100
            4, // sym 7:  1101
            0, // sym 8:
            0, // sym 9:
            0, // sym 10:
            0, // sym 11:
            0, // sym 12:
            0, // sym 13:
            0, // sym 14:
            0, // sym 15:
            6, // sym 16: 111110
            5, // sym 17: 11110
            4, // sym 18: 1110
        ];

        let mut d = HuffmanDecoder::default();
        d.init(&lens, lens.len()).unwrap();

        let mut used = 0;
        // 000 (msb-first) -> 000 (lsb-first)
        assert_eq!(d.huffman_decode(0x0, &mut used).unwrap(), 0);
        assert_eq!(used, 3);

        /* 011 (msb-first) -> 110 (lsb-first)*/
        assert_eq!(d.huffman_decode(0x6, &mut used).unwrap(), 3);
        assert_eq!(used, 3);

        /* 11110 (msb-first) -> 01111 (lsb-first)*/
        assert_eq!(d.huffman_decode(0x0f, &mut used).unwrap(), 17);
        assert_eq!(used, 5);

        /* 111110 (msb-first) -> 011111 (lsb-first)*/
        assert_eq!(d.huffman_decode(0x1f, &mut used).unwrap(), 16);
        assert_eq!(used, 6);

        /* 1111111 (msb-first) -> 1111111 (lsb-first)*/
        assert!(d.huffman_decode(0x7f, &mut used).is_err());

        /* Make sure used is set even when decoding fails. */
        assert_eq!(used, 0);
    }
}
