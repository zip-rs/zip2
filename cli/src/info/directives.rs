use std::{
    fmt,
    io::{self, Write},
};

use super::formats::FormatValue;
use crate::{CommandError, WrapCommandErr};

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

pub mod entry {
    use super::{
        super::formats::{
            ByteSizeValue, CompressionMethodValue, FileTypeValue, FormatValue, NameString,
            UnixModeValue,
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

    pub struct UncompressedSizeField(pub ByteSizeValue);

    impl FormatDirective for UncompressedSizeField {
        type Data<'a> = &'a EntryData<'a>;
        type FieldType = ByteSizeValue;
        fn extract_field<'a>(
            &self,
            data: Self::Data<'a>,
        ) -> <Self::FieldType as FormatValue>::Input<'a> {
            data.size
        }
        fn value_formatter(&self) -> ByteSizeValue {
            self.0
        }
    }
}
