use std::collections::VecDeque;
use std::io::{self, copy, Error, Read};

use super::bitstream::{lsb, BitStream};

const MIN_CODE_SIZE: u8 = 9;
const MAX_CODE_SIZE: u8 = 13;

const MAX_CODE: usize = (1 << MAX_CODE_SIZE) - 1;
const INVALID_CODE: u16 = u16::MAX;
const CONTROL_CODE: usize = 256;
const INC_CODE_SIZE: u64 = 1;
const PARTIAL_CLEAR: u64 = 2;

// const HASH_BITS: usize = MAX_CODE_SIZE + 1; /* For a load factor of 0.5. */
// const HASHTAB_SIZE: usize = 1 << HASH_BITS;
const UNKNOWN_LEN: u16 = u16::MAX;
/*
#[derive(Error, Debug)]
enum ShrinkError {
    #[error("self-referential code")]
    InvalidPrefixCode,

    #[error("first code needs to be literal")]
    FirstCodeNeedsToBeLiteral,

    #[error("invalid code")]
    InvalidCode,

    #[error("prev code no longer valid")]
    PrevCodeNoLongerValid,
}*/
struct CodeQueue {
    next_idx: usize,
    codes: [u16; MAX_CODE as usize - CONTROL_CODE + 1],
}

impl CodeQueue {
    fn new() -> Self {
        let mut codes = [0; MAX_CODE as usize - CONTROL_CODE + 1];
        for (i, code) in (CONTROL_CODE as u16 + 1..=MAX_CODE as u16).enumerate() {
            codes[i] = code;
        }
        Self { next_idx: 0, codes }
    }

    // Return the next code in the queue, or INVALID_CODE if the queue is empty.
    fn next(&self) -> u16 {
        //   assert(q->next_idx < sizeof(q->codes) / sizeof(q->codes[0]));
        self.codes[self.next_idx]
    }

    /// Return and remove the next code from the queue, or return INVALID_CODE if
    /// the queue is empty.
    fn remove_next(&mut self) -> u16 {
        let code = self.next();
        if code != INVALID_CODE {
            self.next_idx += 1;
        }
        code
    }
}

#[derive(Clone, Copy)]
struct Codetab {
    prefix_code: u16, // INVALID_CODE means the entry is invalid.
    ext_byte: u8,
    len: u16,
    last_dst_pos: usize,
}

impl Default for Codetab {
    fn default() -> Self {
        Self {
            prefix_code: INVALID_CODE,
            ext_byte: 0,
            len: 0,
            last_dst_pos: 0,
        }
    }
}

impl Codetab {
    pub fn new() -> [Self; MAX_CODE + 1] {
        let mut codetab = 0..=u8::MAX
            .map(|i| Codetab {
                prefix_code: i as u16,
                ext_byte: i,
                len: 1,
            })
            .collect::Vec<_>();
        codetab.resize(MAX_CODE + 1, Codetab::default());
        codetab.try_into().unwrap()
    }
}
fn unshrink_partial_clear(codetab: &mut [Codetab], queue: &mut CodeQueue) {
    let mut is_prefix = [false; MAX_CODE + 1];

    // Scan for codes that have been used as a prefix.
    for i in CONTROL_CODE + 1..=MAX_CODE {
        if codetab[i].prefix_code != INVALID_CODE {
            is_prefix[codetab[i].prefix_code as usize] = true;
        }
    }

    // Clear "non-prefix" codes in the table; populate the code queue.
    let mut code_queue_size = 0;
    for i in CONTROL_CODE + 1..MAX_CODE {
        if !is_prefix[i] {
            codetab[i].prefix_code = INVALID_CODE;
            queue.codes[code_queue_size] = i as u16;
            code_queue_size += 1;
        }
    }
    queue.codes[code_queue_size] = INVALID_CODE; // End-of-queue marker.
    queue.next_idx = 0;
}

/// Read the next code from the input stream and return it in next_code. Returns
/// false if the end of the stream is reached. If the stream contains invalid
/// data, next_code is set to INVALID_CODE but the return value is still true.
fn read_code(
    is: &mut BitStream,
    code_size: &mut u8,
    codetab: &mut [Codetab],
    queue: &mut CodeQueue,
    next_code: &mut u16,
) -> io::Result<()> {
    // assert(sizeof(code) * CHAR_BIT >= *code_size);
    let code = lsb(is.bits(), *code_size) as u16;
    is.advance(*code_size)?;

    // Handle regular codes (the common case).
    if code != CONTROL_CODE as u16 {
        *next_code = code;
        return Ok(());
    }

    // Handle control codes.
    let control_code = lsb(is.bits(), *code_size);
    if is.advance(*code_size).is_err() {
        *next_code = INVALID_CODE;
        return Ok(());
    }
    if control_code == INC_CODE_SIZE && *code_size < MAX_CODE_SIZE {
        (*code_size) += 1;
        read_code(is, code_size, codetab, queue, next_code)?;
        return Ok(());
    }
    if control_code == PARTIAL_CLEAR {
        unshrink_partial_clear(codetab, queue);
        read_code(is, code_size, codetab, queue, next_code)?;
        return Ok(());
    }
    *next_code = INVALID_CODE;
    return Ok(());
}

