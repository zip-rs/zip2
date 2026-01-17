//! Shared entry point for `zip-cli` and `zip-clite`.
//!
//! The difference between the two distributions should be a matter of their selected features and
//! optimization flags, and nothing more. If the two retain a 100% compatible CLI API, users will be
//! able to select the distribution purely based upon the functionality/security they need for that
//! particular use case.

use std::{
    collections::VecDeque,
    env, ffi, fs,
    io::{self, Write},
    path, process,
};

use zip::{write::SimpleFileOptions, CompressionMethod, ZipArchive, ZipWriter};

#[repr(i32)]
enum ExitCode {
    Success = 0,
    InvalidArg = 1,
    InvalidFile = 2,
}

pub fn shared_main() -> ! {
    let mut argv: VecDeque<ffi::OsString> = env::args_os().collect();

    let this = argv
        .pop_front()
        .unwrap_or_else(|| unsafe {
            ffi::OsString::from_encoded_bytes_unchecked(b"<none>".to_vec())
        })
        .into_string()
        .unwrap();

    let cmd = match argv.pop_front() {
        None => {
            eprintln!("{this} [compress|extract|info] ...");
            process::exit(ExitCode::InvalidArg as i32)
        }
        Some(arg) if matches!(arg.as_encoded_bytes(), b"-h" | b"--help") => {
            println!("{this} [compress|extract|info] ...");
            process::exit(ExitCode::Success as i32)
        }
        Some(cmd) => cmd.into_string().unwrap(),
    };

    match cmd.as_str() {
        "compress" => compress(this, argv),
        "extract" => extract(this, argv),
        "info" => info(this, argv),
        "-h" | "--help" => {
            println!("{this} [compress|extract|info] ...");
            process::exit(ExitCode::Success as i32)
        }
        cmd => {
            eprintln!("unrecognized command name: {cmd}");
            eprintln!("{this} [compress|extract|info] ...");
            process::exit(ExitCode::InvalidArg as i32)
        }
    }
}

fn compress(this: String, mut args: VecDeque<ffi::OsString>) -> ! {
    let outfile = match args.pop_front() {
        None => {
            eprintln!("{this} compress outfile.zip");
            eprintln!("(zip entry paths over stdin)");
            process::exit(ExitCode::InvalidArg as i32)
        }
        Some(arg) if matches!(arg.as_encoded_bytes(), b"-h" | b"--help") => {
            println!("{this} compress outfile.zip");
            println!("(zip entry paths over stdin)");
            process::exit(ExitCode::Success as i32)
        }
        Some(outfile) => outfile,
    };
    if !args.is_empty() {
        /* Print an error message, but keep going. */
        eprintln!("{this} compress takes no further arguments, but got {args:?}");
    }

    let mut w = match match fs::OpenOptions::new()
        .write(true)
        .read(true)
        .create(true)
        .truncate(false)
        .open(&outfile)
    {
        Err(e) => {
            eprintln!("error opening compress output file {outfile:?}: {e}");
            process::exit(ExitCode::InvalidFile as i32)
        }
        Ok(f) => {
            if f.metadata().unwrap().len() > 0 {
                ZipWriter::new_append(f)
            } else {
                Ok(ZipWriter::new(f))
            }
        }
    } {
        Err(e) => {
            eprintln!("error creating zip writer from output file {outfile:?}: {e}");
            process::exit(ExitCode::InvalidFile as i32)
        }
        Ok(w) => w,
    };

    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let compressed = zip::cfg_if_expr! {
        #[cfg(feature = "_deflate-any")] => SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .compression_level(Some(9)),
        _ => stored,
    };
    for line in io::stdin().lines() {
        let line = line.unwrap();
        let p = path::Path::new(&line);
        if line.ends_with("/") {
            w.add_directory_from_path(p, stored).unwrap();
        } else {
            match fs::symlink_metadata(p) {
                Err(e) => {
                    /* Error for this entry, but do not exit the whole thing. */
                    eprintln!("error reading input file path {p:?}: {e}");
                    continue;
                }
                Ok(m) => {
                    if m.is_dir() {
                        w.add_directory_from_path(p, stored).unwrap();
                    } else if m.is_symlink() {
                        let target = fs::read_link(p).unwrap();
                        w.add_symlink_from_path(p, target, stored).unwrap();
                    } else {
                        assert!(m.is_file());
                        w.start_file_from_path(p, compressed).unwrap();
                        let mut f = fs::File::open(p).unwrap();
                        let _ = io::copy(&mut f, &mut w).unwrap();
                    }
                }
            }
        }
    }
    w.finish().unwrap();

    process::exit(ExitCode::Success as i32)
}

