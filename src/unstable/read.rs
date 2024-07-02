//! Alternate implementation of [`crate::read`].

use crate::compression::CompressionMethod;
use crate::crc32::non_crypto::Crc32Reader;
use crate::read::{find_data_start, EntryData, HasZipMetadata};
use crate::result::{ZipError, ZipResult};
use crate::types::{DateTime, ZipFileData};
use crate::zipcrypto::{ZipCryptoReader, ZipCryptoReaderValid, ZipCryptoValidator};

#[cfg(feature = "lzma")]
use crate::read::lzma::LzmaDecoder;
#[cfg(feature = "xz")]
use crate::read::xz::XzDecoder;
#[cfg(feature = "aes-crypto")]
use crate::{
    aes::{AesReader, AesReaderValid},
    read::AesInfo,
    types::{AesModeInfo, AesVendorVersion},
};

#[cfg(feature = "bzip2")]
use bzip2::read::BzDecoder;
#[cfg(feature = "deflate64")]
use deflate64::Deflate64Decoder;
#[cfg(feature = "deflate-flate2")]
use flate2::read::DeflateDecoder;
#[cfg(feature = "zstd")]
use zstd::stream::read::Decoder as ZstdDecoder;

use std::io::{self, Read, Seek};

pub(crate) enum EntryReader<R> {
    Stored(R),
    #[cfg(feature = "_deflate-any")]
    Deflated(DeflateDecoder<R>),
    #[cfg(feature = "deflate64")]
    Deflate64(Deflate64Decoder<io::BufReader<R>>),
    #[cfg(feature = "bzip2")]
    Bzip2(BzDecoder<R>),
    #[cfg(feature = "zstd")]
    Zstd(ZstdDecoder<'static, io::BufReader<R>>),
    #[cfg(feature = "lzma")]
    /* According to clippy, this is >30x larger than the other variants, so we box it to avoid
     * unnecessary large stack allocations. */
    Lzma(Box<LzmaDecoder<io::BufReader<R>>>),
    #[cfg(feature = "xz")]
    Xz(XzDecoder<io::BufReader<R>>),
}

impl<R> Read for EntryReader<R>
where
    R: Read,
{
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Stored(r) => r.read(buf),
            #[cfg(feature = "_deflate-any")]
            Self::Deflated(r) => r.read(buf),
            #[cfg(feature = "deflate64")]
            Self::Deflate64(r) => r.read(buf),
            #[cfg(feature = "bzip2")]
            Self::Bzip2(r) => r.read(buf),
            #[cfg(feature = "zstd")]
            Self::Zstd(r) => r.read(buf),
            #[cfg(feature = "lzma")]
            Self::Lzma(r) => r.read(buf),
            #[cfg(feature = "xz")]
            Self::Xz(r) => r.read(buf),
        }
    }
}

/// A struct for reading a zip file
pub struct ZipEntry<'a, R> {
    pub(crate) data: &'a ZipFileData,
    pub(crate) reader: R,
}

impl<'a, R> Read for ZipEntry<'a, R>
where
    R: Read,
{
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.reader.read(buf)
    }
}

impl<'a, R> ZipEntry<'a, R>
where
    R: Read,
{
    /// Returns the verification value and salt for the AES encryption of the file
    ///
    /// # Returns
    ///
    /// - None if the file is not encrypted with AES
    #[cfg(feature = "aes-crypto")]
    pub fn get_aes_verification_key_and_salt(self) -> ZipResult<Option<AesInfo>> {
        let Self { data, reader } = self;
        if let Some(AesModeInfo { aes_mode, .. }) = data.aes_mode {
            let (verification_value, salt) = AesReader::new(reader, aes_mode, data.compressed_size)
                .get_verification_value_and_salt()?;
            let aes_info = AesInfo {
                aes_mode,
                verification_value,
                salt,
            };
            Ok(Some(aes_info))
        } else {
            Ok(None)
        }
    }
}

