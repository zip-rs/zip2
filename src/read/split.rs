//! Traits for splitting and teeing file contents into multiple parallel streams.

macro_rules! interruptible_buffered_io_op {
    ($op:expr) => {
        match $op {
            Ok(n) => n,
            Err(e) if e.kind() == ::std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    };
}

macro_rules! syscall_errno {
    ($syscall:expr) => {
        match $syscall {
            rc if rc < 0 => return Err(::std::io::Error::last_os_error()),
            rc => rc,
        }
    };
}

pub mod file {
    use std::io;
    use std::mem::MaybeUninit;
    use std::ops;

    pub trait FixedFile {
        fn extent(&self) -> u64;

        #[inline(always)]
        fn convert_range(&self, range: impl ops::RangeBounds<u64>) -> io::Result<ops::Range<u64>> {
            let len = self.extent();
            let start = match range.start_bound() {
                ops::Bound::Included(&start) => start,
                ops::Bound::Excluded(start) => start.checked_add(1).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "start too large")
                })?,
                ops::Bound::Unbounded => 0,
            };
            let end = {
                let unclamped_end = match range.end_bound() {
                    ops::Bound::Included(end) => end.checked_add(1).ok_or_else(|| {
                        io::Error::new(io::ErrorKind::InvalidInput, "end too large")
                    })?,
                    ops::Bound::Excluded(&end) => end,
                    ops::Bound::Unbounded => len,
                };
                #[allow(clippy::let_and_return)]
                let clamped_end = unclamped_end.min(len);
                clamped_end
            };

            if start > end {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "start past end",
                ));
            }
            Ok(ops::Range { start, end })
        }

        #[inline(always)]
        fn range_len(&self, start: u64, len: usize) -> io::Result<usize> {
            let len: u64 = len.try_into().unwrap();
            let ops::Range { start, end } = self.convert_range(start..(start + len))?;
            let len: u64 = end - start;
            Ok(len.try_into().unwrap())
        }
    }

    pub trait InputFile: FixedFile {
        fn pread(&self, start: u64, buf: &mut [MaybeUninit<u8>]) -> io::Result<usize>;

        fn pread_all(&self, start: u64, buf: &mut [MaybeUninit<u8>]) -> io::Result<()> {
            let len: usize = buf.len();
            let mut input_offset: u64 = start;
            let mut remaining_to_read: usize = len;

            while remaining_to_read > 0 {
                let num_read: usize = interruptible_buffered_io_op![
                    self.pread(input_offset, &mut buf[(len - remaining_to_read)..])
                ];
                if num_read == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "pread less than expected range",
                    ));
                }
                assert!(num_read <= remaining_to_read);
                remaining_to_read -= num_read;
                let num_read_offset: u64 = num_read.try_into().unwrap();
                input_offset += num_read_offset;
            }

            Ok(())
        }
    }

    pub trait OutputFile: FixedFile {
        fn pwrite(&mut self, start: u64, buf: &[u8]) -> io::Result<usize>;

        fn pwrite_all(&mut self, start: u64, buf: &[u8]) -> io::Result<()> {
            let len: usize = buf.len();
            let mut output_offset: u64 = start;
            let mut remaining_to_write: usize = len;

            while remaining_to_write > 0 {
                let num_written: usize = interruptible_buffered_io_op![
                    self.pwrite(output_offset, &buf[(len - remaining_to_write)..])
                ];
                if num_written == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "pwrite less than expected range",
                    ));
                }
                assert!(num_written <= remaining_to_write);
                remaining_to_write -= num_written;
                let num_written_offset: u64 = num_written.try_into().unwrap();
                output_offset += num_written_offset;
            }

            Ok(())
        }
    }

    pub trait CopyRange {
        type InF: InputFile;
        type OutF: OutputFile;

        fn copy_file_range(
            &mut self,
            from: (&Self::InF, u64),
            to: (&mut Self::OutF, u64),
            len: usize,
        ) -> io::Result<usize>;

        fn copy_file_range_all(
            &mut self,
            from: (&Self::InF, u64),
            mut to: (&mut Self::OutF, u64),
            len: usize,
        ) -> io::Result<()> {
            #[allow(clippy::needless_borrow)]
            let (ref from, from_offset) = from;
            let (ref mut to, to_offset) = to;

            let mut remaining_to_copy: usize = len;
            let mut input_offset: u64 = from_offset;
            let mut output_offset: u64 = to_offset;

            while remaining_to_copy > 0 {
                let num_copied: usize = interruptible_buffered_io_op![self.copy_file_range(
                    (from, input_offset),
                    (to, output_offset),
                    remaining_to_copy,
                )];
                if num_copied == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "copied less than expected file range",
                    ));
                }
                assert!(num_copied <= remaining_to_copy);
                remaining_to_copy -= num_copied;
                let num_copied_offset: u64 = num_copied.try_into().unwrap();
                input_offset += num_copied_offset;
                output_offset += num_copied_offset;
            }

            Ok(())
        }
    }

    #[cfg(unix)]
    pub mod unix {
        use super::{CopyRange, FixedFile, InputFile, OutputFile};

        use std::fs;
        use std::io;
        use std::marker::PhantomData;
        use std::mem::MaybeUninit;
        use std::os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd, RawFd};
        use std::slice;

        use libc;

        #[derive(Debug, Copy, Clone)]
        pub struct FileInput<'fd> {
            handle: BorrowedFd<'fd>,
            extent: u64,
        }

        pub(crate) fn fstat(fd: RawFd) -> io::Result<libc::stat> {
            let fd: libc::c_int = fd;
            let mut stat: MaybeUninit<libc::stat> = MaybeUninit::uninit();

            syscall_errno![unsafe { libc::fstat(fd, stat.as_mut_ptr()) }];
            Ok(unsafe { stat.assume_init() })
        }

        pub(crate) fn get_len(fd: RawFd) -> io::Result<u64> {
            let libc::stat { st_size, .. } = fstat(fd)?;
            let size: u64 = st_size.try_into().unwrap();
            Ok(size)
        }

        impl<'fd> FileInput<'fd> {
            pub fn new(file: &'fd impl AsFd) -> io::Result<Self> {
                let handle = file.as_fd();
                let extent = get_len(handle.as_raw_fd())?;
                Ok(Self { handle, extent })
            }

            pub(crate) fn fd(&self) -> RawFd {
                self.handle.as_raw_fd()
            }

            #[allow(dead_code)]
            pub fn on_same_device(&self, to: &FileOutput) -> io::Result<bool> {
                let libc::stat {
                    st_dev: from_dev, ..
                } = fstat(self.fd())?;
                let libc::stat { st_dev: to_dev, .. } = fstat(to.fd())?;
                Ok(from_dev == to_dev)
            }
        }

        impl<'fd> FixedFile for FileInput<'fd> {
            fn extent(&self) -> u64 {
                self.extent
            }
        }

        impl<'fd> InputFile for FileInput<'fd> {
            fn pread(&self, start: u64, buf: &mut [MaybeUninit<u8>]) -> io::Result<usize> {
                let count = self.range_len(start, buf.len())?;

                let fd: libc::c_int = self.fd();
                let p: *mut libc::c_void = buf.as_mut_ptr().cast();
                let offset: libc::off_t = start.try_into().unwrap();

                let n: usize = syscall_errno![unsafe { libc::pread(fd, p, count, offset) }]
                    .try_into()
                    .unwrap();
                Ok(n)
            }
        }

        pub struct FileOutput {
            handle: OwnedFd,
            extent: u64,
        }

        impl FileOutput {
            pub fn new(file: fs::File, extent: u64) -> io::Result<Self> {
                file.set_len(extent)?;
                Ok(Self {
                    handle: file.into(),
                    extent,
                })
            }

            pub(crate) fn fd(&self) -> RawFd {
                self.handle.as_raw_fd()
            }

            pub fn into_file(self) -> fs::File {
                self.handle.into()
            }
        }

        impl FixedFile for FileOutput {
            fn extent(&self) -> u64 {
                self.extent
            }
        }

        impl OutputFile for FileOutput {
            fn pwrite(&mut self, start: u64, buf: &[u8]) -> io::Result<usize> {
                let count = self.range_len(start, buf.len())?;

                let fd: libc::c_int = self.fd();
                let p: *const libc::c_void = buf.as_ptr().cast();
                let offset: libc::off_t = start.try_into().unwrap();

                let n: usize = syscall_errno![unsafe { libc::pwrite(fd, p, count, offset) }]
                    .try_into()
                    .unwrap();
                Ok(n)
            }
        }

        pub struct FileBufferCopy<'infd, 'buf> {
            buf: &'buf mut [u8],
            _ph: PhantomData<&'infd u8>,
        }

        impl<'infd, 'buf> FileBufferCopy<'infd, 'buf> {
            pub fn new(buf: &'buf mut [u8]) -> Self {
                assert!(!buf.is_empty());
                Self {
                    buf,
                    _ph: PhantomData,
                }
            }
        }

        impl<'infd, 'buf> CopyRange for FileBufferCopy<'infd, 'buf> {
            type InF = FileInput<'infd>;
            type OutF = FileOutput;

            fn copy_file_range(
                &mut self,
                from: (&Self::InF, u64),
                mut to: (&mut Self::OutF, u64),
                len: usize,
            ) -> io::Result<usize> {
                #[allow(clippy::needless_borrow)]
                let (ref from, from_start) = from;
                let (ref mut to, to_start) = to;

                let buf_clamped_len = len.min(self.buf.len());
                let from_len = from.range_len(from_start, buf_clamped_len)?;
                let to_len = to.range_len(to_start, buf_clamped_len)?;
                let clamped_len = from_len.min(to_len);
                if clamped_len == 0 {
                    return Ok(0);
                }

                let clamped_buf: &'buf mut [MaybeUninit<u8>] = {
                    let p: *mut MaybeUninit<u8> = self.buf.as_mut_ptr().cast();
                    unsafe { slice::from_raw_parts_mut(p, clamped_len) }
                };

                let num_read: usize = from.pread(from_start, clamped_buf)?;
                assert!(num_read > 0);
                assert!(num_read <= clamped_buf.len());

                let result_buf: &'buf [u8] = {
                    let p: *const u8 = clamped_buf.as_mut_ptr().cast_const().cast();
                    unsafe { slice::from_raw_parts(p, num_read) }
                };

                /* TODO: use a ring buffer instead of .pwrite_all() here! */
                to.pwrite_all(to_start, result_buf)?;

                Ok(result_buf.len())
            }
        }

        #[cfg(test)]
        mod test {
            use super::*;

            use std::fs;
            use std::io::{self, prelude::*};
            use std::mem;

            use tempfile;

            fn readable_file(input: &[u8]) -> io::Result<fs::File> {
                let mut i = tempfile::tempfile()?;
                i.write_all(input)?;
                Ok(i)
            }

            #[allow(clippy::missing_transmute_annotations)]
            #[test]
            fn pread() {
                let i = readable_file(b"asdf").unwrap();
                let ii = FileInput::new(&i).unwrap();

                let buf: MaybeUninit<[u8; 10]> = MaybeUninit::zeroed();
                let mut buf: [MaybeUninit<u8>; 10] = unsafe { mem::transmute(buf) };
                assert_eq!(2, ii.pread(0, &mut buf[..2]).unwrap());
                assert_eq!(
                    unsafe { mem::transmute::<_, &[u8]>(&buf[..2]) },
                    b"as".as_ref()
                );
                assert_eq!(3, ii.pread(1, &mut buf[4..]).unwrap());
                assert_eq!(
                    unsafe { mem::transmute::<_, &[u8]>(&buf[..]) },
                    &[b'a', b's', 0, 0, b's', b'd', b'f', 0, 0, 0]
                );
            }

            #[test]
            fn pwrite() {
                let o = tempfile::tempfile().unwrap();
                let mut oo = FileOutput::new(o, 10).unwrap();

                let i = b"asdf";
                assert_eq!(2, oo.pwrite(0, &i[..2]).unwrap());
                assert_eq!(3, oo.pwrite(4, &i[1..]).unwrap());
                assert_eq!(1, oo.pwrite(9, &i[..]).unwrap());

                let mut o = oo.into_file();
                o.rewind().unwrap();
                let mut buf = Vec::new();
                o.read_to_end(&mut buf).unwrap();
                assert_eq!(&buf[..], &[b'a', b's', 0, 0, b's', b'd', b'f', 0, 0, b'a']);
            }

            #[test]
            fn copy_file_range() {
                let i = readable_file(b"asdf").unwrap();
                let ii = FileInput::new(&i).unwrap();

                let o = tempfile::tempfile().unwrap();
                let mut oo = FileOutput::new(o, 10).unwrap();

                /* Buffer is size 2, which limits the max size of individual copy_file_range()
                 * calls. */
                let mut buf = vec![0u8; 2].into_boxed_slice();

                let mut c = FileBufferCopy::new(&mut buf);
                assert_eq!(2, c.copy_file_range((&ii, 0), (&mut oo, 0), 2).unwrap());
                assert_eq!(2, c.copy_file_range((&ii, 1), (&mut oo, 4), 20).unwrap());
                assert_eq!(1, c.copy_file_range((&ii, 0), (&mut oo, 9), 35).unwrap());

                let mut o = oo.into_file();
                o.rewind().unwrap();
                let mut buf = Vec::new();
                o.read_to_end(&mut buf).unwrap();

                assert_eq!(&buf[..], &[b'a', b's', 0, 0, b's', b'd', 0, 0, 0, b'a']);
            }
        }
    }

    #[cfg(target_os = "linux")]
    pub mod linux {
        use super::unix::{FileInput, FileOutput};
        use super::{CopyRange, FixedFile};

        use std::io;
        use std::marker::PhantomData;

        use libc;

        pub struct FileCopy<'infd>(PhantomData<&'infd u8>);

        impl<'infd> FileCopy<'infd> {
            pub const fn new() -> Self {
                Self(PhantomData)
            }
        }

        impl<'infd> CopyRange for FileCopy<'infd> {
            type InF = FileInput<'infd>;
            type OutF = FileOutput;

            fn copy_file_range(
                &mut self,
                from: (&Self::InF, u64),
                to: (&mut Self::OutF, u64),
                len: usize,
            ) -> io::Result<usize> {
                let (from, from_start) = from;
                let (to, to_start) = to;

                let from_len = from.range_len(from_start, len)?;
                let to_len = to.range_len(to_start, len)?;
                let clamped_len = from_len.min(to_len);

                let from_fd: libc::c_int = from.fd();
                let mut from_offset: libc::off64_t = from_start.try_into().unwrap();
                let to_fd: libc::c_int = to.fd();
                let mut to_offset: libc::off64_t = to_start.try_into().unwrap();

                let flags: libc::c_uint = 0;

                let n: usize = syscall_errno![unsafe {
                    libc::copy_file_range(
                        from_fd,
                        &mut from_offset,
                        to_fd,
                        &mut to_offset,
                        clamped_len,
                        flags,
                    )
                }]
                .try_into()
                .unwrap();
                Ok(n)
            }
        }

        #[cfg(test)]
        mod test {
            use super::*;

            use std::fs;
            use std::io::{self, prelude::*};

            use tempfile;

            fn readable_file(input: &[u8]) -> io::Result<fs::File> {
                let mut i = tempfile::tempfile()?;
                i.write_all(input)?;
                Ok(i)
            }

            #[test]
            fn copy_file_range() {
                let i = readable_file(b"asdf").unwrap();
                let ii = FileInput::new(&i).unwrap();

                let o = tempfile::tempfile().unwrap();
                let mut oo = FileOutput::new(o, 10).unwrap();

                let mut c = FileCopy::new();
                assert_eq!(2, c.copy_file_range((&ii, 0), (&mut oo, 0), 2).unwrap());
                assert_eq!(3, c.copy_file_range((&ii, 1), (&mut oo, 4), 20).unwrap());
                assert_eq!(1, c.copy_file_range((&ii, 0), (&mut oo, 9), 35).unwrap());

                let mut o = oo.into_file();
                o.rewind().unwrap();
                let mut buf = Vec::new();
                o.read_to_end(&mut buf).unwrap();

                assert_eq!(&buf[..], &[b'a', b's', 0, 0, b's', b'd', b'f', 0, 0, b'a']);
            }
        }
    }
}

