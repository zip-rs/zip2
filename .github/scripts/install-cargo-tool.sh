#!/usr/bin/env bash
set -euo pipefail

if [[ "$#" -lt 2 || "$#" -gt 3 ]] || [[ "$#" -eq 3 && "$3" != "--force" ]]; then
  echo "::error::Usage: install-cargo-tool.sh <package> <manifest-path> [--force]"
  exit 2
fi

package="$1"
manifest_path="$2"
force_install="${3:-}"
version="$(cargo metadata --no-deps --format-version 1 --manifest-path "$manifest_path" |
  python -c 'import json,sys; p=sys.argv[1]; d=json.load(sys.stdin); v=[x["req"] for m in d["packages"] for x in m["dependencies"] if x["name"]==p]; assert len(v)==1 and v[0].startswith("="), f"Expected exactly one exact-version pin for {p}"; print(v[0][1:])' "$package")"

installed_version="$("$package" --version 2>/dev/null | awk 'NR == 1 { print $2; exit }' || true)"
echo "Installed: $installed_version, desired: $version"
if [[ "$installed_version" != "$version" || "$force_install" == "--force" ]]; then
  cargo install --locked --version "$version" --force "$package"
fi