impl<'a, R> HasZipMetadata for ZipEntry<'a, R> {
    fn get_metadata(&self) -> &ZipFileData {
        self.data
    }
}

impl<'a, R> EntryData for ZipEntry<'a, R> {}

pub(crate) fn find_entry_content_range<R>(
    data: &ZipFileData,
    mut reader: R,
) -> Result<io::Take<R>, ZipError>
where
    R: Read + Seek,
{
    // TODO: use .get_or_try_init() once stabilized to provide a closure returning a Result!
    let data_start = match data.data_start.get() {
        Some(data_start) => *data_start,
        None => find_data_start(data, &mut reader)?,
    };

    reader.seek(io::SeekFrom::Start(data_start))?;
    Ok(reader.take(data.compressed_size))
}

pub(crate) fn construct_decompressing_reader<R>(
    compression_method: &CompressionMethod,
    reader: R,
) -> Result<EntryReader<R>, ZipError>
where
    /* TODO: this really shouldn't be required upon construction (especially since the reader
     * doesn't need to be mutable, indicating the Read capability isn't used), but multiple of our
     * constituent constructors require it. We should be able to make upstream PRs to fix these. */
    R: Read,
{
    match compression_method {
        &CompressionMethod::Stored => Ok(EntryReader::Stored(reader)),
        #[cfg(feature = "_deflate-any")]
        &CompressionMethod::Deflated => {
            let deflate_reader = DeflateDecoder::new(reader);
            Ok(EntryReader::Deflated(deflate_reader))
        }
        #[cfg(feature = "deflate64")]
        &CompressionMethod::Deflate64 => {
            let deflate64_reader = Deflate64Decoder::new(reader);
            Ok(EntryReader::Deflate64(deflate64_reader))
        }
        #[cfg(feature = "bzip2")]
        &CompressionMethod::Bzip2 => {
            let bzip2_reader = BzDecoder::new(reader);
            Ok(EntryReader::Bzip2(bzip2_reader))
        }
        #[cfg(feature = "zstd")]
        &CompressionMethod::Zstd => {
            let zstd_reader = ZstdDecoder::new(reader).unwrap();
            Ok(EntryReader::Zstd(zstd_reader))
        }
        #[cfg(feature = "lzma")]
        &CompressionMethod::Lzma => {
            let buf_reader = io::BufReader::new(reader);
            let lzma_reader = LzmaDecoder::new(buf_reader);
            Ok(EntryReader::Lzma(Box::new(lzma_reader)))
        }
        #[cfg(feature = "xz")]
        &CompressionMethod::Xz => {
            let buf_reader = io::BufReader::new(reader);
            let xz_reader = XzDecoder::new(buf_reader);
            Ok(EntryReader::Xz(xz_reader))
        }
        /* TODO: make this into its own EntryReadError error type! */
        _ => Err(ZipError::UnsupportedArchive(
            "Compression method not supported",
        )),
    }
}

pub(crate) enum CryptoReader<R> {
    ZipCrypto(ZipCryptoReaderValid<R>),
    #[cfg(feature = "aes-crypto")]
    Aes(AesReaderValid<R>),
}

impl<R> Read for CryptoReader<R>
where
    R: Read,
{
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            CryptoReader::ZipCrypto(r) => r.read(buf),
            #[cfg(feature = "aes-crypto")]
            CryptoReader::Aes(r) => r.read(buf),
        }
    }
}

pub(crate) enum CryptoKeyValidationSource {
    Crc32(u32),
    DateTime(DateTime),
    #[cfg(feature = "aes-crypto")]
    Aes {
        info: AesModeInfo,
        compressed_size: u64,
    },
}

