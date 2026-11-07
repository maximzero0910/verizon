#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$ROOT/../.." && pwd)"
FABRIC_DIR="${HORIZON_FABRIC_LIBS_DIR:-$REPO/.local/j2h-compat/fabric-libraries}"
OUTPUT="${1:-$ROOT/test_resources/j2h-access-widener-agent.jar}"
TMP="$(mktemp -d "${TMPDIR:-/tmp}/j2h-aw-agent.XXXXXX")"
trap 'rm -rf "$TMP"' EXIT

mkdir -p "$TMP/classes" "$(dirname "$OUTPUT")"
javac --release 17 -classpath "$FABRIC_DIR/asm-9.10.1.jar" -d "$TMP/classes" \
  "$ROOT/bridge-java/dev/horizon/j2h/AccessWidenerAgent.java"
cat > "$TMP/manifest.mf" <<'EOF'
Manifest-Version: 1.0
Premain-Class: dev.horizon.j2h.AccessWidenerAgent
Can-Redefine-Classes: false
Can-Retransform-Classes: false

EOF
jar --create --file "$OUTPUT" --manifest "$TMP/manifest.mf" -C "$TMP/classes" .
echo "Built $OUTPUT"
