# fuzz_write regression inputs

Replayed by `fuzz/replay.sh` in the `fuzz_regressions` CI job. Each must finish in <10 s without a signal.

- `finish-4gib-after-abort-and-comment.bin`: found 2026-10-19 (CI run 39289509472, --no-default-features).
  On 165b0817..9fabdc85 it made `finish()` emit a ~4 GiB archive (abort_file seeks to a bogus header_start
  > EOF after new_append). Masked from f8a0fdb6 (stricter EOCD fallback), fixed by #1107 (abort_file
  rewind guard). Kept as a seed; no longer reproduces on master.
- `stack-overflow-deep-copy.bin`: found 2026-10-20 (run 39365893419). SIGABRT on pre-#1095 harness;
  no longer reproduces on master. Kept as a seed.