impl CryptoKeyValidationSource {
    pub fn from_data(data: &ZipFileData) -> Result<Self, ZipError> {
        debug_assert!(
            data.encrypted,
            "should never enter this method except on encrypted entries"
        );

        #[allow(deprecated)]
        if let CompressionMethod::Unsupported(_) = data.compression_method {
            /* TODO: make this into its own EntryReadError error type! */
            return Err(ZipError::UnsupportedArchive(
                "Compression method not supported",
            ));
        }

        #[allow(unused_variables)]
        if let Some(info) = data.aes_mode {
            #[cfg(feature = "aes-crypto")]
            return Ok(Self::Aes {
                info,
                compressed_size: data.compressed_size,
            });
            /* TODO: make this into its own EntryReadError error type! */
            #[cfg(not(feature = "aes-crypto"))]
            return Err(ZipError::UnsupportedArchive(
                "AES encrypted files cannot be decrypted without the aes-crypto feature.",
            ));
        }

        if let Some(last_modified_time) = data.last_modified_time {
            /* TODO: use let chains once stabilized! */
            if data.using_data_descriptor {
                return Ok(Self::DateTime(last_modified_time));
            }
        }

        Ok(Self::Crc32(data.crc32))
    }

    /// Returns `true` if the data is encrypted using AE2.
    pub const fn is_ae2_encrypted(&self) -> bool {
        match self {
            #[cfg(feature = "aes-crypto")]
            Self::Aes {
                info:
                    AesModeInfo {
                        vendor_version: AesVendorVersion::Ae2,
                        ..
                    },
                ..
            } => true,
            _ => false,
        }
    }

    pub fn make_crypto_reader<R>(
        self,
        password: &[u8],
        reader: R,
    ) -> Result<CryptoReader<R>, ZipError>
    where
        R: Read,
    {
        match self {
            #[cfg(feature = "aes-crypto")]
            Self::Aes {
                info: AesModeInfo { aes_mode, .. },
                compressed_size,
            } => {
                let aes_reader =
                    AesReader::new(reader, aes_mode, compressed_size).validate(password)?;
                Ok(CryptoReader::Aes(aes_reader))
            }
            Self::DateTime(last_modified_time) => {
                let validator = ZipCryptoValidator::InfoZipMsdosTime(last_modified_time.timepart());
                let zc_reader = ZipCryptoReader::new(reader, password).validate(validator)?;
                Ok(CryptoReader::ZipCrypto(zc_reader))
            }
            Self::Crc32(crc32) => {
                let validator = ZipCryptoValidator::PkzipCrc32(crc32);
                let zc_reader = ZipCryptoReader::new(reader, password).validate(validator)?;
                Ok(CryptoReader::ZipCrypto(zc_reader))
            }
        }
    }
}

pub(crate) enum CryptoEntryReader<R> {
    Unencrypted(Crc32Reader<EntryReader<R>>),
    Ae2Encrypted(EntryReader<CryptoReader<R>>),
    NonAe2Encrypted(Crc32Reader<EntryReader<CryptoReader<R>>>),
}

impl<R> Read for CryptoEntryReader<R>
where
    R: Read,
{
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Unencrypted(r) => r.read(buf),
            Self::Ae2Encrypted(r) => r.read(buf),
            Self::NonAe2Encrypted(r) => r.read(buf),
        }
    }
}

pub mod streaming {
    use super::{construct_decompressing_reader, Crc32Reader, ZipError, ZipFileData, ZipResult};

    use crate::read::{
        central_header_to_zip_file_inner, parse_extra_field, EntryData, HasZipMetadata,
    };
    use crate::spec::{self, FixedSizeBlock};
    use crate::types::{ZipCentralEntryBlock, ZipLocalEntryBlock};

    use std::io::{self, Read};
    use std::mem;
    use std::ops;

    pub struct StreamingArchive<R> {
        reader: R,
        remaining_before_next_entry: u64,
        first_metadata_block: Option<[u8; mem::size_of::<ZipLocalEntryBlock>()]>,
    }

    impl<R> StreamingArchive<R> {
        pub const fn new(reader: R) -> Self {
            Self {
                reader,
                remaining_before_next_entry: 0,
                first_metadata_block: None,
            }
        }

