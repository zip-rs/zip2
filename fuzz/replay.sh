#!/usr/bin/env bash
# Replay inputs against a fuzz target, deterministically, with a generous timeout.
# Usage: fuzz/replay.sh <binary> <timeout-seconds> <dir-or-file>...
# Exit 1 if any input crashes (signal / panic abort) or exceeds the timeout.
# Used both as a fast required check (seed corpus + regressions) and to
# re-check AFL "hangs" so that only *reproducible* hangs fail CI.
set -uo pipefail
bin=$1; tmo=$2; shift 2
fail=0; n=0
while IFS= read -r -d '' f; do
  n=$((n+1))
  start=$(date +%s%N)
  timeout -s KILL "$tmo" "$bin" < "$f" > /dev/null 2> /tmp/replay.err
  rc=$?
  ms=$(( ($(date +%s%N) - start) / 1000000 ))
  # Fails on timeout (SIGKILL=137) or any other non-zero exit. Both targets use afl::fuzz!, so a
  # panic aborts (SIGABRT=134) just as AFL sees it; a plain rc=101 panic is a failure too.
  if [ "$rc" -ne 0 ]; then
    echo "::error file=$f::rc=$rc after ${ms}ms"; tail -5 /tmp/replay.err; fail=1
  elif [ "$ms" -gt 1000 ]; then
    echo "::warning file=$f::slow input: ${ms}ms (rc=$rc)"
  fi
done < <(find "$@" -type f ! -name 'README*' -print0 2>/dev/null)
echo "replayed $n inputs"; exit $fail
