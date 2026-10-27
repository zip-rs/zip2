use std::io::{self, Error, Read};

use bitstream_io::{BitRead, BitReader, Endianness, LittleEndian};

const MIN_CODE_SIZE: u8 = 9;
const MAX_CODE_SIZE: u8 = 13;

const MAX_CODE: usize = (1 << MAX_CODE_SIZE) - 1;
const CONTROL_CODE: usize = 256;
const INC_CODE_SIZE: u16 = 1;
const PARTIAL_CLEAR: u16 = 2;

/// Number of codes available for the LZW dictionary (excluding control codes)
/// These are the codes from CONTROL_CODE+1 to MAX_CODE
const FREE_CODE_QUEUE_SIZE: usize = MAX_CODE - CONTROL_CODE + 1;

// const HASH_BITS: usize = MAX_CODE_SIZE + 1; /* For a load factor of 0.5. */
// const HASHTAB_SIZE: usize = 1 << HASH_BITS;
const UNKNOWN_LEN: u16 = u16::MAX;

struct CodeQueue {
    next_idx: usize,
    codes: [Option<u16>; FREE_CODE_QUEUE_SIZE],
}

impl CodeQueue {
    fn new() -> Self {
        let mut codes = [None; FREE_CODE_QUEUE_SIZE];
        // All free codes, CONTROL_CODE + 1 ..= MAX_CODE (8191 included), as in hwzip's
        // code_queue_init and PKZIP's encoder. Leaving MAX_CODE out made every code the
        // encoder assigns after it one off from the decoder's table.
        for (i, code) in ((CONTROL_CODE as u16 + 1)..=(MAX_CODE as u16)).enumerate() {
            codes[i] = Some(code);
        }
        Self { next_idx: 0, codes }
    }

    // Return the next code in the queue, or INVALID_CODE if the queue is empty.
    fn next(&self) -> Option<u16> {
        //   assert(q->next_idx < sizeof(q->codes) / sizeof(q->codes[0]));
        if let Some(Some(next)) = self.codes.get(self.next_idx) {
            Some(*next)
        } else {
            None
        }
    }

    /// Return and remove the next code from the queue, or return INVALID_CODE if
    /// the queue is empty.
    fn remove_next(&mut self) -> Option<u16> {
        let res = self.next();
        if res.is_some() {
            self.next_idx += 1;
        }
        res
    }
}

#[derive(Clone, Debug, Copy, Default)]
struct Codetab {
    last_dst_pos: usize,
    prefix_code: Option<u16>,
    len: u16,
    ext_byte: u8,
}

impl Codetab {
    pub fn create_new() -> [Self; MAX_CODE + 1] {
        let mut codetab = [Codetab::default(); MAX_CODE + 1];
        for (i, code) in codetab.iter_mut().enumerate().take(u8::MAX as usize + 1) {
            *code = Codetab {
                prefix_code: Some(i as u16),
                ext_byte: i as u8,
                len: 1,
                last_dst_pos: 0,
            };
        }
        codetab
    }
}

fn unshrink_partial_clear(codetab: &mut [Codetab], queue: &mut CodeQueue) {
    let mut is_prefix = [false; MAX_CODE + 1];

    // Scan for codes that have been used as a prefix.
    for code in codetab.iter().take(MAX_CODE + 1).skip(CONTROL_CODE + 1) {
        if let Some(prefix_code) = code.prefix_code {
            is_prefix[prefix_code as usize] = true;
        }
    }

    // Clear "non-prefix" codes in the table; populate the code queue.
    let mut code_queue_size = 0;
    for i in (CONTROL_CODE + 1)..=MAX_CODE {
        if !is_prefix[i] {
            codetab[i].prefix_code = None;
            queue.codes[code_queue_size] = Some(i as u16);
            code_queue_size += 1;
        }
    }
    // End-of-queue marker, unless every free code went back into the queue (it is then full).
    if let Some(end) = queue.codes.get_mut(code_queue_size) {
        *end = None;
    }
    queue.next_idx = 0;
}

