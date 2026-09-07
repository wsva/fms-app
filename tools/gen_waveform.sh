#!/usr/bin/env bash
# Generate waveform JSON for all audio/video files in /fms_data.
# Requires: audiowaveform (https://github.com/bbc/audiowaveform)
#
# Usage: ./tools/gen_waveform.sh [/path/to/data]
#
# Each media file gets a companion <basename>.waveform.json served at
# /api/data/<path>/<basename>.waveform.json, which the browser loads
# to render the waveform canvas without real-time audio decoding.

DATA_DIR="${1:-/home/mutundweisheit/wsva/data/fms_data/listen}"
PPS=10

if ! command -v audiowaveform &>/dev/null; then
    echo "Error: audiowaveform not found"
    echo "Install: https://github.com/bbc/audiowaveform#installation"
    exit 1
fi

if [ ! -d "$DATA_DIR" ]; then
    echo "Error: data directory not found: $DATA_DIR"
    exit 1
fi

count=0
skip=0

while IFS= read -r file; do
    base="${file%.*}"
    out="${base}.waveform.json"
    if [ -f "$out" ]; then
        skip=$((skip + 1))
        continue
    fi
    printf 'Generating: %s ... ' "$file"
    if audiowaveform -i "$file" -o "$out" --pixels-per-second "$PPS" --bits 8 2>/dev/null; then
        printf 'OK\n'
        count=$((count + 1))
    else
        printf 'SKIP\n'
        rm -f "$out"
    fi
done < <(find "$DATA_DIR" -type f \( \
    -iname "*.mp3" -o -iname "*.wav" -o -iname "*.m4a" -o -iname "*.m4b" -o \
    -iname "*.ogg" -o -iname "*.flac" -o -iname "*.aac" -o \
    -iname "*.mp4" -o -iname "*.webm" -o -iname "*.mkv" \
\))

echo ""
echo "Done: $count generated, $skip already exist"
