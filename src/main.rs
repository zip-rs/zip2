use std::env;
use std::fs::{create_dir, File};
use std::io::Write;

use anyhow::Result;
use opendal::services::Ghac;
use opendal::Operator;

#[tokio::main]
async fn main() -> Result<()> {
    let cache_dir = env::current_dir()?.join("cache");
    create_dir(&cache_dir)?;
    // Create ghac backend builder.
    let builder = Ghac::default()
        .root(cache_dir.canonicalize()?.to_str().unwrap())
        .version("6088e45028fd0140424d7b9ae4d1aafdbac9925f0411980db21f15a236bef626");
    let op: Operator = Operator::new(builder)?.finish();
    let dl = op.read("semver-cargo_semver-93c4e40f0b737772-linux-rustc-1.90.0-(1159e78c4-2025-09-14)-cargo-semver-checks-0.44.0-d41d8cd98f00b204e9800998ecf8427e-semver-checks-rustdoc-4854acc0120975c17defd9b1c649fb889256a863").await?;
    let out = cache_dir.join("cache");
    let mut file = File::create(out)?;
    file.write_all(&dl.to_bytes())?;
    Ok(())
}
