use std::{
    fs,
    io::{self, Cursor, IsTerminal, Seek, Write},
    mem,
    path::Path,
};

use zip::{
    unstable::path_to_string,
    write::{SimpleFileOptions, ZipWriter},
    CompressionMethod, ZIP64_BYTES_THR,
};

use crate::{args::info::*, CommandError, OutputHandle, WrapCommandErr};

pub fn execute_info(mut err: impl Write, args: Info) -> Result<(), CommandError> {
    todo!()
}
