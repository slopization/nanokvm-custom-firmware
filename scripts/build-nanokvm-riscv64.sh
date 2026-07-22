#!/usr/bin/env bash
set -euo pipefail

TARGET="${TARGET:-riscv64gc-unknown-linux-gnu}"
PROFILE="${PROFILE:-release}"
BIN="${BIN:-nanokvm-host}"
PACKAGE="${PACKAGE:-nanokvm-host}"

cargo build \
  --package "$PACKAGE" \
  --bin "$BIN" \
  --target "$TARGET" \
  --profile "$PROFILE"

artifact="target/$TARGET/$PROFILE/$BIN"

if command -v riscv64-linux-gnu-strip >/dev/null 2>&1; then
  riscv64-linux-gnu-strip "$artifact"
fi

file "$artifact"
ls -lh "$artifact"
