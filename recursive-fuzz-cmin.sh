#!/bin/bash
cp -r "fuzz/corpus/fuzz_$1" "fuzz/corpus/fuzz_$1_iter_0"
i=0
while true; do
  j=$((i + 1))
  cp -r "fuzz/corpus/fuzz_$1_iter_${i}" "fuzz/corpus/fuzz_$1_iter_${i}.bak"
  mkdir "fuzz/corpus/fuzz_$1_iter_${j}"
  cargo fuzz cmin --all-features "fuzz_$1" "fuzz/corpus/fuzz_$1_iter_${i}" -- \
    -dict=fuzz/fuzz.dict -max_len=70000 "fuzz/corpus/fuzz_$1_iter_${j}"
  i=$j
done