        pub fn into_inner(self) -> R {
            let Self { reader, .. } = self;
            reader
        }
    }

    impl<R> StreamingArchive<R>
    where
        R: Read,
    {
        fn drain_remaining(&mut self) -> Result<(), ZipError> {
            let Self {
                ref mut reader,
                ref mut remaining_before_next_entry,
                ..
            } = self;
            if *remaining_before_next_entry > 0 {
                io::copy(
                    &mut reader.by_ref().take(*remaining_before_next_entry),
                    &mut io::sink(),
                )?;
                *remaining_before_next_entry = 0;
            }
            Ok(())
        }

        pub fn next_entry(&mut self) -> ZipResult<Option<StreamingZipEntry<impl Read + '_>>> {
            // We can't use the typical ::parse() method, as we follow separate code paths depending
            // on the "magic" value (since the magic value will be from the central directory header
            // if we've finished iterating over all the actual files).
            self.drain_remaining()?;
            let Self {
                ref mut reader,
                ref mut remaining_before_next_entry,
                ref mut first_metadata_block,
            } = self;
            assert_eq!(0, *remaining_before_next_entry);

            let mut block = [0u8; mem::size_of::<ZipLocalEntryBlock>()];
            reader.read_exact(&mut block)?;

            let signature = spec::Magic::from_first_le_bytes(block.as_ref());
            match signature {
                ZipLocalEntryBlock::MAGIC => (),
                /* If the signature corresponds to the first central directory entry, then we are
                 * out of file entries. We can't seek backwards in a stream, so we save the block
                 * we just read to our mutable state. */
                ZipCentralEntryBlock::MAGIC => {
                    assert!(
                        first_metadata_block.is_none(),
                        "metadata block should never be set except exactly once"
                    );
                    assert!(
                        mem::size_of::<ZipLocalEntryBlock>()
                            < mem::size_of::<ZipCentralEntryBlock>()
                    );
                    *first_metadata_block = Some(block);
                    return Ok(None);
                }
                _ => return Err(ZipLocalEntryBlock::WRONG_MAGIC_ERROR),
            }
            let block = ZipLocalEntryBlock::interpret(&block)?;

            let mut data = ZipFileData::from_local_block(block, reader)?;

            let stripped_extra_field = parse_extra_field(&mut data)?;
            data.extra_field = stripped_extra_field;

            let limit_reader =
                DrainWrapper::new(data.compressed_size, remaining_before_next_entry, reader);
            let entry_reader =
                construct_decompressing_reader(&data.compression_method, limit_reader)?;
            let crc32_reader = Crc32Reader::new(entry_reader, data.crc32);
            Ok(Some(StreamingZipEntry {
                data,
                reader: crc32_reader,
            }))
        }

