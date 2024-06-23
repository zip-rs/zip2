#!/bin/bash
set -euxo pipefail
mkdir "fuzz/corpus/fuzz_$1_recombination_sources" || true
for size in "${@:2}"; do
  rm -rf "fuzz/corpus/fuzz_$1_pre_fresh_blood" || true
  mv "fuzz/corpus/fuzz_$1/*" "fuzz/corpus/fuzz_$1_recombination_sources" || true
  ./build-fuzz-corpus-multiple-restarts.sh "$1" "$size"
  find "fuzz/corpus/fuzz_$1_recombination_sources" -type -f -size "-${size}c" -exec mv '{}' "fuzz/corpus/fuzz_$1" ';'
  ./fuzz-until-converged.sh "$1" "$size"
done