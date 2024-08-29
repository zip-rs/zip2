use std::{
    fmt,
    io::{self, Write},
};

use super::formats::FormatValue;
use crate::{
    args::info::{ParseableDirective, ParseableFormatComponent, ParseableFormatSpec},
    CommandError, WrapCommandErr,
};

pub trait Writeable {
    fn write_to(&self, out: &mut dyn Write) -> Result<(), io::Error>;
}

impl<S> Writeable for S
where
    S: fmt::Display,
{
    fn write_to(&self, out: &mut dyn Write) -> Result<(), io::Error> {
        write!(out, "{}", self)
    }
}

pub trait FormatDirective {
    type Data<'a>;
    type FieldType: FormatValue;
    fn extract_field<'a>(
        &self,
        data: Self::Data<'a>,
    ) -> <Self::FieldType as FormatValue>::Input<'a>;
    fn value_formatter(&self) -> Self::FieldType;

    fn format_field<'a>(
        &self,
        data: Self::Data<'a>,
    ) -> Result<<Self::FieldType as FormatValue>::Output<'a>, <Self::FieldType as FormatValue>::E>
    {
        self.value_formatter()
            .format_value(self.extract_field(data))
    }
}

/// Wrap a [`FormatDirective`] and write it to a stream. This isn't directly type-eraseable, but it
/// removes one layer of polymorphism to enable us to do that in a subsequent wrapper trait.
pub trait DirectiveFormatter {
    type Data<'a>;

    fn write_directive<'a>(
        &self,
        data: Self::Data<'a>,
        out: &mut dyn Write,
    ) -> Result<(), CommandError>;
}

impl<FD> DirectiveFormatter for FD
where
    FD: FormatDirective,
    for<'a> <<FD as FormatDirective>::FieldType as FormatValue>::Output<'a>: Writeable + fmt::Debug,
    <<FD as FormatDirective>::FieldType as FormatValue>::E: fmt::Display,
{
    type Data<'a> = <FD as FormatDirective>::Data<'a>;

    fn write_directive<'a>(
        &self,
        data: Self::Data<'a>,
        out: &mut dyn Write,
    ) -> Result<(), CommandError> {
        let output = self
            .format_field(data)
            .map_err(|e| CommandError::InvalidData(format!("error formatting field: {e}")))?;
        output
            .write_to(out)
            .wrap_err_with(|| format!("failed to write output to stream: {output:?}"))
    }
}

pub mod compiled {
    use super::*;

    enum CompiledFormatComponent<F> {
        Directive(F),
        ContiguousLiteral(String),
    }

    impl<F> CompiledFormatComponent<F>
    where
        F: DirectiveFormatter,
    {
        pub fn write_component<'a>(
            &self,
            data: <F as DirectiveFormatter>::Data<'a>,
            mut out: impl Write,
        ) -> Result<(), CommandError> {
            match self {
                Self::Directive(d) => d.write_directive(data, &mut out),
                Self::ContiguousLiteral(lit) => out
                    .write_all(lit.as_bytes())
                    .wrap_err_with(|| format!("failed to write literal {lit:?} to output")),
            }
        }
    }

    pub trait CompiledFormat {
        type Spec: ParseableDirective;
        type Fmt: DirectiveFormatter;

        fn from_directive_spec(spec: Self::Spec) -> Result<Self::Fmt, CommandError>;
    }

    pub struct CompiledFormatSpec<F> {
        components: Vec<CompiledFormatComponent<F>>,
    }

    impl<F> CompiledFormatSpec<F> {
        pub fn is_empty(&self) -> bool {
            self.components.is_empty()
        }
    }

    impl<F> CompiledFormatSpec<F>
    where
        F: DirectiveFormatter,
    {
        pub fn from_spec<CF>(
            spec: ParseableFormatSpec<<CF as CompiledFormat>::Spec>,
        ) -> Result<Self, CommandError>
        where
            CF: CompiledFormat<Fmt = F>,
        {
            let ParseableFormatSpec {
                components: spec_components,
            } = spec;

            let mut components: Vec<CompiledFormatComponent<F>> = Vec::new();
            for c in spec_components.into_iter() {
                match c {
                    ParseableFormatComponent::Directive(d) => {
                        let d = CF::from_directive_spec(d)?;
                        components.push(CompiledFormatComponent::Directive(d));
                    }
                    ParseableFormatComponent::Escaped(s) => match components.last_mut() {
                        Some(CompiledFormatComponent::ContiguousLiteral(ref mut last_lit)) => {
                            last_lit.push_str(s);
                        }
                        _ => {
                            components
                                .push(CompiledFormatComponent::ContiguousLiteral(s.to_string()));
                        }
                    },
                    ParseableFormatComponent::Literal(new_lit) => match components.last_mut() {
                        Some(CompiledFormatComponent::ContiguousLiteral(ref mut last_lit)) => {
                            last_lit.push_str(new_lit.as_str());
                        }
                        _ => {
                            components.push(CompiledFormatComponent::ContiguousLiteral(new_lit));
                        }
                    },
                }
            }

            Ok(Self { components })
        }

        pub fn execute_format<'a>(
            &self,
            data: <F as DirectiveFormatter>::Data<'a>,
            mut out: impl Write,
        ) -> Result<(), CommandError>
        where
            <F as DirectiveFormatter>::Data<'a>: Clone,
        {
            for c in self.components.iter() {
                c.write_component(data.clone(), &mut out)?
            }
            Ok(())
        }
    }
}