/// Read the next code from the input stream and return it in next_code. Returns
/// `Ok(None)` if the end of the stream is reached. If the stream contains invalid
/// data or excessive consecutive control codes, returns `Err`.
fn read_code<T: Read, E: Endianness>(
    is: &mut BitReader<T, E>,
    code_size: &mut u8,
    codetab: &mut [Codetab],
    queue: &mut CodeQueue,
) -> io::Result<Option<u16>> {
    const MAX_CONSECUTIVE_CONTROL_CODES: usize = 256;
    for _ in 0..MAX_CONSECUTIVE_CONTROL_CODES {
        let code = is.read_var::<u16>(u32::from(*code_size))?;

        // Handle regular codes (the common case).
        if code != CONTROL_CODE as u16 {
            return Ok(Some(code));
        }

        // Handle control codes.
        if let Ok(control_code) = is.read_var::<u16>(u32::from(*code_size)) {
            match control_code {
                INC_CODE_SIZE => {
                    if *code_size >= MAX_CODE_SIZE {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "tried to increase code size when already at maximum",
                        ));
                    }
                    *code_size += 1;
                }
                PARTIAL_CLEAR => {
                    unshrink_partial_clear(codetab, queue);
                }
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("Invalid control code {}", control_code),
                    ));
                }
            }
        } else {
            return Ok(None);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "excessive consecutive LZW control codes",
    ))
}

/// Output the string represented by a code into dst at dst_pos.
///
/// # Returns
/// new first byte of the string, or io::Error InvalidData on invalid prefix code.
fn output_code(
    dst: &mut Vec<u8>,
    code: u16,
    prev_code: u16,
    codetab: &mut [Codetab],
    queue: &CodeQueue,
    first_byte: u8,
) -> io::Result<u8> {
    debug_assert!(code <= MAX_CODE as u16 && code != CONTROL_CODE as u16);
    if code <= u16::from(u8::MAX) {
        dst.push(code as u8);
        return Ok(code as u8);
    }
    if codetab[code as usize].prefix_code.is_none()
        || codetab[code as usize].prefix_code == Some(code)
    {
        // Reject invalid codes. Self-referential codes may exist in
        // the table but cannot be used.
        return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid code"));
    }
    if codetab[code as usize].len != UNKNOWN_LEN {
        // Output string with known length (the common case).
        let ct = &codetab[code as usize];
        for i in ct.last_dst_pos..ct.last_dst_pos + ct.len as usize {
            dst.push(dst[i]);
        }
        return Ok(dst[ct.last_dst_pos]);
    }

    // Output a string of unknown length. This happens when the prefix
    // was invalid (due to partial clearing) when the code was inserted into
    // the table. The prefix can then become valid when it's added to the
    // table at a later point.
    let prefix_code = codetab[code as usize]
        .prefix_code
        .ok_or_else(|| io::Error::other("Cannot get prefix code"))?;
    if cfg!(debug_assertions) {
        let tab_entry = codetab[code as usize];
        assert!(tab_entry.len == UNKNOWN_LEN);
        assert!(
            tab_entry
                .prefix_code
                .ok_or_else(|| io::Error::other("Cannot get prefix code"))? as usize
                > CONTROL_CODE
        );
    }

    if Some(prefix_code) == queue.next() {
        /* The prefix code hasn't been added yet, but we were just
        about to: the KwKwK case. Add the previous string extended
        with its first byte. */
        codetab[prefix_code as usize] = Codetab {
            prefix_code: Some(prev_code),
            ext_byte: first_byte,
            len: codetab[prev_code as usize].len + 1,
            last_dst_pos: codetab[prev_code as usize].last_dst_pos,
        };
        dst.push(first_byte);
    } else if codetab[prefix_code as usize].prefix_code.is_none() {
        // The prefix code is still invalid.
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid prefix code",
        ));
    }

    // Output the prefix string, then the extension byte.
    let ct = codetab[prefix_code as usize];
    let last_dst_pos = dst.len();
    let start = ct.last_dst_pos;
    let len = ct.len as usize;
    let first = dst[start];
    dst.extend_from_within(start..start + len);
    dst.push(codetab[code as usize].ext_byte);
    // Update the code table now that the string has a length and pos.
    debug_assert!(prev_code != code);

    codetab[code as usize].len = (len + 1) as u16;
    codetab[code as usize].last_dst_pos = last_dst_pos;

    Ok(first)
}

