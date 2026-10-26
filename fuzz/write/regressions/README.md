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
- `merge-sub-writer-comment-shrink-junk.bin`: found 2026-10-25 (PR #1175 persistent-mode AFL). A
  sub-writer reopened after its comment shrank pads its archive with junk, which `merge_archive` copies
  into the outer archive; the harness only marked the top-level writer as non-streamable for this.
  Harness-only fix: treat `SetArchiveComment` inside a merged sub-writer as non-streamable.
- `deep-copy-keeps-stale-unicode-path.bin`: found 2026-10-25 (PR #1175 persistent-mode AFL,
  --no-default-features). `deep_copy_file`/`shallow_copy_file` kept the source entry's Info-ZIP Unicode
  Path extra field (0x7075); its CRC matched the copy's new name (`""`, CRC 0), so on read-back the copy
  was renamed to the source's name, collided with it and was lost, and a later `abort_file` hit
  `FileNotFound`. Fixed by dropping such a field from the copy.
- `abort-stale-tail-completes-comment-eocd.bin`: found 2026-10-25 (PR #1175 persistent-mode AFL,
  --no-default-features). The comment ended with the first bytes of an EOCD record; after `abort_file`
  rewound, `finalize` zero-filled the stale tail after it, which completed that record into a valid empty
  EOCD that `new_append` picked over the real one. Fixed by filling such a tail with 0xFF (any EOCD so
  completed claims a comment of >= 0xFF00 bytes, which cannot fit), and moving the footer instead when
  the tail is that long.
- `finish-into-readable-comment-signature.bin`, `reopen-nested-copy-comment-signature.bin`: found
  2026-10-25 (PR #1175 persistent-mode AFL). The archive comment contains a complete EOCD record
  followed by more bytes. Like Python's `zipfile` and Info-ZIP, `ZipArchive` picks that inner record
  (GHSA-wjr6-mhvj-cjvm), so reopening via `finish_into_readable` (or a later `finish` inside a nested
  operation) gave a different comment or "Invalid Central Directory header". Such an archive is
  ambiguous by design, so the harness now tolerates a failed or changed reopen once the comment holds a
  signature, for both reopen methods.
- `stack-overflow-nested-copy-arbitrary.bin`: found 2026-10-25 (PR #1175 persistent-mode AFL,
  --no-default-features). An 87 KB input decoded to copies/merges nested thousands deep; the derived
  `Arbitrary` (and `Debug`/`Drop`) recursed once per level and overflowed the stack. Harness-only fix:
  cap the nesting depth at 64 while decoding.
