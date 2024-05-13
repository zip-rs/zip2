use std::io;

/// Get the n least significant bits of x.
pub fn lsb(x: u64, n: u8) -> u64 {
    debug_assert!(n <= 63);
    x & ((1u64 << (n as u32)) - 1)
}

/// Reverse the n least significant bits of x.
/// The (16 - n) most significant bits of the result will be zero.
pub fn reverse16(x: u16, n: usize) -> u16 {
    debug_assert!(n > 0);
    debug_assert!(n <= 16);
    return x.reverse_bits() >> (16 - n);
}

/*
pub fn round_up(x: usize, m: usize) -> usize {
    debug_assert!((m & (m - 1)) == 0, "m must be a power of two");
    (x + m - 1) & (-(m as i64)) as usize // Hacker's Delight (2nd), 3-1.
}
*/
/// Input bitstream.
pub struct BitStream<'a> {
    src: &'a [u8],     /* Source bytes. */
    bitpos: usize,     /* Position of the next bit to read. */
    bitpos_end: usize, /* Position of past-the-end bit. */
}

/// Initialize an input stream to present the n bytes from src as an LSB-first
/// bitstream.
impl<'a> BitStream<'a> {
    pub fn new(src: &'a [u8], n: usize) -> Self {
        Self {
            src,
            bitpos: 0,
            bitpos_end: n * 8,
        }
    }

    /// Get the next bits from the input stream. The number of bits returned is
    /// between ISTREAM_MIN_BITS and 64, depending on the position in the stream, or
    /// fewer if the end of stream is reached. The upper bits are zero-padded.
    pub fn bits(&mut self) -> u64 {
        let next = self.bitpos / 8;
        debug_assert!(next < self.src.len(), "Cannot read past end of stream.");

        let bits = if next + 8 <= self.src.len() {
            // Common case: read 8 bytes in one go.
            u64::from_le_bytes(self.src[next..next + 8].try_into().unwrap())
        } else {
            // Read the available bytes and zero-pad.
            let mut bits = 0;
            for i in 0..self.src.len() - next {
                bits |= (self.src[next + i] as u64).wrapping_shl(i as u32 * 8);
            }
            bits
        };

        return bits >> (self.bitpos % 8);
    }

    /// Advance n bits in the bitstream if possible. Returns false if that many bits
    /// are not available in the stream.
    pub fn advance(&mut self, n: u8) -> std::io::Result<()> {
        debug_assert!(self.bitpos <= self.bitpos_end);

        if self.bitpos_end - self.bitpos < n as usize {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "End of stream",
            ));
        }

        self.bitpos += n as usize;
        Ok(())
    }

    /// Align the input stream to the next 8-bit boundary and return a pointer to
    /// that byte, which may be the past-the-end-of-stream byte.
    pub fn _byte_align(&mut self) -> usize {
        debug_assert!(self.bitpos <= self.bitpos_end, "Not past end of stream.");
        self.bitpos = 8 * (self.bitpos / 8);
        debug_assert!(self.bitpos <= self.bitpos_end, "Not past end of stream.");
        return self.bitpos / 8;
    }

    pub fn bytes_read(&self) -> usize {
        (self.bitpos + 7) / 8
    }
}

pub const ISTREAM_MIN_BITS: usize = 64 - 7;

#[cfg(test)]
mod tests {
    use crate::legacy::bitstream::{lsb, reverse16};

    #[test]
    fn test_reverse16() {
        assert_eq!(reverse16(0x0000, 1), 0x0);
        assert_eq!(reverse16(0xffff, 1), 0x1);
        assert_eq!(reverse16(0x0000, 16), 0x0);
        assert_eq!(reverse16(0xffff, 16), 0xffff);
        // 0001 0010 0011 0100 -> 0010 1100 0100 1000
        assert_eq!(reverse16(0x1234, 16), 0x2c48);
        // 111 1111 0100 0001 -> 100 0001 0111 1111
        assert_eq!(reverse16(0x7f41, 15), 0x417f);
    }
    /*
    #[test]
    fn test_bits_round_up() {
        assert_eq!(round_up(0, 4), 0);
        assert_eq!(round_up(1, 4), 4);
        assert_eq!(round_up(2, 4), 4);
        assert_eq!(round_up(3, 4), 4);
        assert_eq!(round_up(4, 4), 4);
        assert_eq!(round_up(5, 4), 8);
    }*/

    #[test]
    fn test_bits_test_bits_lsbround_up() {
        assert_eq!(lsb(0x1122334455667788, 0), 0x0);
        assert_eq!(lsb(0x1122334455667788, 5), 0x8);
        assert_eq!(lsb(0x7722334455667788, 63), 0x7722334455667788);
    }

    #[test]
    fn test_istream_basic() {
        let bits = [0x47];
        let mut is = super::BitStream::new(&bits, 1);

        assert_eq!(lsb(is.bits(), 1), 1);
        is.advance(1).unwrap();
        assert_eq!(lsb(is.bits(), 1), 1);
        is.advance(1).unwrap();
        assert_eq!(lsb(is.bits(), 1), 1);
        is.advance(1).unwrap();
        assert_eq!(lsb(is.bits(), 1), 0);
        is.advance(1).unwrap();
        assert_eq!(lsb(is.bits(), 1), 0);
        is.advance(1).unwrap();
        assert_eq!(lsb(is.bits(), 1), 0);
        is.advance(1).unwrap();
        assert_eq!(lsb(is.bits(), 1), 1);
        is.advance(1).unwrap();
        assert_eq!(lsb(is.bits(), 1), 0);
        is.advance(1).unwrap();
    }

    #[test]
    fn test_istream_case1() {
        let bits = [0x45, 048];
        let mut is = super::BitStream::new(&bits, 9);
        assert_eq!(lsb(is.bits(), 3), 0x05);
        is.advance(3).unwrap();

        assert_eq!(lsb(is.bits(), 4), 0x08);
        is.advance(4).unwrap();
    }
}