pub mod pipe {
    use super::file::{InputFile, OutputFile};

    use std::io;

    #[allow(dead_code)]
    pub trait WriteEnd: io::Write {}

    pub trait WriteSplicer {
        type InF: InputFile;
        type OutP: WriteEnd;

        fn splice_from_file(
            &mut self,
            from: (&Self::InF, u64),
            to: &mut Self::OutP,
            len: usize,
        ) -> io::Result<usize>;

        fn splice_from_file_all(
            &mut self,
            from: (&Self::InF, u64),
            to: &mut Self::OutP,
            len: usize,
        ) -> io::Result<()> {
            #[allow(clippy::needless_borrow)]
            let (ref from, from_offset) = from;

            let mut remaining_to_read: usize = len;
            let mut input_offset: u64 = from_offset;
            while remaining_to_read > 0 {
                let num_read: usize = interruptible_buffered_io_op![self.splice_from_file(
                    (from, input_offset),
                    to,
                    remaining_to_read
                )];
                if num_read == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "spliced less than expected range from file",
                    ));
                }
                assert!(num_read <= remaining_to_read);
                remaining_to_read -= num_read;
                let num_read_offset: u64 = num_read.try_into().unwrap();
                input_offset += num_read_offset;
            }

            Ok(())
        }
    }

    #[allow(dead_code)]
    pub trait ReadEnd: io::Read {}

    pub trait ReadSplicer {
        type InP: ReadEnd;
        type OutF: OutputFile;

        fn splice_to_file(
            &mut self,
            from: &mut Self::InP,
            to: (&mut Self::OutF, u64),
            len: usize,
        ) -> io::Result<usize>;

        fn splice_to_file_all(
            &mut self,
            from: &mut Self::InP,
            mut to: (&mut Self::OutF, u64),
            len: usize,
        ) -> io::Result<()> {
            let (ref mut to, to_offset) = to;

            let mut remaining_to_write: usize = len;
            let mut output_offset: u64 = to_offset;
            while remaining_to_write > 0 {
                let num_written: usize = interruptible_buffered_io_op![self.splice_to_file(
                    from,
                    (to, output_offset),
                    remaining_to_write
                )];
                if num_written == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "spliced less than expected range to file",
                    ));
                }
                assert!(num_written <= remaining_to_write);
                remaining_to_write -= num_written;
                let num_written_offset: u64 = num_written.try_into().unwrap();
                output_offset += num_written_offset;
            }

            Ok(())
        }
    }

    #[cfg(unix)]
    pub mod unix {
        use super::{ReadEnd, ReadSplicer, WriteEnd, WriteSplicer};

        use crate::read::split::file::unix::{FileInput, FileOutput};
        use crate::read::split::file::{FixedFile, InputFile, OutputFile};

        use std::io::{self, Read, Write};
        use std::marker::PhantomData;
        use std::mem::MaybeUninit;
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
        use std::slice;

        use libc;

        pub struct WritePipe {
            handle: OwnedFd,
        }

        impl WritePipe {
            pub(crate) unsafe fn from_fd(fd: RawFd) -> Self {
                Self {
                    handle: OwnedFd::from_raw_fd(fd),
                }
            }

            pub(crate) fn fd(&self) -> RawFd {
                self.handle.as_raw_fd()
            }
        }

        impl io::Write for WritePipe {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                let fd: libc::c_int = self.fd();

                /* TODO: use vmsplice() instead on linux! However, UB results if the buffer is
                 * modified before the data is read by the output: see
                 * https://stackoverflow.com/questions/70515745/how-do-i-use-vmsplice-to-correctly-output-to-a-pipe.
                 * This may be possible to handle with some sort of ring buffer, but for now let's
                 * take the hit and avoid race conditions by using write() on all unix-likes. */
                let n: usize =
                    syscall_errno![unsafe { libc::write(fd, buf.as_ptr().cast(), buf.len()) }]
                        .try_into()
                        .unwrap();
                Ok(n)
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        impl WriteEnd for WritePipe {}

        pub struct ReadPipe {
            handle: OwnedFd,
        }

        impl ReadPipe {
            pub(crate) unsafe fn from_fd(fd: RawFd) -> Self {
                Self {
                    handle: OwnedFd::from_raw_fd(fd),
                }
            }

            pub(crate) fn fd(&self) -> RawFd {
                self.handle.as_raw_fd()
            }
        }

        impl io::Read for ReadPipe {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                let fd: libc::c_int = self.fd();

                /* TODO: vmsplice() on linux currently offers no additional optimization for reads,
                 * so just use read() on all platforms. Also note as in WritePipe::write() that
                 * some sort of ring buffer is probably necessary to avoid race conditions if this
                 * optimization is performed. */
                let n: usize =
                    syscall_errno![unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) }]
                        .try_into()
                        .unwrap();
                Ok(n)
            }
        }

        impl ReadEnd for ReadPipe {}

        pub fn create_pipe() -> io::Result<(ReadPipe, WritePipe)> {
            let mut fds: [libc::c_int; 2] = [0; 2];
            syscall_errno![unsafe { libc::pipe(fds.as_mut_ptr()) }];
            let [r, w] = fds;
            let (r, w) = unsafe { (ReadPipe::from_fd(r), WritePipe::from_fd(w)) };
            Ok((r, w))
        }

        pub struct PipeWriteBufferSplicer<'infd, 'buf> {
            buf: &'buf mut [u8],
            _ph: PhantomData<&'infd u8>,
        }

        impl<'infd, 'buf> PipeWriteBufferSplicer<'infd, 'buf> {
            #[allow(dead_code)]
            pub fn new(buf: &'buf mut [u8]) -> Self {
                assert!(!buf.is_empty());
                Self {
                    buf,
                    _ph: PhantomData,
                }
            }
        }

        impl<'infd, 'buf> WriteSplicer for PipeWriteBufferSplicer<'infd, 'buf> {
            type InF = FileInput<'infd>;
            type OutP = WritePipe;

            fn splice_from_file(
                &mut self,
                from: (&Self::InF, u64),
                to: &mut Self::OutP,
                len: usize,
            ) -> io::Result<usize> {
                #[allow(clippy::needless_borrow)]
                let (ref from, from_start) = from;

                let buf_clamped_len = len.min(self.buf.len());
                let from_len = from.range_len(from_start, buf_clamped_len)?;
                let clamped_len = from_len;
                if clamped_len == 0 {
                    return Ok(0);
                }

                let clamped_buf: &'buf mut [MaybeUninit<u8>] = {
                    let p: *mut MaybeUninit<u8> = self.buf.as_mut_ptr().cast();
                    unsafe { slice::from_raw_parts_mut(p, clamped_len) }
                };

                let num_read: usize = from.pread(from_start, clamped_buf)?;
                assert!(num_read > 0);
                assert!(num_read <= clamped_buf.len());

                let result_buf: &'buf [u8] = {
                    let p: *const u8 = clamped_buf.as_mut_ptr().cast_const().cast();
                    unsafe { slice::from_raw_parts(p, num_read) }
                };

                /* TODO: use a ring buffer instead of .write_all() here! */
                to.write_all(result_buf)?;

                Ok(result_buf.len())
            }
        }

        pub struct PipeReadBufferSplicer<'buf> {
            buf: &'buf mut [u8],
        }

        impl<'buf> PipeReadBufferSplicer<'buf> {
            #[allow(dead_code)]
            pub fn new(buf: &'buf mut [u8]) -> Self {
                assert!(!buf.is_empty());
                Self { buf }
            }
        }

        impl<'buf> ReadSplicer for PipeReadBufferSplicer<'buf> {
            type InP = ReadPipe;
            type OutF = FileOutput;

            fn splice_to_file(
                &mut self,
                from: &mut Self::InP,
                mut to: (&mut Self::OutF, u64),
                len: usize,
            ) -> io::Result<usize> {
                let (ref mut to, to_start) = to;

                let buf_clamped_len = len.min(self.buf.len());
                let to_len = to.range_len(to_start, buf_clamped_len)?;
                let clamped_len = to_len;
                if clamped_len == 0 {
                    return Ok(0);
                }

                let clamped_buf: &'buf mut [u8] =
                    unsafe { slice::from_raw_parts_mut(self.buf.as_mut_ptr(), clamped_len) };

                let num_read: usize = from.read(clamped_buf)?;
                if num_read == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "expected to read nonzero from blocking pipe",
                    ));
                }
                assert!(num_read <= clamped_buf.len());

                let result_buf: &'buf [u8] = unsafe {
                    slice::from_raw_parts(clamped_buf.as_mut_ptr().cast_const(), num_read)
                };

                /* TODO: use a ring buffer instead of .pwrite_all() here! */
                to.pwrite_all(to_start, result_buf)?;

                Ok(result_buf.len())
            }
        }

        #[cfg(test)]
        mod test {
            use super::*;

            use std::fs;
            use std::io::prelude::*;
            use std::thread;

            use tempfile;

            fn readable_file(input: &[u8]) -> io::Result<fs::File> {
                let mut i = tempfile::tempfile()?;
                i.write_all(input)?;
                Ok(i)
            }

            #[test]
            fn read_write_pipe() {
                let (mut r, mut w) = create_pipe().unwrap();

                let t = thread::spawn(move || w.write_all(b"asdf"));
                /* The write end is dropped after the string is written, which stops .read_to_end()
                 * from blocking. */
                let mut buf: Vec<u8> = Vec::new();
                r.read_to_end(&mut buf).unwrap();
                assert_eq!(b"asdf".as_ref(), &buf[..]);
                t.join().unwrap().unwrap();
            }

            #[test]
            fn splice_from_file() {
                let (mut r, mut w) = create_pipe().unwrap();

                let t = thread::spawn(move || {
                    let i = readable_file(b"asdf").unwrap();
                    let ii = FileInput::new(&i).unwrap();
                    /* Buffer is size 2, which limits the max size of individual splice() calls. */
                    let mut buf = vec![0u8; 2].into_boxed_slice();
                    let mut s = PipeWriteBufferSplicer::new(&mut buf);
                    s.splice_from_file((&ii, 1), &mut w, 13)
                });

                let mut buf: Vec<u8> = Vec::new();
                r.read_to_end(&mut buf).unwrap();
                /* Started from offset 1, and buf limited to 2, so only get 2 chars. */
                assert_eq!(b"sd".as_ref(), &buf[..]);
                assert_eq!(2, t.join().unwrap().unwrap());
            }

            #[test]
            fn splice_to_file() {
                let o = tempfile::tempfile().unwrap();
                let mut oo = FileOutput::new(o, 5).unwrap();

                let (mut r, mut w) = create_pipe().unwrap();
                let t = thread::spawn(move || w.write_all(b"asdfasdf"));

                /* Buffer is size 2, which limits the max size of individual splice() calls. */
                let mut buf = vec![0u8; 2].into_boxed_slice();
                let mut s = PipeReadBufferSplicer::new(&mut buf);
                assert_eq!(2, s.splice_to_file(&mut r, (&mut oo, 2), 13).unwrap());

                let mut o = oo.into_file();
                o.rewind().unwrap();
                let mut buf: Vec<u8> = Vec::new();
                o.read_to_end(&mut buf).unwrap();

                /* Started from offset 2, and buf limited to 2, so only get 2 chars. */
                assert_eq!(&buf[..], &[0, 0, b'a', b's', 0]);

                /* Get remaining chars written. */
                buf.clear();
                r.read_to_end(&mut buf).unwrap();
                assert_eq!(&buf[..], b"dfasdf".as_ref());

                t.join().unwrap().unwrap();
            }
        }
    }

    #[cfg(target_os = "linux")]
    pub mod linux {
        use super::unix::{ReadPipe, WritePipe};
        use super::{ReadSplicer, WriteSplicer};

        use crate::read::split::file::unix::{FileInput, FileOutput};
        use crate::read::split::file::FixedFile;

        use std::io;
        use std::marker::PhantomData;
        use std::ptr;

        use libc;

        pub struct PipeWriteSplicer<'infd>(PhantomData<&'infd u8>);

        impl<'infd> PipeWriteSplicer<'infd> {
            pub const fn new() -> Self {
                Self(PhantomData)
            }
        }

        impl<'infd> WriteSplicer for PipeWriteSplicer<'infd> {
            type InF = FileInput<'infd>;
            type OutP = WritePipe;

            fn splice_from_file(
                &mut self,
                from: (&Self::InF, u64),
                to: &mut Self::OutP,
                len: usize,
            ) -> io::Result<usize> {
                let (from, from_start) = from;

                let count = from.range_len(from_start, len)?;

                let from_fd: libc::c_int = from.fd();
                let mut from_offset: libc::loff_t = from_start.try_into().unwrap();
                let to_fd: libc::c_int = to.fd();

                let flags: libc::c_uint = 0;
                let n: usize = syscall_errno![unsafe {
                    libc::splice(
                        from_fd,
                        &mut from_offset,
                        to_fd,
                        ptr::null_mut(),
                        count,
                        flags,
                    )
                }]
                .try_into()
                .unwrap();
                Ok(n)
            }
        }

        pub struct PipeReadSplicer;

        impl ReadSplicer for PipeReadSplicer {
            type InP = ReadPipe;
            type OutF = FileOutput;

            fn splice_to_file(
                &mut self,
                from: &mut Self::InP,
                to: (&mut Self::OutF, u64),
                len: usize,
            ) -> io::Result<usize> {
                let (to, to_start) = to;

                let count = to.range_len(to_start, len)?;

                let from_fd: libc::c_int = from.fd();
                let to_fd: libc::c_int = to.fd();
                let mut to_offset: libc::loff_t = to_start.try_into().unwrap();

                let flags: libc::c_uint = 0;
                let n: usize = syscall_errno![unsafe {
                    libc::splice(
                        from_fd,
                        ptr::null_mut(),
                        to_fd,
                        &mut to_offset,
                        count,
                        flags,
                    )
                }]
                .try_into()
                .unwrap();
                Ok(n)
            }
        }

        #[cfg(test)]
        mod test {
            use super::super::unix::create_pipe;
            use super::*;

            use std::fs;
            use std::io::prelude::*;
            use std::thread;

            use tempfile;

            fn readable_file(input: &[u8]) -> io::Result<fs::File> {
                let mut i = tempfile::tempfile()?;
                i.write_all(input)?;
                Ok(i)
            }

            #[test]
            fn splice_from_file() {
                let (mut r, mut w) = create_pipe().unwrap();
                let t = thread::spawn(move || {
                    let i = readable_file(b"asdf").unwrap();
                    let ii = FileInput::new(&i).unwrap();
                    let mut s = PipeWriteSplicer::new();
                    s.splice_from_file((&ii, 1), &mut w, 13)
                });

                let mut buf: Vec<u8> = Vec::new();
                r.read_to_end(&mut buf).unwrap();
                /* Started from offset 1, so only get 3 chars. */
                assert_eq!(b"sdf".as_ref(), &buf[..]);
                assert_eq!(3, t.join().unwrap().unwrap());
            }

            #[test]
            fn splice_to_file() {
                let o = tempfile::tempfile().unwrap();
                let mut oo = FileOutput::new(o, 5).unwrap();

                let (mut r, mut w) = create_pipe().unwrap();
                let t = thread::spawn(move || w.write_all(b"asdfasdf"));

                let mut s = PipeReadSplicer;
                assert_eq!(3, s.splice_to_file(&mut r, (&mut oo, 2), 13).unwrap());

                let mut o = oo.into_file();
                o.rewind().unwrap();
                let mut buf: Vec<u8> = Vec::new();
                o.read_to_end(&mut buf).unwrap();

                /* Started from offset 2, so only get 3 chars. */
                assert_eq!(&buf[..], &[0, 0, b'a', b's', b'd']);

                /* Get remaining chars written. */
                buf.clear();
                r.read_to_end(&mut buf).unwrap();
                assert_eq!(&buf[..], b"fasdf".as_ref());

                t.join().unwrap().unwrap();
            }
        }
    }
}

