#!/usr/bin/env bash
# Regenerate yaml-value-samples/ from a yaml-test-suite source checkout.
# Usage: ./regenerate.sh <path-to-yaml-test-suite>
set -euo pipefail

if [ $# -ne 1 ]; then
    echo "Usage: $0 <path-to-yaml-test-suite>" >&2
    exit 1
fi

SRC="$1/src"
OUT="$(dirname "$0")/yaml-value-samples"

# Test IDs excluded from the corpus because they require alias resolution,
# which yamlito does not support at the value level.
EXCLUDE_IDS="3GZX 6KGN 7BUB C4HZ CUP7 JS2J LE5A UGM3 V55R W5VH 26DV 2SXE E76Z ZWK4 9KAX CN3R HMQ5"

if [ ! -d "$SRC" ]; then
    echo "Error: $SRC does not exist" >&2
    exit 1
fi

rm -rf "$OUT"
mkdir -p "$OUT"

total=0
written=0
skipped_fail=0
skipped_no_json=0

for src_file in "$SRC"/*.yaml; do
    stem=$(basename "$src_file" .yaml)
    count=$(yq 'length' "$src_file")

    for ((i=0; i<count; i++)); do
        total=$((total + 1))

        fail=$(yq ".[$i] | has(\"fail\")" "$src_file")
        has_json=$(yq ".[$i] | has(\"json\")" "$src_file")
        has_yaml=$(yq ".[$i] | has(\"yaml\")" "$src_file")

        if [ "$fail" = "true" ]; then
            skipped_fail=$((skipped_fail + 1))
            continue
        fi

        if [ "$has_json" = "false" ] || [ "$has_yaml" = "false" ]; then
            skipped_no_json=$((skipped_no_json + 1))
            continue
        fi

        if [ "$count" -eq 1 ]; then
            out_stem="$stem"
        else
            out_stem=$(printf '%s-%02d' "$stem" "$i")
        fi

        # Skip excluded IDs
        if echo "$EXCLUDE_IDS" | grep -qw "$out_stem"; then
            continue
        fi

        # The suite stores each field as a YAML block literal, so yq appends
        # one clip newline that is not part of the field. Strip exactly that
        # one trailing newline (chomp at EOF); real trailing newlines in the
        # input are encoded with the ↵/∎ visual markers, never as raw bytes.
        yq ".[$i].yaml" "$src_file" | perl -pe 'chomp if eof' > "$OUT/${out_stem}.yaml"
        yq ".[$i].json" "$src_file" | perl -pe 'chomp if eof' > "$OUT/${out_stem}.json"

        written=$((written + 1))
    done
done

cp "$1/License" "$OUT/LICENSE"

echo "Total: $total"
echo "Written: $written"
echo "Skipped (fail): $skipped_fail"
echo "Skipped (no json/yaml): $skipped_no_json"