pub mod entry {
    use super::{
        super::formats::{
            BinaryNumericValue, BinaryStringValue, ByteSizeValue, CompressionMethodValue,
            FileTypeValue, FormatValue, NameString, OffsetValue, TimestampValue, UnixModeValue,
        },
        FormatDirective,
    };
    use crate::extract::receiver::EntryData;

    pub struct EntryNameField(pub NameString);

    impl FormatDirective for EntryNameField {
        type Data<'a> = &'a EntryData<'a>;
        type FieldType = NameString;
        fn extract_field<'a>(
            &self,
            data: Self::Data<'a>,
        ) -> <Self::FieldType as FormatValue>::Input<'a> {
            data.name
        }
        fn value_formatter(&self) -> NameString {
            self.0
        }
    }

    pub struct FileTypeField(pub FileTypeValue);

    impl FormatDirective for FileTypeField {
        type Data<'a> = &'a EntryData<'a>;
        type FieldType = FileTypeValue;
        fn extract_field<'a>(
            &self,
            data: Self::Data<'a>,
        ) -> <Self::FieldType as FormatValue>::Input<'a> {
            data.kind
        }
        fn value_formatter(&self) -> FileTypeValue {
            self.0
        }
    }

    pub struct EntryCommentField(pub BinaryStringValue);

    impl FormatDirective for EntryCommentField {
        type Data<'a> = &'a EntryData<'a>;
        type FieldType = BinaryStringValue;
        fn extract_field<'a>(
            &self,
            data: Self::Data<'a>,
        ) -> <Self::FieldType as FormatValue>::Input<'a> {
            Some(data.comment.as_bytes())
        }
        fn value_formatter(&self) -> BinaryStringValue {
            self.0
        }
    }

    pub struct LocalHeaderStartField(pub OffsetValue);

    impl FormatDirective for LocalHeaderStartField {
        type Data<'a> = &'a EntryData<'a>;
        type FieldType = OffsetValue;
        fn extract_field<'a>(
            &self,
            data: Self::Data<'a>,
        ) -> <Self::FieldType as FormatValue>::Input<'a> {
            Some(data.local_header_start)
        }
        fn value_formatter(&self) -> OffsetValue {
            self.0
        }
    }

    pub struct ContentStartField(pub OffsetValue);

    impl FormatDirective for ContentStartField {
        type Data<'a> = &'a EntryData<'a>;
        type FieldType = OffsetValue;
        fn extract_field<'a>(
            &self,
            data: Self::Data<'a>,
        ) -> <Self::FieldType as FormatValue>::Input<'a> {
            Some(data.content_start)
        }
        fn value_formatter(&self) -> OffsetValue {
            self.0
        }
    }

    pub struct UncompressedSizeField(pub ByteSizeValue);

    impl FormatDirective for UncompressedSizeField {
        type Data<'a> = &'a EntryData<'a>;
        type FieldType = ByteSizeValue;
        fn extract_field<'a>(
            &self,
            data: Self::Data<'a>,
        ) -> <Self::FieldType as FormatValue>::Input<'a> {
            data.uncompressed_size
        }
        fn value_formatter(&self) -> ByteSizeValue {
            self.0
        }
    }

    pub struct CompressedSizeField(pub ByteSizeValue);

    impl FormatDirective for CompressedSizeField {
        type Data<'a> = &'a EntryData<'a>;
        type FieldType = ByteSizeValue;
        fn extract_field<'a>(
            &self,
            data: Self::Data<'a>,
        ) -> <Self::FieldType as FormatValue>::Input<'a> {
            data.compressed_size
        }
        fn value_formatter(&self) -> ByteSizeValue {
            self.0
        }
    }

    pub struct ContentEndField(pub OffsetValue);

    impl FormatDirective for ContentEndField {
        type Data<'a> = &'a EntryData<'a>;
        type FieldType = OffsetValue;
        fn extract_field<'a>(
            &self,
            data: Self::Data<'a>,
        ) -> <Self::FieldType as FormatValue>::Input<'a> {
            Some(data.content_end())
        }
        fn value_formatter(&self) -> OffsetValue {
            self.0
        }
    }

    pub struct CentralHeaderStartField(pub OffsetValue);

    impl FormatDirective for CentralHeaderStartField {
        type Data<'a> = &'a EntryData<'a>;
        type FieldType = OffsetValue;
        fn extract_field<'a>(
            &self,
            data: Self::Data<'a>,
        ) -> <Self::FieldType as FormatValue>::Input<'a> {
            Some(data.central_header_start)
        }
        fn value_formatter(&self) -> OffsetValue {
            self.0
        }
    }

    pub struct CompressionMethodField(pub CompressionMethodValue);

    impl FormatDirective for CompressionMethodField {
        type Data<'a> = &'a EntryData<'a>;
        type FieldType = CompressionMethodValue;
        fn extract_field<'a>(
            &self,
            data: Self::Data<'a>,
        ) -> <Self::FieldType as FormatValue>::Input<'a> {
            data.compression
        }
        fn value_formatter(&self) -> CompressionMethodValue {
            self.0
        }
    }

    pub struct UnixModeField(pub UnixModeValue);

    impl FormatDirective for UnixModeField {
        type Data<'a> = &'a EntryData<'a>;
        type FieldType = UnixModeValue;
        fn extract_field<'a>(
            &self,
            data: Self::Data<'a>,
        ) -> <Self::FieldType as FormatValue>::Input<'a> {
            data.unix_mode
        }
        fn value_formatter(&self) -> UnixModeValue {
            self.0
        }
    }

    pub struct Crc32Field(pub BinaryNumericValue);

    impl FormatDirective for Crc32Field {
        type Data<'a> = &'a EntryData<'a>;
        type FieldType = BinaryNumericValue;
        fn extract_field<'a>(
            &self,
            data: Self::Data<'a>,
        ) -> <Self::FieldType as FormatValue>::Input<'a> {
            data.crc32
        }
        fn value_formatter(&self) -> BinaryNumericValue {
            self.0
        }
    }

    pub struct TimestampField(pub TimestampValue);

    impl FormatDirective for TimestampField {
        type Data<'a> = &'a EntryData<'a>;
        type FieldType = TimestampValue;
        fn extract_field<'a>(
            &self,
            data: Self::Data<'a>,
        ) -> <Self::FieldType as FormatValue>::Input<'a> {
            data.last_modified_time
        }
        fn value_formatter(&self) -> TimestampValue {
            self.0
        }
    }

    pub mod compiled {
        use super::{
            super::{compiled::CompiledFormat, DirectiveFormatter},
            *,
        };
        use crate::{args::info::EntryFormatDirective, CommandError};

        use std::io::Write;

        /// Used for type erasure by removing the lifetime-bounded associated type.
        trait EntryDirectiveFormatter {
            fn write_entry_directive<'a>(
                &self,
                data: &EntryData<'a>,
                out: &mut dyn Write,
            ) -> Result<(), CommandError>;
        }

        impl<CF> EntryDirectiveFormatter for CF
        where
            CF: for<'a> DirectiveFormatter<Data<'a> = &'a EntryData<'a>>,
        {
            fn write_entry_directive<'a>(
                &self,
                data: &EntryData<'a>,
                out: &mut dyn Write,
            ) -> Result<(), CommandError> {
                self.write_directive(data, out)
            }
        }

        /// This re-implements the generic trait using the type-erased boxed vtable.
        pub struct CompiledEntryDirective(Box<dyn EntryDirectiveFormatter>);

        impl DirectiveFormatter for CompiledEntryDirective {
            type Data<'a> = EntryData<'a>;

            fn write_directive<'a>(
                &self,
                data: Self::Data<'a>,
                out: &mut dyn Write,
            ) -> Result<(), CommandError> {
                self.0.write_entry_directive(&data, out)
            }
        }

        pub struct CompiledEntryFormat;

        impl CompiledFormat for CompiledEntryFormat {
            type Spec = EntryFormatDirective;
            type Fmt = CompiledEntryDirective;

            fn from_directive_spec(
                spec: EntryFormatDirective,
            ) -> Result<CompiledEntryDirective, CommandError> {
                Ok(CompiledEntryDirective(match spec {
                    EntryFormatDirective::Name => Box::new(EntryNameField(NameString)),
                    EntryFormatDirective::FileType(f) => Box::new(FileTypeField(FileTypeValue(f))),
                    EntryFormatDirective::CompressedSize(f) => {
                        Box::new(CompressedSizeField(ByteSizeValue(f)))
                    }
                    EntryFormatDirective::UncompressedSize(f) => {
                        Box::new(UncompressedSizeField(ByteSizeValue(f)))
                    }
                    EntryFormatDirective::UnixMode(f) => Box::new(UnixModeField(UnixModeValue(f))),
                    EntryFormatDirective::CompressionMethod(f) => {
                        Box::new(CompressionMethodField(CompressionMethodValue(f)))
                    }
                    EntryFormatDirective::Comment(f) => {
                        Box::new(EntryCommentField(BinaryStringValue(f)))
                    }
                    EntryFormatDirective::LocalHeaderStart(f) => {
                        Box::new(LocalHeaderStartField(OffsetValue(f)))
                    }
                    EntryFormatDirective::ContentStart(f) => {
                        Box::new(ContentStartField(OffsetValue(f)))
                    }
                    EntryFormatDirective::ContentEnd(f) => {
                        Box::new(ContentEndField(OffsetValue(f)))
                    }
                    EntryFormatDirective::CentralHeaderStart(f) => {
                        Box::new(CentralHeaderStartField(OffsetValue(f)))
                    }
                    EntryFormatDirective::CrcValue(f) => {
                        Box::new(Crc32Field(BinaryNumericValue(f)))
                    }
                    EntryFormatDirective::Timestamp(f) => {
                        Box::new(TimestampField(TimestampValue(f)))
                    }
                }))
            }
        }
    }
}

