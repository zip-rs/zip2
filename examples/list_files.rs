//! List the entries of a ZIP archive without building an in-memory index.
//!
//! Usage:
//!
//! ```sh
//! cargo run --example list_files -- archive.zip
//! ```

use std::error::Error;

use zip::read::Config;
use zip::unstable::read::ZipIterable;

fn main() -> Result<(), Box<dyn Error>> {
    let path = std::env::args()
        .nth(1)
        .ok_or("usage: list_files <archive.zip>")?;
    let file = std::fs::File::open(&path)?;
    let mut archive = ZipIterable::try_new(file, Config::default())?;

    for entry in archive.files()? {
        let entry = entry?;
        println!("{:>12}  {}", entry.size(), entry.name()?);
    }

    Ok(())
}
