#!/usr/bin/env python3
"""Build the J2H Java fixture mods into the ignored compatibility lab."""

import hashlib
import json
import subprocess
import tempfile
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SPEC = Path(__file__).resolve().parent / "fixtures.json"
SOURCES = Path(__file__).resolve().parent / "fixtures"
OUT = ROOT / ".local" / "j2h-compat" / "fixture-artifacts"
LOCK = ROOT / ".local" / "j2h-compat" / "fixtures-lock.json"
BRIDGE = ROOT / "crates" / "java2horizon" / "test_resources" / "j2h-runtime.jar"


def sha512(path):
    digest = hashlib.sha512()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    fixtures = json.loads(SPEC.read_text(encoding="utf-8"))["fixtures"]
    lock_rows = []
    with tempfile.TemporaryDirectory(prefix="j2h-fixtures-") as tmp:
        classes = Path(tmp) / "classes"
        classes.mkdir()
        java_files = sorted(SOURCES.rglob("*.java"))
        subprocess.run(["javac", "--release", "17", "-classpath", str(BRIDGE), "-d", str(classes),
                        *(str(path) for path in java_files)], check=True)
        for fixture in fixtures:
            mod_id = fixture["fixture_id"]
            entrypoint = fixture["entrypoint"]
            metadata = {"schemaVersion": 1, "id": mod_id, "version": fixture["version"],
                        "entrypoints": {"main": [entrypoint]}}
            if fixture.get("depends"):
                metadata["depends"] = {name: "*" for name in fixture["depends"]}
            if fixture.get("recommends"):
                metadata["recommends"] = {name: "*" for name in fixture["recommends"]}
            class_path = Path(*entrypoint.split(".")).with_suffix(".class")
            jar_path = OUT / f"{mod_id}.jar"
            with zipfile.ZipFile(jar_path, "w") as jar:
                for file_path, arcname in [(classes / class_path, class_path.as_posix())]:
                    info = zipfile.ZipInfo(arcname, (1980, 1, 1, 0, 0, 0))
                    info.compress_type = zipfile.ZIP_DEFLATED
                    jar.writestr(info, file_path.read_bytes())
                info = zipfile.ZipInfo("fabric.mod.json", (1980, 1, 1, 0, 0, 0))
                info.compress_type = zipfile.ZIP_DEFLATED
                jar.writestr(info, json.dumps(metadata, separators=(",", ":")).encode())
            row = {**fixture, "filename": jar_path.name, "sha512": sha512(jar_path)}
            lock_rows.append(row)
            print(f"{mod_id}\t{jar_path}\t{row['sha512']}")
    LOCK.parent.mkdir(parents=True, exist_ok=True)
    LOCK.write_text(json.dumps({"schema": 1, "fixtures": lock_rows}, indent=2, sort_keys=True) + "\n",
                    encoding="utf-8")


if __name__ == "__main__":
    main()
