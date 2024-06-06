#!/bin/bash
rm -r "fuzz/corpus/fuzz_$1_old"
updated=1
while [[ $updated ]]; do
  updated=0
  cp -r "fuzz/corpus/fuzz_$1" "fuzz/corpus/fuzz_$1_old"
  cargo fuzz run --all-features "fuzz_$1" "fuzz/corpus/fuzz_$1" -- \
    -dict=fuzz/fuzz.dict -max_len="$2" -rss_limit_mb=8192 \
    -fork="$(nproc || getconf NPROCESSORS_ONLN)" -runs=1000000
  ./recursive-fuzz-cmin.sh "$1" "$2"
  updated=$(diff "fuzz/corpus/fuzz_$1" "fuzz/corpus/fuzz_$1_old")
  rm -r "fuzz/corpus/fuzz_$1_old"
done