pub mod archive {
    use super::{
        super::{
            formats::{
                BinaryStringValue, ByteSizeValue, DecimalNumberValue, FormatValue, OffsetValue,
                PathString,
            },
            ArchiveWithPath,
        },
        FormatDirective,
    };

    use std::path::Path;

    #[derive(Debug, Clone, PartialEq, Eq, Hash)]
    pub struct ArchiveData<'a> {
        pub path: Option<&'a Path>,
        pub stream_length: u64,
        pub num_entries: usize,
        pub comment: Option<&'a [u8]>,
        pub first_entry_start: Option<u64>,
        pub central_directory_start: Option<u64>,
    }

    impl<'a> ArchiveData<'a> {
        pub fn from_archive_with_path(zip: &'a ArchiveWithPath) -> Self {
            Self {
                path: Some(zip.path.as_path()),
                stream_length: zip.len,
                num_entries: zip.archive.len(),
                comment: Some(zip.archive.comment()),
                first_entry_start: Some(zip.archive.offset()),
                central_directory_start: Some(zip.archive.central_directory_start()),
            }
        }
    }

    pub struct ArchiveNameField(pub PathString);

    impl FormatDirective for ArchiveNameField {
        type Data<'a> = ArchiveData<'a>;
        type FieldType = PathString;
        fn extract_field<'a>(
            &self,
            data: Self::Data<'a>,
        ) -> <Self::FieldType as FormatValue>::Input<'a> {
            data.path
        }
        fn value_formatter(&self) -> PathString {
            self.0
        }
    }

    pub struct ArchiveSizeField(pub ByteSizeValue);

    impl FormatDirective for ArchiveSizeField {
        type Data<'a> = ArchiveData<'a>;
        type FieldType = ByteSizeValue;
        fn extract_field<'a>(
            &self,
            data: Self::Data<'a>,
        ) -> <Self::FieldType as FormatValue>::Input<'a> {
            data.stream_length
        }
        fn value_formatter(&self) -> ByteSizeValue {
            self.0
        }
    }

    pub struct NumEntriesField(pub DecimalNumberValue);

    impl FormatDirective for NumEntriesField {
        type Data<'a> = ArchiveData<'a>;
        type FieldType = DecimalNumberValue;
        fn extract_field<'a>(
            &self,
            data: Self::Data<'a>,
        ) -> <Self::FieldType as FormatValue>::Input<'a> {
            data.num_entries.try_into().unwrap()
        }
        fn value_formatter(&self) -> DecimalNumberValue {
            self.0
        }
    }

    pub struct ArchiveCommentField(pub BinaryStringValue);

    impl FormatDirective for ArchiveCommentField {
        type Data<'a> = ArchiveData<'a>;
        type FieldType = BinaryStringValue;
        fn extract_field<'a>(
            &self,
            data: Self::Data<'a>,
        ) -> <Self::FieldType as FormatValue>::Input<'a> {
            data.comment
        }
        fn value_formatter(&self) -> BinaryStringValue {
            self.0
        }
    }

    pub struct FirstEntryStartField(pub OffsetValue);

    impl FormatDirective for FirstEntryStartField {
        type Data<'a> = ArchiveData<'a>;
        type FieldType = OffsetValue;
        fn extract_field<'a>(
            &self,
            data: Self::Data<'a>,
        ) -> <Self::FieldType as FormatValue>::Input<'a> {
            data.first_entry_start
        }
        fn value_formatter(&self) -> OffsetValue {
            self.0
        }
    }

    pub struct CentralDirectoryStartField(pub OffsetValue);

    impl FormatDirective for CentralDirectoryStartField {
        type Data<'a> = ArchiveData<'a>;
        type FieldType = OffsetValue;
        fn extract_field<'a>(
            &self,
            data: Self::Data<'a>,
        ) -> <Self::FieldType as FormatValue>::Input<'a> {
            data.central_directory_start
        }
        fn value_formatter(&self) -> OffsetValue {
            self.0
        }
    }

    pub mod compiled {
        use super::{
            super::{compiled::CompiledFormat, DirectiveFormatter},
            *,
        };
        use crate::{args::info::ArchiveOverviewFormatDirective, CommandError};

        use std::io::Write;

        trait ArchiveDirectiveFormatter {
            fn write_archive_directive<'a>(
                &self,
                data: ArchiveData<'a>,
                out: &mut dyn Write,
            ) -> Result<(), CommandError>;
        }

        impl<CF> ArchiveDirectiveFormatter for CF
        where
            CF: for<'a> DirectiveFormatter<Data<'a> = ArchiveData<'a>>,
        {
            fn write_archive_directive<'a>(
                &self,
                data: ArchiveData<'a>,
                out: &mut dyn Write,
            ) -> Result<(), CommandError> {
                self.write_directive(data, out)
            }
        }

        pub struct CompiledArchiveDirective(Box<dyn ArchiveDirectiveFormatter>);

        impl DirectiveFormatter for CompiledArchiveDirective {
            type Data<'a> = ArchiveData<'a>;

            fn write_directive<'a>(
                &self,
                data: Self::Data<'a>,
                out: &mut dyn Write,
            ) -> Result<(), CommandError> {
                self.0.write_archive_directive(data, out)
            }
        }

        pub struct CompiledArchiveFormat;

        impl CompiledFormat for CompiledArchiveFormat {
            type Spec = ArchiveOverviewFormatDirective;
            type Fmt = CompiledArchiveDirective;

            fn from_directive_spec(
                spec: ArchiveOverviewFormatDirective,
            ) -> Result<CompiledArchiveDirective, CommandError> {
                Ok(CompiledArchiveDirective(match spec {
                    ArchiveOverviewFormatDirective::ArchiveName => {
                        Box::new(ArchiveNameField(PathString))
                    }
                    ArchiveOverviewFormatDirective::TotalSize(f) => {
                        Box::new(ArchiveSizeField(ByteSizeValue(f)))
                    }
                    ArchiveOverviewFormatDirective::NumEntries => {
                        Box::new(NumEntriesField(DecimalNumberValue))
                    }
                    ArchiveOverviewFormatDirective::ArchiveComment(f) => {
                        Box::new(ArchiveCommentField(BinaryStringValue(f)))
                    }
                    ArchiveOverviewFormatDirective::FirstEntryStart(f) => {
                        Box::new(FirstEntryStartField(OffsetValue(f)))
                    }
                    ArchiveOverviewFormatDirective::CentralDirectoryStart(f) => {
                        Box::new(CentralDirectoryStartField(OffsetValue(f)))
                    }
                }))
            }
        }
    }
}
