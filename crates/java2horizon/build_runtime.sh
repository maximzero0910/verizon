#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$ROOT/../.." && pwd)"
MINECRAFT_DIR="${HORIZON_MINECRAFT_DIR:-$REPO/.local/minecraft/26.3}"
FABRIC_DIR="${HORIZON_FABRIC_LIBS_DIR:-$REPO/.local/j2h-compat/fabric-libraries}"
OUTPUT="${1:-$ROOT/test_resources/j2h-runtime.jar}"
FABRIC_API_JAR="${HORIZON_FABRIC_API_JAR:-$REPO/.local/j2h-compat/artifacts/fabric-api-0.160.6+26.3.jar}"
TMP="$(mktemp -d "${TMPDIR:-/tmp}/j2h-runtime.XXXXXX")"
trap 'rm -rf "$TMP"' EXIT

CLIENT_JAR="$MINECRAFT_DIR/jars/client.jar"
if [[ ! -f "$CLIENT_JAR" ]]; then
    echo "Minecraft 26.3 client jar not found: $CLIENT_JAR" >&2
    exit 1
fi
if [[ ! -d "$FABRIC_DIR" ]]; then
    echo "Fabric Loader libraries not found: $FABRIC_DIR" >&2
    exit 1
fi
if [[ ! -f "$FABRIC_API_JAR" ]]; then
    echo "Pinned Fabric API aggregate jar not found: $FABRIC_API_JAR" >&2
    echo "Fetch it with tools/j2h_compat.py fetch fabric-api" >&2
    exit 1
fi

mapfile -d '' LIBRARIES < <(find "$MINECRAFT_DIR/libraries" "$FABRIC_DIR" -type f -name '*.jar' -print0)
mkdir -p "$TMP/agent-classes" "$TMP/classes" "$TMP/fabric-api"
(cd "$TMP/fabric-api" && jar --extract --file "$FABRIC_API_JAR" META-INF/jars)
CLASSPATH="$CLIENT_JAR:$TMP/agent-classes"
for library in "${LIBRARIES[@]}"; do
    CLASSPATH="$CLASSPATH:$library"
done
while IFS= read -r -d '' nested; do
    CLASSPATH="$CLASSPATH:$nested"
done < <(find "$TMP/fabric-api/META-INF/jars" -type f -name '*.jar' -print0)

mkdir -p "$(dirname "$OUTPUT")"
javac --release 17 -classpath "$CLASSPATH" -d "$TMP/agent-classes" \
    "$ROOT/bridge-java/dev/horizon/j2h/AccessWidenerAgent.java"
mapfile -d '' SOURCES < <(find "$ROOT/bridge-java" -type f -name '*.java' \
    ! -name 'AccessWidenerAgent.java' -print0 | sort -z)
javac --release 17 -classpath "$CLASSPATH" -d "$TMP/classes" "${SOURCES[@]}"
jar --create --file "$OUTPUT" -C "$TMP/classes" .
echo "Built $OUTPUT"
