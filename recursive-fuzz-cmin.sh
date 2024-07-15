#!/bin/bash
set -euxo pipefail
i=0
rm -rf "fuzz/corpus/fuzz_$1_iter_0" || true
mv "fuzz/corpus/fuzz_$1" "fuzz/corpus/fuzz_$1_iter_0"
mkdir "fuzz/corpus/fuzz_$1"
while true; do
  j=$((i + 1))
  rm -rf "fuzz/corpus/fuzz_$1_iter_${i}.bak" || true
  cp -r "fuzz/corpus/fuzz_$1_iter_${i}" "fuzz/corpus/fuzz_$1_iter_${i}.bak"
  rm -rf "fuzz/corpus/fuzz_$1_iter_${j}" || true
  mkdir "fuzz/corpus/fuzz_$1_iter_${j}"
  cargo fuzz cmin --all-features "fuzz_$1" "fuzz/corpus/fuzz_$1_iter_${i}" -- \
    -dict=fuzz/fuzz.dict -max_len="$2" -rss_limit_mb=8192 -timeout=30 "fuzz/corpus/fuzz_$1_iter_${j}"
  if diff "fuzz/corpus/fuzz_$1_iter_${i}.bak" "fuzz/corpus/fuzz_$1_iter_${j}"; then
    # Last iteration made no difference, so we're done
    rm -rf "fuzz/corpus/fuzz_$1"
    mv "fuzz/corpus/fuzz_$1_iter_${j}" "fuzz/corpus/fuzz_$1"
    rm -rf "fuzz/corpus/fuzz_$1_iter_${i}.bak"
    exit 0
  fi
  i=$j
done
