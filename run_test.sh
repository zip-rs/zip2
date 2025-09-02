#!/bin/bash
echo "Running LZMA test..."
cd /workspace
cargo test --features lzma lzma 2>&1
echo "Test completed."