fn extract(this: String, mut args: VecDeque<ffi::OsString>) -> ! {
    match args.pop_front() {
        None => {
            eprintln!("{this} extract [single|all]");
            process::exit(ExitCode::InvalidArg as i32)
        }
        Some(arg) if matches!(arg.as_encoded_bytes(), b"-h" | b"--help") => {
            println!("{this} extract [single|all]");
            process::exit(ExitCode::Success as i32)
        }
        Some(arg) => match arg.as_encoded_bytes() {
            b"single" => extract_single(this, args),
            b"all" => extract_all(this, args),
            _ => {
                eprintln!("unrecognized subcommand {arg:?}");
                eprintln!("{this} extract [single|all]");
                process::exit(ExitCode::InvalidArg as i32)
            }
        },
    }
}

fn extract_single(this: String, mut args: VecDeque<ffi::OsString>) -> ! {
    let infile = match args.pop_front() {
        None => {
            eprintln!("{this} extract single infile.zip entry-name");
            process::exit(ExitCode::InvalidArg as i32)
        }
        Some(arg) if matches!(arg.as_encoded_bytes(), b"-h" | b"--help") => {
            println!("{this} extract single infile.zip entry-name");
            process::exit(ExitCode::Success as i32)
        }
        Some(infile) => infile,
    };
    let mut archive = ZipArchive::new(fs::File::open(&infile).unwrap()).unwrap();

    let entry_name = match args.pop_front() {
        None => {
            eprintln!("no entry-name provided");
            eprintln!("{this} extract single infile.zip entry-name");
            process::exit(ExitCode::InvalidArg as i32)
        }
        Some(entry_name) => entry_name.into_string().unwrap(),
    };

    if !args.is_empty() {
        /* Print an error message, but keep going. */
        eprintln!("{this} extract single takes no further arguments, but got {args:?}");
    }

    let mut entry = match archive.by_name(&entry_name) {
        Err(e) => {
            eprintln!("error extracting single entry {entry_name}: {e}");
            process::exit(ExitCode::InvalidFile as i32)
        }
        Ok(zf) => zf,
    };

    let mut stdout = io::stdout().lock();
    let _ = io::copy(&mut entry, &mut stdout).unwrap();
    stdout.flush().unwrap();
    process::exit(ExitCode::Success as i32)
}

fn extract_all(this: String, mut args: VecDeque<ffi::OsString>) -> ! {
    let infile = match args.pop_front() {
        None => {
            eprintln!("{this} extract all infile.zip");
            process::exit(ExitCode::InvalidArg as i32)
        }
        Some(infile) => infile,
    };
    let mut archive = ZipArchive::new(fs::File::open(&infile).unwrap()).unwrap();

    if !args.is_empty() {
        /* Print an error message, but keep going. */
        eprintln!("{this} extract all takes no further arguments, but got {args:?}");
    }

    archive.extract(".").unwrap();

    process::exit(ExitCode::Success as i32)
}

fn info(this: String, mut args: VecDeque<ffi::OsString>) -> ! {
    let infile = match args.pop_front() {
        None => {
            eprintln!("{this} info infile.zip");
            process::exit(ExitCode::InvalidArg as i32)
        }
        Some(arg) if matches!(arg.as_encoded_bytes(), b"-h" | b"--help") => {
            println!("{this} info infile.zip");
            process::exit(ExitCode::Success as i32)
        }
        Some(infile) => infile,
    };
    let archive = ZipArchive::new(fs::File::open(&infile).unwrap()).unwrap();

    if !args.is_empty() {
        /* Print an error message, but keep going. */
        eprintln!("{this} info takes no further arguments, but got {args:?}");
    }

    let mut stdout = io::stdout().lock();

    for name in archive.file_names() {
        stdout.write_fmt(format_args!("{}\n", name)).unwrap();
    }

    stdout.flush().unwrap();

    process::exit(ExitCode::Success as i32)
}