pub mod util {
    use std::io::{self, Read, Write};

    pub struct TakeWrite<W> {
        inner: W,
        limit: u64,
    }

    impl<W> TakeWrite<W> {
        pub const fn take(inner: W, limit: u64) -> Self {
            Self { inner, limit }
        }

        #[allow(dead_code)]
        #[inline(always)]
        pub const fn limit(&self) -> u64 {
            self.limit
        }

        #[allow(dead_code)]
        pub fn into_inner(self) -> W {
            self.inner
        }
    }

    impl<W> Write for TakeWrite<W>
    where
        W: Write,
    {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if self.limit == 0 {
                return Ok(0);
            }

            let buf_len: u64 = buf.len().try_into().unwrap();
            let to_write_offset: u64 = buf_len.min(self.limit);
            let to_write: usize = to_write_offset.try_into().unwrap();

            let num_written: usize = self.inner.write(&buf[..to_write])?;
            assert!(num_written <= to_write);
            let num_written_offset: u64 = num_written.try_into().unwrap();
            self.limit -= num_written_offset;
            Ok(num_written)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.inner.flush()
        }
    }

    pub fn copy_via_buf<R, W>(r: &mut R, w: &mut W, buf: &mut [u8]) -> io::Result<u64>
    where
        R: Read + ?Sized,
        W: Write + ?Sized,
    {
        assert!(!buf.is_empty());
        let mut total_copied: u64 = 0;

        loop {
            let num_read: usize = interruptible_buffered_io_op![r.read(buf)];
            if num_read == 0 {
                break;
            }
            let num_read_offset: u64 = num_read.try_into().unwrap();

            /* TODO: use a ring buffer instead of .write_all() here! */
            w.write_all(&buf[..num_read])?;
            total_copied += num_read_offset;
        }

        Ok(total_copied)
    }

    #[cfg(test)]
    mod test {
        use super::*;

        use tempfile;

        use std::fs;
        use std::io::{self, Cursor, Seek};

        fn readable_file(input: &[u8]) -> io::Result<fs::File> {
            let mut i = tempfile::tempfile()?;
            i.write_all(input)?;
            i.rewind()?;
            Ok(i)
        }

        #[test]
        fn take_write_copy() {
            let mut i = readable_file(b"asdf".as_ref()).unwrap();
            let out = Cursor::new(Vec::new());
            let mut limited = TakeWrite::take(out, 3);
            assert_eq!(3, limited.limit());

            let mut buf = [0u8; 15];

            assert_eq!(
                io::ErrorKind::WriteZero,
                copy_via_buf(&mut i, &mut limited, &mut buf[..])
                    .err()
                    .unwrap()
                    .kind()
            );
            assert_eq!(0, limited.limit());
            let out = limited.into_inner().into_inner();
            assert_eq!(&out[..], b"asd".as_ref());
        }
    }
}
