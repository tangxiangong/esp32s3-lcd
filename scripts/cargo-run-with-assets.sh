#!/bin/sh
set -eu

if [ "$#" -lt 1 ]; then
  echo "usage: cargo-run-with-assets.sh <elf> [probe-rs args...]" >&2
  exit 2
fi

elf="$1"
shift

target_dir="${CARGO_TARGET_DIR:-target}"
assets_bin="$target_dir/display-assets/assets.bin"
assets_base="0x800000"

if [ ! -f "$assets_bin" ]; then
  echo "display assets package not found: $assets_bin" >&2
  echo "run cargo build first so build.rs can generate it" >&2
  exit 1
fi

probe-rs download \
  --chip=esp32s3 \
  --preverify \
  --binary-format bin \
  --base-address "$assets_base" \
  "$assets_bin"

probe-rs run \
  --chip=esp32s3 \
  --preverify \
  --always-print-stacktrace \
  --no-location \
  "$elf" \
  "$@"
