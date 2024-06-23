#!/bin/bash
mkdir "fuzz/corpus/fuzz_$1_recombination_sources" || true
mv "fuzz/corpus/fuzz_$1/*" "fuzz/corpus/fuzz_$1_recombination_sources"
for size in "${@:2}"; do
  rm -rf "fuzz/corpus/fuzz_$1_pre_fresh_blood"
  ./build-fuzz-corpus-multiple-restarts.sh "$1" "$size"
  find "fuzz/corpus/fuzz_$1_pre_fresh_blood" -type -f -size "-${size}c" -exec mv '{}' "fuzz/corpus/fuzz_$1" ';'
  find "fuzz/corpus/fuzz_$1_recombination_sources" -type -f -size "-${size}c" -exec mv '{}' "fuzz/corpus/fuzz_$1" ';'
  ./fuzz-until-converged.sh "$1" "$size"
done