/// Output the string represented by a code into dst at dst_pos. Returns
/// HWUNSHRINK_OK on success, and also updates *first_byte and *len with the
/// first byte and length of the output string, respectively.
fn output_code(
    code: u16,
    dst: &mut VecDeque<u8>,
    prev_code: u16,
    codetab: &mut [Codetab],
    queue: &mut CodeQueue,
    first_byte: &mut u8,
    len: &mut usize,
) -> io::Result<()> {
    debug_assert!(code <= MAX_CODE as u16 && code != CONTROL_CODE as u16);
    if code <= u8::MAX as u16 {
        // Output literal byte.
        *first_byte = code as u8;
        *len = 1;
        dst.push_back(code as u8);
        return Ok(());
    }

    if codetab[code as usize].prefix_code == INVALID_CODE
        || codetab[code as usize].prefix_code == code
    {
        // Reject invalid codes. Self-referential codes may exist in
        // the table but cannot be used.
        return Err(io::Error::new(io::ErrorKind::InvalidData, "Invalid code"));
    }

    if codetab[code as usize].len != UNKNOWN_LEN {
        // Output string with known length (the common case).
        let ct = &codetab[code as usize];
        for i in ct.last_dst_pos..ct.last_dst_pos + ct.len as usize {
            dst.push_back(dst[i]);
        }
        *first_byte = dst[ct.last_dst_pos];
        *len = ct.len as usize;
        return Ok(());
    }

    // Output a string of unknown length. This happens when the prefix
    // was invalid (due to partial clearing) when the code was inserted into
    // the table. The prefix can then become valid when it's added to the
    // table at a later point.
    debug_assert!(codetab[code as usize].len == UNKNOWN_LEN);
    let prefix_code = codetab[code as usize].prefix_code;
    debug_assert!(prefix_code as usize > CONTROL_CODE);

    if prefix_code == queue.next() {
        /* The prefix code hasn't been added yet, but we were just
        about to: the KwKwK case. Add the previous string extended
        with its first byte. */
        debug_assert!(codetab[prev_code as usize].prefix_code != INVALID_CODE);
        codetab[prefix_code as usize].prefix_code = prev_code;
        codetab[prefix_code as usize].ext_byte = *first_byte;
        codetab[prefix_code as usize].len = codetab[prev_code as usize].len + 1;
        codetab[prefix_code as usize].last_dst_pos = codetab[prev_code as usize].last_dst_pos;
        dst.push_back(*first_byte);
    } else if codetab[prefix_code as usize].prefix_code == INVALID_CODE {
        // The prefix code is still invalid.
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Invalid prefix code",
        ));
    }

    // Output the prefix string, then the extension byte.
    *len = codetab[prefix_code as usize].len as usize + 1;
    let last_dst_pos = dst.len();
    let ct = &codetab[prefix_code as usize];
    for i in ct.last_dst_pos..ct.last_dst_pos + ct.len as usize {
        dst.push_back(dst[i]);
    }
    dst.push_back(codetab[code as usize].ext_byte);
    *first_byte = dst[ct.last_dst_pos];

    // Update the code table now that the string has a length and pos.
    debug_assert!(prev_code != code);
    codetab[code as usize].len = *len as u16;
    codetab[code as usize].last_dst_pos = last_dst_pos;

    Ok(())
}

