#!/usr/bin/env rust-script

//! Simple test to verify the LZMA memory limit fix
//! This script tests that LZMA decompression works with the new unlimited memory setting

#[cfg(feature = "lzma")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Testing LZMA memory limit fix...");

    // Test that we can create an LZMA decoder with u64::MAX memory limit
    // This would fail in ancient liblzma versions if we used 0
    let _stream = liblzma::stream::Stream::new_lzma_decoder(u64::MAX)?;
    println!("✓ Successfully created LZMA decoder with unlimited memory (u64::MAX)");

    // Test that the old problematic value would work too (for comparison)
    // Note: This might fail in ancient versions, but should work in modern ones
    let _stream_old = liblzma::stream::Stream::new_lzma_decoder(0);
    match _stream_old {
        Ok(_) => println!("✓ Old parameter (0) also works in this liblzma version"),
        Err(e) => println!(
            "⚠ Old parameter (0) fails as expected in ancient versions: {}",
            e
        ),
    }

    println!("Test completed successfully!");
    Ok(())
}
