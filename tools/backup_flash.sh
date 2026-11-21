#!/usr/bin/env bash
# Back up the full 8 MB flash of the ESP32-C6 in 64 KB chunks, retrying each
# chunk. A single 8 MB read-flash over jumper wires tends to abort with
# "serial data stream stopped"; chunking makes a flaky connection survivable.
#
#   tools/backup_flash.sh [PORT] [OUTPUT]
#
# The chip must already be in download mode (boot strap pin bridged to GND at
# power-up, or the `dfu` console command on this firmware).
set -uo pipefail

PORT="${1:-/dev/cu.usbserial-210}"
OUT="${2:-shelly-1pm-gen4-stock-backup.bin}"
CHUNK=65536       # 64 KB
TOTAL=8388608     # 8 MB
MAXTRY=10

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
: > "$OUT.partial"

offset=0
while [ "$offset" -lt "$TOTAL" ]; do
  part="$TMP/part.bin"
  ok=0
  for _ in $(seq 1 "$MAXTRY"); do
    rm -f "$part"
    if esptool --chip esp32c6 --port "$PORT" --before no-reset --after no-reset \
         --baud 115200 read-flash "$offset" "$CHUNK" "$part" >/dev/null 2>&1 \
       && [ -f "$part" ] && [ "$(wc -c < "$part" | tr -d ' ')" -eq "$CHUNK" ]; then
      ok=1
      break
    fi
    sleep 1
  done
  if [ "$ok" -ne 1 ]; then
    echo "FAILED at offset $offset after $MAXTRY tries" >&2
    exit 1
  fi
  cat "$part" >> "$OUT.partial"
  offset=$((offset + CHUNK))
  printf '\rprogress: %3d%%' $((offset * 100 / TOTAL))
done
echo
mv "$OUT.partial" "$OUT"
echo "saved $OUT ($(wc -c < "$OUT" | tr -d ' ') bytes)"
if command -v shasum >/dev/null; then shasum -a 256 "$OUT"; else sha256sum "$OUT"; fi
