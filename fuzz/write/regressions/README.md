# fuzz_write regression inputs

Replayed by `fuzz/replay.sh` in the `fuzz_regressions` CI job. Each must finish in <10 s without a signal.

- `finish-4gib-after-abort-and-comment.bin`: found 2026-10-19 (CI run 39289509472, --no-default-features).
  On 165b0817..9fabdc85 it made `finish()` emit a ~4 GiB archive (abort_file seeks to a bogus header_start
  > EOF after new_append). Masked from f8a0fdb6 (stricter EOCD fallback), fixed by #1107 (abort_file
  rewind guard). Kept as a seed; no longer reproduces on master.
- `stack-overflow-deep-copy.bin`: found 2026-10-20 (run 39365893419). SIGABRT on pre-#1095 harness;
  no longer reproduces on master. Kept as a seed.
- `merge-duplicate-filename.bin`: found 2026-10-23 (PR #1136, --no-default-features). `merge_archive`
  silently replaced an existing entry with the same name (`IndexMap::extend`), so the harness's file
  count drifted and a later `abort_file` hit `FileNotFound`. Fixed by rejecting duplicate names in
  `merge_archive`.
- `finish-stale-tail-shorter-than-comment.bin`: found 2026-10-24 (run 40062524047, --no-default-features).
  After `abort_file` rewound, `finalize` only moved the footer when the stale tail was longer than
  the new comment; a shorter tail was left as is, so the old EOCD in it stayed last and `new_append`
  failed with "Variable-length field extends beyond file boundary". Fixed by zeroing such a tail.
- `reopen-unicode-comment-extra-field-grows.bin`: found 2026-10-24 (run 40076154738). A file's
  Info-ZIP Unicode Comment extra field (0x6375) matched the empty stored comment, so the reader
  took its UTF-8 content as the comment, but the entry was written back without the language
  encoding flag. Each reopen decoded the UTF-8 bytes as CP437 and grew the comment until its length
  overflowed `u16` (`TryFromIntError(PosOverflow)` in `write_central_directory_header`). Fixed by
  setting the flag for a non-ASCII comment when the name bytes are valid UTF-8.