fn hwunshrink(
    src: &[u8],
    src_len: usize,
    uncompressed_size: usize,
    src_used: &mut usize,
    dst: &mut VecDeque<u8>,
) -> io::Result<()> {
    let mut codetab = Codetab::new();
    let mut queue = CodeQueue::new();
    let mut is = BitStream::new(src, src_len);
    let mut code_size = MIN_CODE_SIZE;

    // Handle the first code separately since there is no previous code.
    let mut curr_code = 0;
    if read_code(
        &mut is,
        &mut code_size,
        &mut codetab,
        &mut queue,
        &mut curr_code,
    )
    .is_err()
    {
        *src_used = is.bytes_read();
        return Ok(());
    }
    debug_assert!(curr_code != CONTROL_CODE as u16);
    if curr_code > u8::MAX as u16 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "the first code must be a literal",
        ));
    }
    let mut first_byte = curr_code as u8;
    codetab[curr_code as usize].last_dst_pos = dst.len();
    dst.push_back(curr_code as u8);

    let mut prev_code = curr_code;
    while dst.len() < uncompressed_size
        && read_code(
            &mut is,
            &mut code_size,
            &mut codetab,
            &mut queue,
            &mut curr_code,
        )
        .is_ok()
    {
        println!("{}", dst.len());
        if curr_code == INVALID_CODE {
            return Err(Error::new(io::ErrorKind::InvalidData, "Invalid code"));
        }
        let dst_pos = dst.len();
        // Handle KwKwK: next code used before being added.
        if curr_code == queue.next() {
            if codetab[prev_code as usize].prefix_code == INVALID_CODE {
                return Err(Error::new(
                    io::ErrorKind::InvalidData,
                    "Previous code no longer valid",
                ));
            }
            // Extend the previous code with its first byte.
            debug_assert!(curr_code != prev_code);
            codetab[curr_code as usize].prefix_code = prev_code;
            codetab[curr_code as usize].ext_byte = first_byte;
            codetab[curr_code as usize].len = codetab[prev_code as usize].len + 1;
            codetab[curr_code as usize].last_dst_pos = codetab[prev_code as usize].last_dst_pos;
            //  dst.push_back(first_byte);
        }

        // Output the string represented by the current code.
        let mut len = 0;
        if let Err(s) = output_code(
            curr_code,
            dst,
            prev_code,
            &mut codetab,
            &mut queue,
            &mut first_byte,
            &mut len,
        ) {
            return Err(s);
        }

        // Verify that the output matches walking the prefixes.
        let mut c = curr_code;
        for i in 0..len {
            debug_assert!(codetab[c as usize].len as usize == len - i);
            //  debug_assert!(codetab[c as usize].ext_byte == dst[dst_pos + len - i - 1]);
            c = codetab[c as usize].prefix_code;
        }

        // Add a new code to the string table if there's room.
        // The string is the previous code's string extended with
        // the first byte of the current code's string.
        let new_code = queue.remove_next();
        if new_code != INVALID_CODE {
            //debug_assert!(codetab[prev_code as usize].last_dst_pos < dst_pos);
            codetab[new_code as usize].prefix_code = prev_code;
            codetab[new_code as usize].ext_byte = first_byte;
            codetab[new_code as usize].len = codetab[prev_code as usize].len + 1;
            codetab[new_code as usize].last_dst_pos = codetab[prev_code as usize].last_dst_pos;

            if codetab[prev_code as usize].prefix_code == INVALID_CODE {
                // prev_code was invalidated in a partial
                // clearing. Until that code is re-used, the
                // string represented by new_code is
                // indeterminate.
                codetab[new_code as usize].len = UNKNOWN_LEN;
            }
            // If prev_code was invalidated in a partial clearing,
            // it's possible that new_code==prev_code, in which
            // case it will never be used or cleared.
        }

        codetab[curr_code as usize].last_dst_pos = dst_pos;
        prev_code = curr_code;
    }

    *src_used = is.bytes_read();
    Ok(())
}

#[derive(Debug)]
pub struct ShrinkDecoder<R> {
    compressed_reader: R,
    stream_read: bool,
    uncompressed_size: u64,
    stream: VecDeque<u8>,
}

impl<R: Read> ShrinkDecoder<R> {
    pub fn new(inner: R, uncompressed_size: u64) -> Self {
        ShrinkDecoder {
            compressed_reader: inner,
            uncompressed_size,
            stream_read: false,
            stream: VecDeque::new(),
        }
    }

    pub fn finish(mut self) -> std::io::Result<VecDeque<u8>> {
        copy(&mut self.compressed_reader, &mut self.stream)?;
        Ok(self.stream)
    }
}

impl<R: Read> Read for ShrinkDecoder<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if !self.stream_read {
            self.stream_read = true;
            let mut compressed_bytes = Vec::new();
            if let Err(err) = self.compressed_reader.read_to_end(&mut compressed_bytes) {
                return Err(err.into());
            }
            let mut src_used = compressed_bytes.len();
            hwunshrink(
                &compressed_bytes,
                compressed_bytes.len(),
                self.uncompressed_size as usize,
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
    use crate::legacy::shrink::hwunshrink;
    use std::collections::VecDeque;

    const LZW_FIG5: &[u8; 17] = b"ababcbababaaaaaaa";
    const LZW_FIG5_SHRUNK: [u8; 12] = [
        0x61, 0xc4, 0x04, 0x1c, 0x23, 0xb0, 0x60, 0x98, 0x83, 0x08, 0xc3, 0x00,
    ];

    #[test]
    fn test_unshrink_lzw_fig5() {
        let mut dst = VecDeque::new();
        let mut src_used = 0;
        hwunshrink(
            &LZW_FIG5_SHRUNK,
            LZW_FIG5_SHRUNK.len(),
            LZW_FIG5.len(),
            &mut src_used,
            &mut dst,
        )
        .unwrap();
        assert_eq!(dst, LZW_FIG5);
    }
}