struct ShrinkState<T> {
    codetab: Box<[Codetab; MAX_CODE + 1]>,
    queue: Box<CodeQueue>,
    bit_reader: BitReader<T, LittleEndian>,
    code_size: u8,
    first_byte: u8,
    prev_code: u16,
    started: bool,
    uncompressed_size: usize,
}

impl<T> std::fmt::Debug for ShrinkState<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShrinkState")
            .field("code_size", &self.code_size)
            .field("started", &self.started)
            .field("uncompressed_size", &self.uncompressed_size)
            .finish_non_exhaustive()
    }
}

impl<T: Read> ShrinkState<T> {
    fn new(src: T, uncompressed_size: usize) -> Self {
        Self {
            codetab: Box::new(Codetab::create_new()),
            queue: Box::new(CodeQueue::new()),
            bit_reader: BitReader::endian(src, LittleEndian),
            code_size: MIN_CODE_SIZE,
            first_byte: 0,
            prev_code: 0,
            started: false,
            uncompressed_size,
        }
    }

    fn unshrink_until(&mut self, target_len: usize, dst: &mut Vec<u8>) -> io::Result<()> {
        let target_len = target_len.min(self.uncompressed_size);
        if dst.len() < target_len {
            dst.try_reserve(
                target_len
                    .saturating_sub(dst.len())
                    .min(crate::legacy::MAX_PREALLOC),
            )
            .map_err(|e| io::Error::new(io::ErrorKind::OutOfMemory, e))?;
        }

        if !self.started {
            self.started = true;
            // Handle the first code separately since there is no previous code.
            let Some(curr_code) = read_code(
                &mut self.bit_reader,
                &mut self.code_size,
                &mut *self.codetab,
                &mut self.queue,
            )?
            else {
                return Ok(());
            };

            debug_assert!(curr_code != CONTROL_CODE as u16);
            if curr_code > u16::from(u8::MAX) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "the first code must be a literal",
                ));
            }
            self.first_byte = curr_code as u8;
            self.codetab[curr_code as usize].last_dst_pos = dst.len();
            dst.push(self.first_byte);
            self.prev_code = curr_code;
        }

        while dst.len() < target_len {
            let curr_opt = match read_code(
                &mut self.bit_reader,
                &mut self.code_size,
                &mut *self.codetab,
                &mut self.queue,
            ) {
                Ok(c) => c,
                Err(_) => break, // treat read error as end-of-stream for now; validated after loop
            };

            let Some(curr_code) = curr_opt else {
                return Err(Error::new(io::ErrorKind::InvalidData, "invalid code"));
            };

            let dst_pos = dst.len();
            // Handle KwKwK: next code used before being added.
            if Some(curr_code) == self.queue.next() {
                if self.codetab[self.prev_code as usize].prefix_code.is_none() {
                    return Err(Error::new(
                        io::ErrorKind::InvalidData,
                        "previous code no longer valid",
                    ));
                }
                // Extend the previous code with its first byte.
                debug_assert!(curr_code != self.prev_code);
                self.codetab[curr_code as usize] = Codetab {
                    prefix_code: Some(self.prev_code),
                    ext_byte: self.first_byte,
                    len: self.codetab[self.prev_code as usize].len + 1,
                    last_dst_pos: self.codetab[self.prev_code as usize].last_dst_pos,
                };
            }
            // Output the string represented by the current code.
            self.first_byte = output_code(
                dst,
                curr_code,
                self.prev_code,
                &mut *self.codetab,
                &self.queue,
                self.first_byte,
            )?;
            if let Some(new_code) = self.queue.remove_next() {
                let prev_entry = self.codetab[self.prev_code as usize];
                self.codetab[new_code as usize] = Codetab {
                    prefix_code: Some(self.prev_code),
                    ext_byte: self.first_byte,
                    last_dst_pos: prev_entry.last_dst_pos,
                    len: if prev_entry.prefix_code.is_none() {
                        // prev_code was invalidated in a partial
                        // clearing. Until that code is re-used, the
                        // string represented by new_code is
                        // indeterminate.
                        UNKNOWN_LEN
                    } else {
                        prev_entry.len + 1
                    },
                };
            }
            self.codetab[curr_code as usize].last_dst_pos = dst_pos;
            self.prev_code = curr_code;
        }

        if dst.len() < target_len
            || (target_len == self.uncompressed_size && dst.len() != self.uncompressed_size)
        {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "unexpected end of compressed stream",
            ));
        }

        Ok(())
    }
}

