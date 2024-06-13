#!/bin/bash
i=0
find fuzz/corpus -iname "fuzz_$1_iter_*" -exec rm -r {} +
cp -r "fuzz/corpus/fuzz_$1" "fuzz/corpus/fuzz_$1_iter_0"
while true; do
  j=$((i + 1))
  cp -r "fuzz/corpus/fuzz_$1_iter_${i}" "fuzz/corpus/fuzz_$1_iter_${i}.bak"
  mkdir "fuzz/corpus/fuzz_$1_iter_${j}"
  cargo fuzz cmin --all-features "fuzz_$1" "fuzz/corpus/fuzz_$1_iter_${i}" -- \
    -dict=fuzz/fuzz.dict -max_len="$2" -rss_limit_mb=8192 "fuzz/corpus/fuzz_$1_iter_${j}"
  if diff "fuzz/corpus/fuzz_$1_iter_${i}.bak" "fuzz/corpus/fuzz_$1_iter_${j}"; then
    # Last iteration made no difference, so we're done
    rm -r "fuzz/corpus/fuzz_$1"
    mv "fuzz/corpus/fuzz_$1_iter_${j}" "fuzz/corpus/fuzz_$1"
    find fuzz/corpus -iname "fuzz_$1_iter_*" -exec rm -r {} +
    exit 0
  fi
  i=$j
done
