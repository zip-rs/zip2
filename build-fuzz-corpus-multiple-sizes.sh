#!/bin/bash
set -euxo pipefail
mkdir "fuzz/corpus/fuzz_$1_recombination_sources" || true

# Ensure all 0-byte, 1-byte and 2-byte strings are eligible for recombination
find "fuzz/corpus/fuzz_$1_recombination_sources" -type f -size -2c -delete
touch fuzz/corpus/fuzz_write_recombination_sources/empty
for i in $(seq 0 255); do
  printf "%02X" "$i" | xargs -n 1 -I '{}' sh -c 'echo {} | xxd -r -p > fuzz/corpus/fuzz_write_recombination_sources/{}'
  for j in $(seq 0 255); do
    printf "%02X%02X" "$i" "$j" | xargs -n 1 -I '{}' sh -c 'echo {} | xxd -r -p > fuzz/corpus/fuzz_write_recombination_sources/{}'
  done
done

for size in "${@:2}"; do
  echo "$(date): STARTING ON SIZE $size"
  rm -rf "fuzz/corpus/fuzz_$1_pre_fresh_blood" || true
  mv "fuzz/corpus/fuzz_$1/*" "fuzz/corpus/fuzz_$1_recombination_sources" || true
  ./build-fuzz-corpus-multiple-restarts.sh "$1" "$size"
  find "fuzz/corpus/fuzz_$1_recombination_sources" -type f -size "-${size}c" -exec mv '{}' "fuzz/corpus/fuzz_$1" ';'
  ./fuzz-until-converged.sh "$1" "$size"
done
echo "$(date): FINISHED"