#[cfg(test)]
fn hwunshrink(src: &[u8], uncompressed_size: usize, dst: &mut Vec<u8>) -> io::Result<()> {
    let mut state = ShrinkState::new(src, uncompressed_size);
    state.unshrink_until(uncompressed_size, dst)
}

#[derive(Debug)]
pub struct ShrinkDecoder<R> {
    compressed_reader: R,
    state: Option<ShrinkState<io::Cursor<Vec<u8>>>>,
    uncompressed_size: u64,
    stream: Vec<u8>,
    read_pos: usize,
}

impl<R: Read> ShrinkDecoder<R> {
    pub fn new(inner: R, uncompressed_size: u64) -> Self {
        Self {
            compressed_reader: inner,
            uncompressed_size,
            state: None,
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

impl<R: Read> Read for ShrinkDecoder<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let state = match &mut self.state {
            Some(state) => state,
            None => {
                let uncompressed_size = usize::try_from(self.uncompressed_size).map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "uncompressed size exceeds usize::MAX",
                    )
                })?;
                let mut compressed_bytes = Vec::new();
                self.compressed_reader.read_to_end(&mut compressed_bytes)?;
                self.state.insert(ShrinkState::new(
                    io::Cursor::new(compressed_bytes),
                    uncompressed_size,
                ))
            }
        };

        let target_len = self
            .read_pos
            .saturating_add(buf.len())
            .min(state.uncompressed_size);
        if self.stream.len() < target_len {
            state.unshrink_until(target_len, &mut self.stream)?;
        }

        let available = self.stream.len().saturating_sub(self.read_pos);
        if available == 0 {
            return Ok(0);
        }
        let n = available.min(buf.len());
        buf[..n].copy_from_slice(&self.stream[self.read_pos..self.read_pos + n]);
        self.read_pos += n;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use crate::legacy::shrink::hwunshrink;

    const LZW_FIG5: &[u8; 17] = b"ababcbababaaaaaaa";
    const LZW_FIG5_SHRUNK: [u8; 12] = [
        0x61, 0xc4, 0x04, 0x1c, 0x23, 0xb0, 0x60, 0x98, 0x83, 0x08, 0xc3, 0x00,
    ];

    #[test]
    fn test_unshrink_lzw_fig5() {
        let mut dst = Vec::with_capacity(LZW_FIG5.len());
        hwunshrink(&LZW_FIG5_SHRUNK, LZW_FIG5.len(), &mut dst).unwrap();
        assert_eq!(dst.as_slice(), LZW_FIG5);
    }

    #[test]
    fn test_shrink_decoder_incremental_take() {
        use super::ShrinkDecoder;
        use std::io::Read;

        let decoder = ShrinkDecoder::new(LZW_FIG5_SHRUNK.as_slice(), LZW_FIG5.len() as u64);
        let mut limited = decoder.take(6);
        let mut out = Vec::new();
        limited.read_to_end(&mut out).unwrap();
        assert_eq!(out, &LZW_FIG5[..6]);
        assert!(limited.get_ref().stream.len() <= 8);
    }

    #[test]
    #[cfg(not(miri))]
    fn test_shrink_uncontrolled_recursion_prevention() {
        use bitstream_io::{BitWrite, BitWriter, LittleEndian};

        let mut buf = Vec::new();
        {
            let mut writer = BitWriter::endian(&mut buf, LittleEndian);
            // Write 500 consecutive (CONTROL_CODE=256, PARTIAL_CLEAR=2) pairs
            for _ in 0..500 {
                writer.write::<9, _>(256u32).unwrap();
                writer.write::<9, _>(2u32).unwrap();
            }
            writer.write::<9, _>(65u32).unwrap();
            writer.byte_align().unwrap();
        }

        let mut dst = Vec::new();
        let res = hwunshrink(&buf, 1, &mut dst);
        assert!(
            res.is_err(),
            "Should return error on excessive consecutive control codes"
        );
    }
}
