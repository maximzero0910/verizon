#!/usr/bin/env python3
"""Install and launch an isolated Horizon profile with pinned Java/Fabric mods.

J2H is compiled into a J2H-enabled Horizon client. This tool verifies that
client, then creates a separate profile; it does not modify another Horizon
installation or patch executable bytes in place.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
COMPAT_SCRIPT = ROOT / "tools" / "j2h_compat.py"
MARKER = ".j2h-profile.json"


def load_compat():
    spec = importlib.util.spec_from_file_location("j2h_compat", COMPAT_SCRIPT)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load compatibility catalog at {COMPAT_SCRIPT}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def load_game_lab():
    path = ROOT / "tools" / "lab.py"
    spec = importlib.util.spec_from_file_location("j2h_game_lab", path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load Horizon game environment helper at {path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def verified_client(path: Path) -> Path:
    path = path.expanduser().resolve()
    if not path.is_file():
        raise RuntimeError(f"Horizon client does not exist: {path}")
    result = subprocess.run([str(path), "--j2h-info"], capture_output=True, text=True, timeout=15)
    if result.returncode:
        raise RuntimeError(f"client rejected --j2h-info: {result.stderr.strip() or result.stdout.strip()}")
    try:
        info = json.loads(result.stdout)
    except json.JSONDecodeError as exc:
        raise RuntimeError("client is not a J2H-enabled build (missing valid --j2h-info response)") from exc
    if info.get("runtime") != "java2horizon" or info.get("minecraft_version") != "26.3":
        raise RuntimeError("client does not report the supported J2H runtime and Minecraft 26.3")
    return path


def install(args):
    client = verified_client(Path(args.client))
    resources = Path(args.resources).expanduser().resolve()
    if not (resources / "horizon-resources.json").is_file():
        raise RuntimeError(f"prepared Horizon resources were not found at {resources}")
    profile = Path(args.profile).expanduser().resolve()
    if profile.exists():
        raise RuntimeError(f"profile directory already exists; choose a new path: {profile}")

    compat = load_compat()
    selected = compat.resolve_mods(args.mods)
    for entry in selected:
        artifact = compat.artifact_path(entry)
        if not artifact.is_file():
            subprocess.run([sys.executable, str(COMPAT_SCRIPT), "fetch", entry["mod_id"]], check=True)
        compat.verify_artifact(entry, artifact)
        compat.verify_metadata(entry, artifact)

    profile.parent.mkdir(parents=True, exist_ok=True)
    staging = Path(tempfile.mkdtemp(prefix=f".{profile.name}.j2h-", dir=profile.parent))
    try:
        game = staging / "game"
        mods_dir = game / "mods"
        mods_dir.mkdir(parents=True)
        installed_client = staging / ("horizon-client.exe" if os.name == "nt" else "horizon-client")
        shutil.copy2(client, installed_client)
        if os.name != "nt":
            installed_client.chmod(installed_client.stat().st_mode | 0o111)
        for entry in selected:
            shutil.copy2(compat.artifact_path(entry), mods_dir / entry["filename"])
        data = {
            "schema": 1,
            "runtime": "java2horizon",
            "minecraft_version": "26.3",
            "client": installed_client.name,
            "resources": str(resources),
            "mods": [{"id": entry["mod_id"], "version": entry["version"],
                      "filename": entry["filename"], "sha512": entry["sha512"]}
                     for entry in selected],
        }
        (staging / MARKER).write_text(json.dumps(data, indent=2) + "\n", encoding="utf-8")
        if profile.exists():
            raise RuntimeError(f"profile path appeared during installation: {profile}")
        os.replace(staging, profile)
    except Exception:
        shutil.rmtree(staging, ignore_errors=True)
        raise
    print(f"Installed isolated J2H profile: {profile}")
    print(f"Mods folder: {profile / 'game' / 'mods'}")
    print("Launch with: python3 tools/j2h_patcher.py launch --profile " + str(profile))


def read_profile(profile: Path):
    marker = profile / MARKER
    if not marker.is_file():
        raise RuntimeError(f"not a J2H profile (missing {MARKER}): {profile}")
    data = json.loads(marker.read_text(encoding="utf-8"))
    if data.get("schema") != 1 or data.get("runtime") != "java2horizon":
        raise RuntimeError(f"unsupported J2H profile marker: {marker}")
    if not isinstance(data.get("client"), str) or Path(data["client"]).name != data["client"]:
        raise RuntimeError(f"invalid client path in profile marker: {marker}")
    client = profile / data["client"]
    verified_client(client)
    resources = Path(data["resources"])
    if not (resources / "horizon-resources.json").is_file():
        raise RuntimeError(f"prepared Horizon resources are no longer available: {resources}")
    for mod in data["mods"]:
        if not isinstance(mod.get("filename"), str) or Path(mod["filename"]).name != mod["filename"]:
            raise RuntimeError(f"invalid mod filename in profile marker: {marker}")
        artifact = profile / "game" / "mods" / mod["filename"]
        compat = load_compat()
        if compat.digest(artifact, "sha512") != mod["sha512"]:
            raise RuntimeError(f"installed mod checksum changed: {artifact}")
    return data, client, resources


def launch(args):
    profile = Path(args.profile).expanduser().resolve()
    data, client, resources = read_profile(profile)
    game = profile / "game"
    isolated = {
        "HORIZON_HOME": profile / "home",
        "HORIZON_LAUNCHER_HOME": profile / "launcher-home",
        "HORIZON_LAUNCHER_TRASH": profile / "launcher-trash",
        "HORIZON_LAUNCHER_CLIPBOARD": profile / "clipboard",
        "HORIZON_LAUNCHER_DESKTOP": profile / "desktop",
        "XDG_DATA_HOME": profile / "data",
        "XDG_CONFIG_HOME": profile / "config",
        "XDG_RUNTIME_DIR": profile / "runtime",
    }
    for path in isolated.values():
        path.mkdir(parents=True, exist_ok=True)
    if os.name != "nt":
        isolated["XDG_RUNTIME_DIR"].chmod(0o700)
    lab = load_game_lab()
    env = lab.game_env(os.environ.copy(), **isolated,
                       HORIZON_LAUNCHER_TRASH=profile / "launcher-trash",
                       HORIZON_LAUNCHER_CLIPBOARD=profile / "clipboard",
                       HORIZON_LAUNCHER_DESKTOP=profile / "desktop")
    env.pop("HORIZON_NO_JAVA_MODS", None)
    env.pop("WAYLAND_DISPLAY", None)
    command = [str(client), "--resources", str(resources), "--game-dir", str(game),
               "--world", args.world, "--render-distance", str(args.render_distance)]
    if args.creative:
        command.append("--creative")
    print(f"Launching {profile} with {len(data['mods'])} pinned mod(s)")
    return subprocess.call(command, cwd=game, env=env)


def status(args):
    profile = Path(args.profile).expanduser().resolve()
    data, _client, _resources = read_profile(profile)
    print(json.dumps({"profile": str(profile), **data}, indent=2))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(required=True)
    install_parser = sub.add_parser("install", help="create a side-by-side J2H profile")
    install_parser.add_argument("--client", required=True, help="J2H-enabled Horizon client binary")
    install_parser.add_argument("--resources", required=True, help="prepared Minecraft 26.3 Horizon resources")
    install_parser.add_argument("--profile", required=True, help="new, isolated profile directory")
    install_parser.add_argument("--mods", nargs="*", default=[], choices=sorted(load_compat().mods_by_id()))
    install_parser.set_defaults(func=install)
    launch_parser = sub.add_parser("launch", help="launch an installed isolated profile")
    launch_parser.add_argument("--profile", required=True)
    launch_parser.add_argument("--world", default="J2H Global Test")
    launch_parser.add_argument("--render-distance", type=int, default=8)
    launch_parser.add_argument("--creative", action="store_true")
    launch_parser.set_defaults(func=launch)
    status_parser = sub.add_parser("status", help="verify a profile and its installed mod checksums")
    status_parser.add_argument("--profile", required=True)
    status_parser.set_defaults(func=status)
    args = parser.parse_args()
    try:
        result = args.func(args)
    except Exception as exc:
        print(f"j2h-patcher: {exc}", file=sys.stderr)
        return 1
    return result if isinstance(result, int) else 0


if __name__ == "__main__":
    raise SystemExit(main())