        pub fn next_metadata_entry(&mut self) -> ZipResult<Option<ZipStreamFileMetadata>> {
            /* We should only need to drain remaining exactly once (if at all), since that's only
             * employed if the end user fails to read the entire contents of a streaming file
             * entry. */
            self.drain_remaining()?;
            let Self {
                ref mut reader,
                ref remaining_before_next_entry,
                ref mut first_metadata_block,
            } = self;
            assert_eq!(0, *remaining_before_next_entry);

            /* Get the bytes out of the stream necessary to create a parseable central block. */
            let block: [u8; mem::size_of::<ZipCentralEntryBlock>()] =
                match first_metadata_block.take() {
                    /* If we have a block we tried to parse earlier from .next_entry(), get the
                     * data from there, then read the additional bytes necessary to construct
                     * a central directory entry. This should always happen exactly once. */
                    Some(block) => {
                        assert!(
                            mem::size_of::<ZipLocalEntryBlock>()
                                < mem::size_of::<ZipCentralEntryBlock>()
                        );
                        assert_eq!(block.len(), mem::size_of::<ZipLocalEntryBlock>());

                        let mut remaining_block = [0u8; mem::size_of::<ZipCentralEntryBlock>()
                            - mem::size_of::<ZipLocalEntryBlock>()];
                        reader.read_exact(remaining_block.as_mut())?;

                        let mut joined_block = [0u8; mem::size_of::<ZipCentralEntryBlock>()];
                        joined_block[..block.len()].copy_from_slice(&block);
                        joined_block[block.len()..].copy_from_slice(&remaining_block);
                        joined_block
                    }
                    /* After the first central block is parsed, we should always go into this
                     * branch, reading the necessary bytes from the stream. */
                    None => {
                        let mut block = [0u8; mem::size_of::<ZipCentralEntryBlock>()];
                        match reader.read_exact(&mut block) {
                            Ok(()) => (),
                            /* The reader is done! This is expected to happen exactly once when the
                             * stream is completely finished. */
                            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
                            Err(e) => return Err(e.into()),
                        };
                        block
                    }
                };
            // Parse central header
            let block = ZipCentralEntryBlock::interpret(&block)?;

            // Give archive_offset and central_header_start dummy value 0, since
            // they are not used in the output.
            let archive_offset = 0;
            let central_header_start = 0;

            let data = central_header_to_zip_file_inner(
                reader,
                archive_offset,
                central_header_start,
                block,
            )?;
            Ok(Some(ZipStreamFileMetadata(data)))
        }
    }

    struct DrainWrapper<'a, R> {
        full_extent: usize,
        current_progress: usize,
        remaining_to_notify: &'a mut u64,
        inner: R,
    }

    impl<'a, R> DrainWrapper<'a, R> {
        pub fn new(extent: u64, remaining_to_notify: &'a mut u64, inner: R) -> Self {
            Self {
                full_extent: extent.try_into().unwrap(),
                current_progress: 0,
                remaining_to_notify,
                inner,
            }
        }

        fn remaining(&self) -> usize {
            debug_assert!(self.current_progress <= self.full_extent);
            self.full_extent - self.current_progress
        }
    }

    impl<'a, R> Read for DrainWrapper<'a, R>
    where
        R: Read,
    {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            assert!(!buf.is_empty());
            let to_read = self.remaining().min(buf.len());
            /* If the input is exhausted, or `buf` was empty, we are done. */
            if to_read == 0 {
                return Ok(0);
            }

            let count = self.inner.read(&mut buf[..to_read])?;
            if count == 0 {
                /* `to_read` was >0, so this was unexpected: */
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "failed to read expected number of bytes for zip entry from stream",
                ));
            }

            debug_assert!(count <= to_read);
            self.current_progress += count;
            Ok(count)
        }
    }

    impl<'a, R> ops::Drop for DrainWrapper<'a, R> {
        fn drop(&mut self) {
            assert_eq!(
                0, *self.remaining_to_notify,
                "remaining should always be zero before drop is called"
            );
            *self.remaining_to_notify = self.remaining().try_into().unwrap();
        }
    }

    /// A struct for reading a zip file from a stream.
    pub struct StreamingZipEntry<R> {
        pub(crate) data: ZipFileData,
        pub(crate) reader: R,
    }

    impl<R> Read for StreamingZipEntry<R>
    where
        R: Read,
    {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.reader.read(buf)
        }
    }

    impl<R> HasZipMetadata for StreamingZipEntry<R> {
        fn get_metadata(&self) -> &ZipFileData {
            &self.data
        }
    }

    impl<R> EntryData for StreamingZipEntry<R> {}

    /// Additional metadata for the file.
    #[derive(Debug)]
    pub struct ZipStreamFileMetadata(ZipFileData);

    impl HasZipMetadata for ZipStreamFileMetadata {
        fn get_metadata(&self) -> &ZipFileData {
            let Self(data) = self;
            data
        }
    }

    impl EntryData for ZipStreamFileMetadata {}
}
