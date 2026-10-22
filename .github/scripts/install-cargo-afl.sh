#!/usr/bin/env bash
set -euo pipefail

version="$(cargo metadata --no-deps --format-version 1 --manifest-path tools/cargo-afl-version/Cargo.toml |
  jq -r '.packages[0].dependencies[] | select(.name == "cargo-afl") | .req | sub("^="; "")')"

if [[ -z "$version" || "$version" == "null" ]]; then
  echo "::error::Could not read the pinned cargo-afl version"
  exit 1
fi

installed_version="$(cargo-afl --version 2>/dev/null | awk '{print $NF}' || true)"
if [[ "$installed_version" != "$version" ]]; then
  # Need --force to update if an older version is present
  cargo install --locked --version "$version" --force cargo-afl
fi
