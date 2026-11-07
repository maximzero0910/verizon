#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")" && pwd)"
SRC="$ROOT/test_mod/src/main/java"
OUT="$ROOT/test_resources/classes"
rm -rf "$OUT" && mkdir -p "$OUT"
javac --release 17 -d "$OUT" \
  "$SRC/com/example/testmod/NativeBridge.java" \
  "$SRC/com/example/testmod/TestMod.java" \
  "$SRC/com/example/libmod/LibMod.java"
# Package libmod jar
jar --create --file "$ROOT/test_resources/libmod.jar" \
  -C "$OUT" com/example/libmod \
  -C "$OUT" com/example/testmod/NativeBridge.class
# Package testmod jar (includes NativeBridge + TestMod)
jar --create --file "$ROOT/test_resources/testmod.jar" \
  -C "$OUT" com/example/testmod
echo "Built libmod.jar and testmod.jar"
jar tf "$ROOT/test_resources/libmod.jar"
jar tf "$ROOT/test_resources/testmod.jar"
