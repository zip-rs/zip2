#!/bin/bash
set -euxo pipefail
mkdir "fuzz/corpus/fuzz_$1_recombination_sources" || true

# Ensure the 0-byte, 1-byte and 2-byte strings won't gain duplicates during recombination
find "fuzz/corpus/fuzz_$1_recombination_sources" -type f -size -3c -delete

for size in "${@:2}"; do
  echo "$(date): STARTING ON SIZE $size"
  rm -rf "fuzz/corpus/fuzz_$1_pre_fresh_blood" || true
  find "fuzz/corpus/fuzz_$1" -type f -exec mv '{}' "fuzz/corpus/fuzz_$1_recombination_sources" ';' || true
  ./build-fuzz-corpus-multiple-restarts.sh "$1" "$size"
  find "fuzz/corpus/fuzz_$1_recombination_sources" -type f -size "-$((size + 1))c" -exec mv '{}' "fuzz/corpus/fuzz_$1" ';'
  ./fuzz-until-converged.sh "$1" "$size"
done
echo "$(date): FINISHED"