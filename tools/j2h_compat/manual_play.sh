#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
PROFILE="${J2H_MANUAL_PROFILE:-$ROOT/.local/j2h-compat/manual-rei-26.3}"
GAME="$PROFILE/game"
MODS="$GAME/mods"
RESOURCES="$ROOT/.local/horizon-client/26.3"
CLIENT="${J2H_COMPAT_CLIENT:-$ROOT/target/fast/horizon-client}"

if [[ ! -f "$RESOURCES/horizon-resources.json" ]]; then
    echo "Prepared Minecraft 26.3 resources not found at: $RESOURCES" >&2
    exit 1
fi
if [[ ! -x "$CLIENT" ]]; then
    echo "Horizon client binary not found or not executable: $CLIENT" >&2
    echo "Build it with: cargo build --profile fast -p horizon-client" >&2
    exit 1
fi

mkdir -p "$MODS"
# Resolve dependencies and verify every artifact against the committed catalog
# before copying the pinned Mod Menu + REI test profile into its private game dir.
python3 - "$ROOT" "$MODS" <<'PY'
import shutil
import sys
from pathlib import Path

root = Path(sys.argv[1])
destination = Path(sys.argv[2])
sys.path.insert(0, str(root / "tools"))
import j2h_compat

entries = j2h_compat.resolve_mods(["modmenu", "roughlyenoughitems"])
expected = set()
for entry in entries:
    source = j2h_compat.artifact_path(entry)
    if not source.is_file():
        raise SystemExit(
            f"Pinned {entry['mod_id']} {entry['version']} is missing: {source}\n"
            "Acquire the catalog artifacts with:\n"
            "  python3 tools/j2h_compat.py fetch modmenu roughlyenoughitems"
        )
    j2h_compat.verify_artifact(entry, source)
    j2h_compat.verify_metadata(entry, source)
    target = destination / entry["filename"]
    shutil.copy2(source, target)
    expected.add(target.name)

unexpected = sorted(path.name for path in destination.glob("*.jar") if path.name not in expected)
if unexpected:
    raise SystemExit(
        "Manual REI profile contains unpinned JARs; move them out before launch:\n  "
        + "\n  ".join(unexpected)
    )
print("Verified pinned mods:")
for name in sorted(expected):
    print(f"  {name}")
PY

mkdir -p "$PROFILE/home" "$PROFILE/data" "$PROFILE/config" "$PROFILE/runtime" \
    "$PROFILE/launcher-home" "$PROFILE/launcher-trash" "$PROFILE/clipboard" \
    "$PROFILE/desktop" "$GAME"
chmod 700 "$PROFILE/runtime"

cat <<EOF
Launching the isolated REI test profile.
Game directory: $GAME
Logs:           $PROFILE/home/logs and $GAME/logs

In game:
  M opens Mod Menu. Search for Roughly Enough Items and open Configure to test
  its settings screen. In the creative inventory, hover an item and press R to
  open REI's recipe view; press U for its usage view. Escape closes the view.
EOF

cd "$GAME"
exec env -u WAYLAND_DISPLAY -u HORIZON_NO_JAVA_MODS \
    HORIZON_HOME="$PROFILE/home" \
    HORIZON_LAUNCHER_HOME="$PROFILE/launcher-home" \
    HORIZON_LAUNCHER_TRASH="$PROFILE/launcher-trash" \
    HORIZON_LAUNCHER_CLIPBOARD="$PROFILE/clipboard" \
    HORIZON_LAUNCHER_DESKTOP="$PROFILE/desktop" \
    HORIZON_LAUNCHER_NO_SELF_UPDATE=1 \
    HORIZON_NO_AUDIO=1 \
    HORIZON_FABRIC_LIBS_DIR="$ROOT/.local/j2h-compat/fabric-libraries" \
    HORIZON_JAVA_MODS_DIR="$MODS" \
    XDG_DATA_HOME="$PROFILE/data" \
    XDG_CONFIG_HOME="$PROFILE/config" \
    XDG_RUNTIME_DIR="$PROFILE/runtime" \
    "$CLIENT" \
        --resources "$RESOURCES" \
        --game-dir "$GAME" \
        --world J2H-Manual-REI \
        --creative \
        --render-distance 4
