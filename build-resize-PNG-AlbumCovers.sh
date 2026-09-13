#!/usr/bin/env bash
set -Eeuo pipefail

TARGET="${1:-.}"

if [[ ! -d "$TARGET" ]]; then
    echo "ERROR: Folder not found: $TARGET"
    exit 1
fi

find "$TARGET" -type f -iname '*.png' -print0 | while IFS= read -r -d '' f; do
    echo "Resizing: $f"
    magick "$f" -resize '1024x768>' "$f"
done

echo "Done."
