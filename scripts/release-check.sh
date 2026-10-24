#!/usr/bin/env bash
# Fails if a release folder carries settings or user data instead of a clean install.
set -euo pipefail
dir="${1:?usage: release-check.sh <folder>}"
found=$(find "$dir" \( -iname 'slam.toml' -o -iname 'portable' -o -iname '*.slamcfg' \
    -o -iname 'replays' -o -iname 'profiles' -o -iname 'library' -o -iname '*.osr' \) -print)
if [ -n "$found" ]; then
    echo "release folder $dir must not contain settings or user data:" >&2
    echo "$found" >&2
    exit 1
fi
echo "release folder $dir is clean"
