#!/usr/bin/env bash

set -e

SOURCE="PIC-icon.png"
OUTPUT="icons"

mkdir -p "$OUTPUT"

# Generate PNG sizes
for size in 16 24 32 48 64 128 256 512 1024; do

    magick "$SOURCE" \
        -resize "${size}x${size}" \
        "$OUTPUT/pic-${size}.png"

done

# Windows ICO
magick \
    "$OUTPUT/pic-16.png" \
    "$OUTPUT/pic-24.png" \
    "$OUTPUT/pic-32.png" \
    "$OUTPUT/pic-48.png" \
    "$OUTPUT/pic-64.png" \
    "$OUTPUT/pic-128.png" \
    "$OUTPUT/pic-256.png" \
    "$OUTPUT/pic.ico"

echo "PIC icons generated successfully!"
