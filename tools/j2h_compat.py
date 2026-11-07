#!/usr/bin/env python3
"""Run pinned Java/Fabric mod compatibility scenarios in isolated Horizon homes."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.request
import zipfile
from datetime import datetime, timezone
from pathlib import Path
from urllib.parse import urlparse


ROOT = Path(__file__).resolve().parents[1]
LAB = ROOT / ".local" / "j2h-compat"
CATALOG = ROOT / "tools" / "j2h_compat" / "catalog.json"
SCORECARD = ROOT / "tools" / "j2h_compat" / "scorecard.json"
ALLOWED_DOWNLOAD_HOSTS = {"cdn.modrinth.com"}


def read_json(path: Path):
    return json.loads(path.read_text(encoding="utf-8"))


def write_json(path: Path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def digest(path: Path, algorithm="sha512") -> str:
    h = hashlib.new(algorithm)
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def mods_by_id():
    return {entry["mod_id"]: entry for entry in read_json(CATALOG)["mods"]}


def fixture_ids():
    return [entry["fixture_id"] for entry in read_json(ROOT / "tools" / "j2h_compat" / "fixtures.json")["fixtures"]]


def all_entries():
    entries = mods_by_id()
    lock = LAB / "fixtures-lock.json"
    if lock.is_file():
        for fixture in read_json(lock)["fixtures"]:
            entries[fixture["fixture_id"]] = {
                **fixture,
                "mod_id": fixture["fixture_id"], "name": fixture["fixture_id"],
                "version": fixture["version"], "filename": fixture["filename"],
                "sha512": fixture["sha512"], "requires": fixture.get("depends", []),
                "artifact_directory": "fixture-artifacts", "fixture": True,
            }
    return entries


def evaluate_feature_probe(entry, combined_log, mod_failures):
    markers = entry.get("expected_log_markers")
    if not markers:
        marker = entry.get("expected_log_marker")
        markers = [marker] if marker else []
    if not markers:
        raise RuntimeError(f"feature probe {entry['mod_id']} has no expected log markers")
    assertions = {
        f"{entry['mod_id']}_marker_{index + 1}": marker in combined_log
        for index, marker in enumerate(markers)
    }
    passed = all(assertions.values()) and entry["mod_id"] not in mod_failures
    assertions[f"{entry['mod_id']}_passed"] = passed
    features = []
    if passed and entry.get("verifies_mod") and entry.get("verified_feature"):
        features.append(entry["verified_feature"])
    return assertions, features


def evaluate_rei_gui_evidence(control_log):
    """Report REI screen transition and successful extraction as separate facts."""
    screen = "me.shedaniel.rei.impl.client.gui.config.REIConfigScreen"
    opened = f"screen accepted: {screen}" in control_log
    match = re.search(
        rf"R2 renderFrame done screen={re.escape(screen)} "
        r"fill=(\d+) grad=(\d+) text=(\d+) sprite=(\d+) blit=(\d+) rstate=(\d+)",
        control_log,
    )
    commands = None
    if match:
        commands = {
            name: int(value) for name, value in zip(
                ("fill", "gradient", "text", "sprite", "blit", "render_state_text"),
                match.groups(),
            )
        }
    return {
        "rei_config_screen_opened": opened,
        "rei_config_screen_rendered": match is not None,
        "rei_config_screen_render_commands": commands,
    }


def evaluate_jade_cloth_config_evidence(control_log):
    """Keep Cloth Config's screen transition distinct from successful extraction."""
    screen = "me.shedaniel.clothconfig2.gui.GlobalizedClothConfigScreen"
    opened = f"screen accepted: {screen}" in control_log
    match = re.search(
        rf"R2 renderFrame done screen={re.escape(screen)} "
        r"fill=(\d+) grad=(\d+) text=(\d+) sprite=(\d+) blit=(\d+) rstate=(\d+)",
        control_log,
    )
    return {
        "jade_cloth_config_screen_opened": opened,
        "jade_cloth_config_screen_rendered": match is not None,
        "jade_cloth_config_render_commands": (
            {
                name: int(value) for name, value in zip(
                    ("fill", "gradient", "text", "sprite", "blit", "render_state_text"),
                    match.groups(),
                )
            }
            if match else None
        ),
    }


def stage_fixture_files(selected, game_dir):
    source_root = (ROOT / "tools" / "j2h_compat" / "fixtures").resolve()
    staged = {}
    for entry in selected:
        for setup_file in entry.get("prelaunch_files", []):
            source = (source_root / setup_file["source"]).resolve()
            destination_relative = Path(setup_file["destination"])
            if not source.is_relative_to(source_root) or destination_relative.is_absolute() or ".." in destination_relative.parts:
                raise RuntimeError(f"fixture {entry['mod_id']} has an unsafe prelaunch file path")
            destination = game_dir / destination_relative
            contents = source.read_bytes()
            relative_name = destination_relative.as_posix()
            previous = staged.get(relative_name)
            if previous is not None and previous != contents:
                raise RuntimeError(f"fixtures provide conflicting prelaunch file {relative_name}")
            staged[relative_name] = contents
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_bytes(contents)
    return sorted(staged)


def resolve_mods(requested):
    entries = all_entries()
    ordered = []
    seen = set()

    def visit(mod_id):
        if mod_id in seen:
            return
        if mod_id not in entries:
            raise RuntimeError(f"required dependency is not pinned in catalog: {mod_id}")
        seen.add(mod_id)
        for dependency in entries[mod_id].get("requires", []):
            visit(dependency)
        ordered.append(entries[mod_id])

    for mod_id in requested:
        visit(mod_id)
    return ordered


def validate_catalog():
    catalog = read_json(CATALOG)
    entries = mods_by_id()
    if catalog.get("schema") != 1 or catalog.get("minecraft_version") != "26.3":
        raise RuntimeError("catalog schema/version is not pinned to Minecraft 26.3")
    for entry in entries.values():
        for key in ("name", "version", "license", "source_url", "download_url", "filename"):
            if not entry.get(key):
                raise RuntimeError(f"{entry.get('mod_id')}: missing pinned field {key}")
        if entry.get("minecraft_version") != "26.3" or not re.fullmatch(r"[0-9a-f]{128}", entry.get("sha512", "")):
            raise RuntimeError(f"{entry['mod_id']}: invalid Minecraft pin or SHA-512")
        parsed = urlparse(entry["download_url"])
        if parsed.scheme != "https" or parsed.hostname not in ALLOWED_DOWNLOAD_HOSTS:
            raise RuntimeError(f"{entry['mod_id']}: download host is not allowlisted")
        resolve_mods([entry["mod_id"]])
    statuses = {"NOT_TESTED", "DISCOVERED", "BASELINE_FAILED", "IN_PROGRESS",
                "PARTIALLY_COMPATIBLE", "VERIFIED_COMPATIBLE", "VERIFIED_IN_COMBINATION",
                "UNSUPPORTED", "BLOCKED_BY_ENGINE", "BLOCKED_BY_DEPENDENCY"}
    scorecard = read_json(SCORECARD)
    rows = {row["mod_id"]: row for row in scorecard["results"]}
    if set(rows) != set(entries):
        raise RuntimeError("scorecard and catalog mod IDs differ")
    for row in rows.values():
        if row.get("status") not in statuses:
            raise RuntimeError(f"{row['mod_id']}: unknown scorecard status {row.get('status')}")
    for row in rows.values():
        if row.get("status") in {"VERIFIED_COMPATIBLE", "VERIFIED_IN_COMBINATION"} and not row.get("evidence"):
            raise RuntimeError(f"{row['mod_id']}: verified status has no evidence path")
    return len(entries)


def artifact_path(entry):
    return LAB / entry.get("artifact_directory", "artifacts") / entry["filename"]


def verify_artifact(entry, path: Path):
    expected = entry["sha512"]
    actual = digest(path)
    if actual != expected:
        raise RuntimeError(f"SHA-512 mismatch for {entry['mod_id']}: expected {expected}, got {actual}")
    return actual


def verify_metadata(entry, path: Path):
    if entry.get("fixture"):
        return
    try:
        with zipfile.ZipFile(path) as archive:
            metadata = json.loads(archive.read("fabric.mod.json"))
    except (OSError, KeyError, zipfile.BadZipFile, json.JSONDecodeError) as exc:
        raise RuntimeError(f"{entry['mod_id']}: invalid Fabric metadata in {path}: {exc}") from exc
    expected_version = entry.get("fabric_metadata_version")
    if metadata.get("id") != entry["mod_id"] or metadata.get("version") != expected_version:
        raise RuntimeError(f"{entry['mod_id']}: artifact metadata ID/version does not match the lock")
    expected_dependencies = entry.get("fabric_metadata_dependencies")
    if expected_dependencies is not None and metadata.get("depends", {}) != expected_dependencies:
        raise RuntimeError(f"{entry['mod_id']}: artifact dependencies do not match the locked metadata")


def fetch(args):
    fetched = set()
    selected = []
    for mod_id in args.mods:
        for entry in resolve_mods([mod_id]):
            if entry["mod_id"] not in fetched:
                fetched.add(entry["mod_id"])
                selected.append(entry)
    for entry in selected:
        destination = artifact_path(entry)
        if destination.is_file():
            verify_artifact(entry, destination)
            verify_metadata(entry, destination)
            print(f"already verified {entry['mod_id']} {entry['version']}")
            continue
        url = entry["download_url"]
        parsed = urlparse(url)
        if parsed.scheme != "https" or parsed.hostname not in ALLOWED_DOWNLOAD_HOSTS:
            raise RuntimeError(f"refusing non-approved artifact host: {url}")
        destination.parent.mkdir(parents=True, exist_ok=True)
        request = urllib.request.Request(url, headers={"User-Agent": "Horizon-J2H-Compatibility-Lab/1.0"})
        with urllib.request.urlopen(request, timeout=90) as response, destination.open("wb") as out:
            shutil.copyfileobj(response, out)
        verify_artifact(entry, destination)
        verify_metadata(entry, destination)
        print(f"verified {entry['mod_id']} {entry['version']} -> {destination}")


def install_local(args):
    entry = mods_by_id()[args.mod]
    source = Path(args.path).expanduser().resolve()
    if not source.is_file():
        raise RuntimeError(f"artifact does not exist: {source}")
    verify_artifact(entry, source)
    verify_metadata(entry, source)
    destination = artifact_path(entry)
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(source, destination)
    print(f"verified and staged {entry['mod_id']} {entry['version']} -> {destination}")


def ctl(socket_path: Path, *args, timeout=30):
    binary = ROOT / "target" / "fast" / "horizon-ctl"
    result = subprocess.run([str(binary), "--socket", str(socket_path), *args], cwd=ROOT,
                            text=True, capture_output=True, timeout=timeout)
    if result.returncode:
        raise RuntimeError(f"horizon-ctl {' '.join(args)} failed: {result.stdout}{result.stderr}")
    return result.stdout.strip()


def screenshot_metrics(path: Path):
    from PIL import Image, ImageChops

    image = Image.open(path).convert("L")
    width, height = image.size

    def bright_fraction(box):
        x1, y1, x2, y2 = box
        crop = image.crop((int(width * x1), int(height * y1),
                           int(width * x2), int(height * y2)))
        histogram = crop.histogram()
        bright = sum(histogram[241:])
        return bright / (crop.width * crop.height)

    return {
        "image_size": [width, height],
        # These are the label interiors of Mod Menu's two side-by-side link buttons.
        "website_label_pixels": bright_fraction((0.55, 0.31, 0.77, 0.38)),
        "issues_label_pixels": bright_fraction((0.80, 0.31, 0.99, 0.38)),
        # The mod card occupies the left list pane below the search/count rows.
        "mod_list_pixels": bright_fraction((0.00, 0.24, 0.49, 0.88)),
    }


def screenshots_differ_in_mod_list(before: Path, after: Path) -> bool:
    from PIL import Image, ImageChops

    a, b = Image.open(before).convert("RGB"), Image.open(after).convert("RGB")
    if a.size != b.size:
        return True
    width, height = a.size
    # Mod Menu's list is the left pane. Compare it so a changing detail panel
    # or tooltip cannot masquerade as list scrolling.
    box = (int(width * 0.00), int(height * 0.24),
           int(width * 0.49), int(height * 0.88))
    diff = ImageChops.difference(a.crop(box), b.crop(box)).convert("L")
    changed = sum(diff.histogram()[8:])
    return changed / (diff.width * diff.height) > 0.002


def copy_if_exists(source: Path, destination: Path):
    if source.is_file():
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, destination)


def isolated_game_env(extra):
    """Use the repository's required scratch-home setup for every game spawn."""
    import importlib.util

    path = ROOT / "tools" / "lab.py"
    spec = importlib.util.spec_from_file_location("j2h_game_lab", path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load game environment helper: {path}")
    lab = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = lab
    spec.loader.exec_module(lab)
    return lab.game_env(os.environ.copy(), **extra)


def update_scorecard(selected, record):
    path = SCORECARD
    scorecard = read_json(path)
    rows = {row["mod_id"]: row for row in scorecard["results"]}
    evidence = f".local/j2h-compat/runs/{record['run_id']}/"
    failed_ids = set(record.get("mod_failures", {}))
    skipped_ids = record.get("dependency_skips", {})
    for entry in selected:
        if entry.get("fixture") or entry["mod_id"] not in rows:
            continue
        row = rows[entry["mod_id"]]
        pipeline_failure = record.get("j2h_pipeline_failure")
        if pipeline_failure:
            diagnostic = f"J2H did not reach mod entrypoints in run {record['run_id']}: {pipeline_failure}"
            if diagnostic not in row.setdefault("diagnostics", []):
                row["diagnostics"].append(diagnostic)
            continue
        verified_features = record.get("features_verified_by_mod", {}).get(entry["mod_id"], [])
        if verified_features:
            for feature in verified_features:
                if feature not in row.setdefault("features_verified", []):
                    row["features_verified"].append(feature)
            if entry["mod_id"] not in failed_ids:
                row["status"] = "PARTIALLY_COMPATIBLE"
        if any(entry["mod_id"] in line for line in record.get("access_widener_transformations", [])):
            feature = "Runtime access widener transformation observed"
            if feature not in row.setdefault("features_verified", []):
                row["features_verified"].append(feature)
        if record["run_id"] not in row.setdefault("regression_history", []):
            row["regression_history"].append(record["run_id"])
        if evidence not in row.setdefault("evidence", []):
            row["evidence"].append(evidence)
        if entry["mod_id"] in record.get("mod_failures", {}):
            row["status"] = "BASELINE_FAILED"
            diagnostic = record["mod_failures"][entry["mod_id"]]
            if diagnostic not in row.setdefault("diagnostics", []):
                row["diagnostics"].append(diagnostic)
        elif entry["mod_id"] == "modmenu" and any(
                key.startswith("modmenu_") for key in record.get("assertions", {})):
            if all(record["assertions"].values()):
                mod_count = sum(not item.get("fixture") for item in selected)
                row["status"] = (
                    "VERIFIED_IN_COMBINATION" if mod_count > 1 else "VERIFIED_COMPATIBLE"
                )
                features = {
                    "modmenu_opens": "Mod Menu opens",
                    "modmenu_button_labels_render": "Website and Issues labels render",
                    "modmenu_search_filters_to_zero": "Search filters to no results",
                    "modmenu_search_restores_result": "Search restores matching result",
                    "modmenu_scroll_changes_visible_content": "Scrolling changes visible content",
                    "modmenu_escape_closes": "Escape closes the screen",
                    "modmenu_reopens": "Reopening the screen works",
                }
                for key, feature in features.items():
                    if feature not in row.setdefault("features_verified", []):
                        row["features_verified"].append(feature)
                if mod_count > 1:
                    companions = sorted(
                        item["mod_id"] for item in selected
                        if item["mod_id"] != "modmenu" and not item.get("fixture")
                    )
                    feature = "Mod Menu GUI regression passed with: " + ", ".join(companions)
                    if feature not in row.setdefault("features_verified", []):
                        row["features_verified"].append(feature)
            else:
                row["status"] = "BASELINE_FAILED"
                diagnostic = "Automated Mod Menu GUI regression assertions failed; inspect gui and screenshots."
                if diagnostic not in row.setdefault("diagnostics", []):
                    row["diagnostics"].append(diagnostic)
        elif any(dependency in failed_ids for dependency in entry.get("requires", [])):
            row["status"] = "BLOCKED_BY_DEPENDENCY"
            dependency = next(dependency for dependency in entry["requires"] if dependency in failed_ids)
            diagnostic = f"Required dependency {dependency} failed initialization in run {record['run_id']}."
            if diagnostic not in row.setdefault("diagnostics", []):
                row["diagnostics"].append(diagnostic)
        elif entry["mod_id"] in skipped_ids:
            row["status"] = "BLOCKED_BY_DEPENDENCY"
            diagnostic = skipped_ids[entry["mod_id"]]
            if diagnostic not in row.setdefault("diagnostics", []):
                row["diagnostics"].append(diagnostic)
        modmenu_assertions = {
            key: value for key, value in record.get("assertions", {}).items()
            if key.startswith("modmenu_")
        }
        modmenu_gui_failed = bool(modmenu_assertions) and not all(modmenu_assertions.values())
        if (entry["mod_id"] not in failed_ids
                and entry["mod_id"] not in skipped_ids
                and not (entry["mod_id"] == "modmenu" and modmenu_gui_failed)
                and row["status"] in {"NOT_TESTED", "DISCOVERED", "BASELINE_FAILED", "BLOCKED_BY_DEPENDENCY"}
                and record["runtime_started"]):
            row["status"] = "IN_PROGRESS"
            diagnostic = "Live runtime startup observed; mod-specific functionality still needs an assertion."
            if diagnostic not in row.setdefault("diagnostics", []):
                row["diagnostics"].append(diagnostic)
    write_json(path, scorecard)


def run_scenario(args):
    requested = (args.mods or []) + (args.fixtures or [])
    if not requested:
        raise RuntimeError("select at least one --mods entry or --fixtures entry")
    if args.fixtures:
        subprocess.run([sys.executable, str(ROOT / "tools" / "j2h_compat" / "build_fixtures.py")],
                       cwd=ROOT, check=True, stdout=subprocess.DEVNULL)
    selected = resolve_mods(requested)
    for entry in selected:
        verify_artifact(entry, artifact_path(entry))
        verify_metadata(entry, artifact_path(entry))

    run_id = args.run_id or datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    run_dir = LAB / "runs" / run_id
    if run_dir.exists():
        raise RuntimeError(f"run already exists: {run_dir}")
    game_dir = run_dir / "game"
    mods_dir = game_dir / "mods"
    mods_dir.mkdir(parents=True)
    fixture_setup_files = stage_fixture_files(selected, game_dir)
    for entry in selected:
        shutil.copy2(artifact_path(entry), mods_dir / entry["filename"])

    isolated = {key: run_dir / name for key, name in {
        "HORIZON_HOME": "home", "HORIZON_LAUNCHER_HOME": "launcher-home",
        "HORIZON_LAUNCHER_TRASH": "launcher-trash", "HORIZON_LAUNCHER_CLIPBOARD": "clipboard",
        "HORIZON_LAUNCHER_DESKTOP": "desktop", "XDG_DATA_HOME": "data",
        "XDG_CONFIG_HOME": "config",
    }.items()}
    for path in isolated.values():
        path.mkdir(parents=True, exist_ok=True)
    for key, path in isolated.items():
        os.chmod(path, 0o700)
    socket_runtime = Path(tempfile.mkdtemp(prefix="j2h-compat-runtime-"))
    os.chmod(socket_runtime, 0o700)
    socket_path = socket_runtime / "horizon-control.sock"
    env = isolated_game_env({key: str(path) for key, path in isolated.items()} | {
        "XDG_RUNTIME_DIR": str(socket_runtime),
        "HORIZON_FABRIC_LIBS_DIR": str(LAB / "fabric-libraries"),
        "HORIZON_JAVA_MODS_DIR": str(mods_dir),
        "HORIZON_LAUNCHER_TRASH": str(isolated["HORIZON_HOME"].parent / "launcher-trash"),
        "HORIZON_LAUNCHER_CLIPBOARD": str(isolated["HORIZON_HOME"].parent / "clipboard"),
        "HORIZON_LAUNCHER_DESKTOP": str(isolated["HORIZON_HOME"].parent / "desktop"),
    })
    if "modmenu" in (args.mods or []):
        # The live GUI checks use these bounded screen markers to distinguish
        # accepting a Java screen from actually extracting its render state.
        env["JAVA_TOOL_OPTIONS"] = (
            env.get("JAVA_TOOL_OPTIONS", "") + " -Dj2h.debug.gui=true"
        ).strip()
    env.pop("WAYLAND_DISPLAY", None)
    library_cache = LAB / "library-cache"
    if library_cache.is_dir():
        shutil.copytree(library_cache, game_dir / ".horizon-cache" / "java2horizon-libraries")

    resources = ROOT / ".local" / "horizon-client" / "26.3"
    client = Path(os.environ.get("J2H_COMPAT_CLIENT", ROOT / "target" / "fast" / "horizon-client"))
    if not resources.is_dir() or not client.is_file():
        raise RuntimeError("prepare .local/horizon-client/26.3 and build target/fast/horizon-client first")
    launch_args = [str(ROOT / "target" / "fast" / "horizon-ctl"), "--socket", str(socket_path),
                   "launch", "--game", str(client), "--wait", "--world", "J2H-" + run_id,
                   "--creative", "--resources", str(resources), "--game-dir", str(game_dir),
                   "--render-distance", "4", "--offscreen"]
    output_log = run_dir / "launch.log"
    status = "NOT_TESTED"
    phase = "launch"
    error = None
    runtime_started = False
    gui = {}
    try:
        with output_log.open("w", encoding="utf-8") as log:
            result = subprocess.run(launch_args, cwd=run_dir, env=env, text=True,
                                    stdout=log, stderr=subprocess.STDOUT, timeout=args.startup_timeout)
        if result.returncode:
            raise RuntimeError(f"horizon-ctl launch exited {result.returncode}")
        startup = ctl(socket_path, "state")
        runtime_started = True
        phase = "live_game"
        (run_dir / "state.json").write_text(startup + "\n", encoding="utf-8")
        if "modmenu" in (args.mods or []):
            ctl(socket_path, "key", "m", "tap")
            time.sleep(1)
            gui["screen"] = ctl(socket_path, "screen")
            gui["widgets"] = ctl(socket_path, "ui")
            ctl(socket_path, "screenshot", str(run_dir / "mod-menu.png"))
            gui["initial_screen"] = json.loads(gui["screen"]).get("screen")
            initial_metrics = screenshot_metrics(run_dir / "mod-menu.png")
            gui["button_label_pixels"] = {
                key: initial_metrics[key]
                for key in ("website_label_pixels", "issues_label_pixels")
            }
            gui["button_text_check"] = all(value > 0.001 for value in gui["button_label_pixels"].values())
            gui["initial_mod_list_pixels"] = initial_metrics["mod_list_pixels"]
            gui["search_zero_results_check"] = False
            gui["search_restores_result_check"] = False
            gui["scroll_changes_visible_content"] = False
            gui["escape_closed"] = False
            gui["reopen_succeeded"] = False
            ctl(socket_path, "click-at", "100", "30")
            ctl(socket_path, "type", "j2h-no-such-mod")
            time.sleep(0.5)
            ctl(socket_path, "screenshot", str(run_dir / "mod-menu-search.png"))
            search_metrics = screenshot_metrics(run_dir / "mod-menu-search.png")
            gui["filtered_mod_list_pixels"] = search_metrics["mod_list_pixels"]
            gui["search_zero_results_check"] = (
                gui["initial_mod_list_pixels"] > 0.001
                and search_metrics["mod_list_pixels"] < gui["initial_mod_list_pixels"] * 0.5
            )
            ctl(socket_path, "key", "esc", "tap")
            time.sleep(0.2)
            gui["closed_screen"] = json.loads(ctl(socket_path, "screen")).get("screen")
            gui["escape_closed"] = gui["closed_screen"] != "java-mods"
            ctl(socket_path, "key", "m", "tap")
            time.sleep(0.5)
            gui["reopened_screen"] = json.loads(ctl(socket_path, "screen")).get("screen")
            gui["reopen_succeeded"] = gui["reopened_screen"] == "java-mods"
            if {"modmenu", "jade"}.issubset({mod["mod_id"] for mod in selected}):
                # Jade's Mod Menu provider returns Cloth Config's
                # GlobalizedClothConfigScreen, the exact screen family from
                # the reported extractPanorama NPE.
                ctl(socket_path, "click-at", "100", "30")
                ctl(socket_path, "type", "Jade")
                time.sleep(0.3)
                ctl(socket_path, "click-at", "80", "100")
                time.sleep(0.2)
                ctl(socket_path, "click-at", "477", "55")
                time.sleep(0.7)
                ctl(socket_path, "screenshot", str(run_dir / "jade-cloth-config.png"))
                gui["jade_cloth_config_screen_opened"] = False
                gui["jade_cloth_config_screen_rendered"] = False
                gui["jade_cloth_config_render_commands"] = None
                ctl(socket_path, "key", "esc", "tap")
                time.sleep(0.2)
                ctl(socket_path, "click-at", "100", "30")
                for _ in range(len("Jade")):
                    ctl(socket_path, "type", "\\b")
                time.sleep(0.2)
            if {"modmenu", "roughlyenoughitems"}.issubset({mod["mod_id"] for mod in selected}):
                # Open REI through the real Mod Menu config-provider path. This
                # captures the live screen so its settings and click targets
                # can be exercised and pinned by a follow-up feature probe.
                ctl(socket_path, "click-at", "100", "30")
                ctl(socket_path, "type", "Roughly Enough Items")
                time.sleep(0.3)
                ctl(socket_path, "click-at", "80", "100")
                time.sleep(0.2)
                ctl(socket_path, "screenshot", str(run_dir / "mod-menu-rei-selected.png"))
                ctl(socket_path, "click-at", "477", "55")
                time.sleep(0.7)
                ctl(socket_path, "screenshot", str(run_dir / "rei-config-screen.png"))
                gui["rei_config_screen_opened"] = False
                gui["rei_config_screen_rendered"] = False
                gui["rei_config_screen_render_commands"] = None
                ctl(socket_path, "key", "esc", "tap")
                time.sleep(0.2)
                # Closing the provider screen returns to Mod Menu with its
                # search query intact. Clear it before the independent scroll
                # assertion so the list has all resolved mods to scroll.
                ctl(socket_path, "click-at", "100", "30")
                for _ in range(len("Roughly Enough Items")):
                    ctl(socket_path, "type", "\\b")
                time.sleep(0.2)
            # Fabric's built-in Minecraft row precedes the user mods. Select
            # the second row so the scroll assertion exercises a longer user
            # mod description, including in combinations with extra libraries.
            ctl(socket_path, "click-at", "80", "120")
            time.sleep(0.2)
            ctl(socket_path, "screenshot", str(run_dir / "mod-menu-results.png"))
            restored_metrics = screenshot_metrics(run_dir / "mod-menu-results.png")
            gui["restored_mod_list_pixels"] = restored_metrics["mod_list_pixels"]
            gui["search_restores_result_check"] = restored_metrics["mod_list_pixels"] > 0.001
            ctl(socket_path, "scroll-at", "130", "130", "-6")
            ctl(socket_path, "screenshot", str(run_dir / "mod-menu-scrolled.png"))
            gui["scroll_changes_visible_content"] = screenshots_differ_in_mod_list(
                run_dir / "mod-menu-results.png", run_dir / "mod-menu-scrolled.png"
            )
            ctl(socket_path, "key", "esc", "tap")
            time.sleep(0.2)
            gui["closed_screen"] = json.loads(ctl(socket_path, "screen")).get("screen")
            gui["escape_closed"] = gui["closed_screen"] != "java-mods"
            ctl(socket_path, "key", "m", "tap")
            time.sleep(0.5)
            gui["reopened_screen"] = json.loads(ctl(socket_path, "screen")).get("screen")
            gui["reopen_succeeded"] = gui["reopened_screen"] == "java-mods"
        ctl(socket_path, "screenshot", str(run_dir / "gameplay.png"))
        status = "IN_PROGRESS"
        error = "Runtime started; only captured startup behavior is assessed, feature compatibility remains unverified."
    except Exception as exc:  # retain diagnostics and make cleanup best-effort
        error = str(exc)
    finally:
        try:
            if socket_path.exists():
                ctl(socket_path, "quit", timeout=60)
        except Exception as exc:
            (run_dir / "shutdown-error.txt").write_text(str(exc) + "\n", encoding="utf-8")
        copy_if_exists(game_dir / "logs" / "latest.log", run_dir / "game-latest.log")
        copy_if_exists(isolated["HORIZON_HOME"] / "logs" / "latest.log", run_dir / "horizon-latest.log")
        copy_if_exists(socket_runtime / "horizon-control.log", run_dir / "horizon-control.log")
        shutil.rmtree(socket_runtime, ignore_errors=True)

    content_root = game_dir / ".horizon-cache" / "java2horizon"
    generated = []
    if content_root.exists():
        generated = [str(p.relative_to(content_root)) for p in sorted(content_root.rglob("*")) if p.is_file()]
    generated_roots = sorted(p.name for p in content_root.iterdir() if p.is_dir() and p.name != "nested") \
        if content_root.exists() else []
    horizon_log = (run_dir / "horizon-latest.log").read_text(encoding="utf-8", errors="replace") \
        if (run_dir / "horizon-latest.log").exists() else ""
    game_log = (run_dir / "game-latest.log").read_text(encoding="utf-8", errors="replace") \
        if (run_dir / "game-latest.log").exists() else ""
    control_log = (run_dir / "horizon-control.log").read_text(encoding="utf-8", errors="replace") \
        if (run_dir / "horizon-control.log").exists() else ""
    access_widener_transformations = [
        line for line in control_log.splitlines()
        if "J2H: access widener transformed " in line
    ]
    if "rei_config_screen_opened" in gui:
        gui.update(evaluate_rei_gui_evidence(control_log))
    if "jade_cloth_config_screen_opened" in gui:
        gui.update(evaluate_jade_cloth_config_evidence(control_log))
    selected_ids = {entry["mod_id"] for entry in selected}
    mod_failures = {}
    dependency_skips = {}
    for line in horizon_log.splitlines():
        match = re.search(r"J2H: mod '([^']+)' initialization failed: (.*)", line)
        if match and match.group(1) in selected_ids:
            mod_failures[match.group(1)] = match.group(2)
        match = re.search(r"J2H: skipping mod '([^']+)': (.*)", line)
        if match and match.group(1) in selected_ids:
            dependency_skips[match.group(1)] = match.group(2)
    j2h_pipeline_failure = next(
        (line.split("Horizon: Java mods not loaded: ", 1)[1]
         for line in horizon_log.splitlines()
         if "Horizon: Java mods not loaded: " in line),
        None,
    )
    if mod_failures and not args.fixtures:
        status = "BASELINE_FAILED"
        error = "Mod initialization failures were logged; see mod_failures and horizon-latest.log."
    if j2h_pipeline_failure and not args.fixtures:
        status = "NOT_TESTED"
        error = f"J2H runtime did not reach mod entrypoints: {j2h_pipeline_failure}"
    assertions = {}
    features_verified_by_mod = {}
    if "modmenu" in (args.mods or []) and runtime_started:
        assertions = {
            "modmenu_opens": gui.get("initial_screen") == "java-mods",
            "modmenu_button_labels_render": gui.get("button_text_check") is True,
            "modmenu_search_filters_to_zero": gui.get("search_zero_results_check") is True,
            "modmenu_search_restores_result": gui.get("search_restores_result_check") is True,
            "modmenu_scroll_changes_visible_content": gui.get("scroll_changes_visible_content") is True,
            "modmenu_escape_closes": gui.get("escape_closed") is True,
            "modmenu_reopens": gui.get("reopen_succeeded") is True,
        }
        if not all(assertions.values()):
            status = "BASELINE_FAILED"
            error = "One or more automated Mod Menu input or screenshot assertions failed."
        elif args.mods == ["modmenu"] and not mod_failures:
            status = "VERIFIED_COMPATIBLE"
            error = None
    if args.fixtures:
        fixture_scenarios = {entry.get("scenario", "transaction") for entry in selected if entry.get("fixture")}
        if fixture_scenarios == {"transaction"}:
            assertions = {
                "failed_mod_rolled_back": "brokenmod" not in generated_roots and "initialization failed" in horizon_log,
                "required_dependent_skipped": "requiredmod" not in generated_roots,
                "independent_mod_committed": "goodmod" in generated_roots,
                "optional_dependent_committed": "optionalmod" in generated_roots,
                "ruby_block_and_item_content_committed": "ruby" in generated_roots,
                "content_pipeline_saw_successful_roots": "content from 3 mod(s)" in horizon_log,
                "registration_counts_match_successes": "Java mods registered 3 block(s), 1 item(s)" in horizon_log,
            }
            status = "VERIFIED_IN_COMBINATION" if all(assertions.values()) else "BASELINE_FAILED"
            if not all(assertions.values()):
                error = "One or more transaction fixture assertions failed."
            else:
                error = None
        elif fixture_scenarios == {"feature_probe"}:
            combined_log = "\n".join((horizon_log, control_log, game_log))
            for entry in selected:
                if not entry.get("fixture"):
                    continue
                probe_assertions, verified_features = evaluate_feature_probe(
                    entry, combined_log, mod_failures
                )
                assertions.update(probe_assertions)
                if verified_features:
                    features_verified_by_mod.setdefault(entry["verifies_mod"], []).extend(verified_features)
            status = "PARTIALLY_COMPATIBLE" if assertions and all(assertions.values()) else "BASELINE_FAILED"
            if status == "BASELINE_FAILED":
                error = "One or more live feature-probe assertions failed; inspect mod_failures and captured logs."
            else:
                error = None
        else:
            raise RuntimeError(f"cannot combine fixture scenarios: {sorted(fixture_scenarios)}")
    record = {
        "schema": 1, "run_id": run_id, "created_at": datetime.now(timezone.utc).isoformat(),
        "requested_mods": args.mods,
        "requested_fixtures": args.fixtures,
        "fixture_setup_files": fixture_setup_files,
        "mods": [{"mod_id": e["mod_id"], "version": e["version"], "sha512": e["sha512"]} for e in selected],
        "minecraft": "26.3", "loader": "Fabric metadata / J2H embedded runtime",
        "environment": str(run_dir), "status": status, "phase": phase, "diagnostic": error,
        "runtime_started": runtime_started,
        "generated_content_files": generated, "generated_mod_roots": generated_roots,
        "assertions": assertions, "features_verified_by_mod": features_verified_by_mod,
        "mod_failures": mod_failures,
        "dependency_skips": dependency_skips, "gui": gui,
        "j2h_pipeline_failure": j2h_pipeline_failure,
        "access_widener_transformations": access_widener_transformations,
        "evidence": [str(p.relative_to(run_dir)) for p in sorted(run_dir.iterdir()) if p.is_file()] + ["result.json"],
    }
    write_json(run_dir / "result.json", record)
    update_scorecard(selected, record)
    print(json.dumps(record, indent=2))


def show_scorecard(_args):
    validate_catalog()
    print(json.dumps(read_json(SCORECARD), indent=2))


def list_catalog(_args):
    validate_catalog()
    print(json.dumps(read_json(CATALOG), indent=2))


def validate(_args):
    print(f"J2H compatibility catalog valid: {validate_catalog()} pinned mods")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(required=True)
    sub.add_parser("catalog").set_defaults(func=list_catalog)
    sub.add_parser("scorecard").set_defaults(func=show_scorecard)
    sub.add_parser("validate").set_defaults(func=validate)
    p = sub.add_parser("fetch", help="download one pinned artifact and verify its published SHA-512")
    p.add_argument("mods", nargs="+", choices=sorted(mods_by_id()))
    p.set_defaults(func=fetch)
    p = sub.add_parser("stage", help="stage an existing artifact after checksum verification")
    p.add_argument("mod", choices=sorted(mods_by_id()))
    p.add_argument("path")
    p.set_defaults(func=install_local)
    p = sub.add_parser("run", help="launch an isolated live-game scenario")
    p.add_argument("--mods", nargs="+", choices=sorted(mods_by_id()))
    p.add_argument("--fixtures", nargs="+", choices=sorted(fixture_ids()))
    p.add_argument("--run-id")
    p.add_argument("--startup-timeout", type=int, default=240)
    p.set_defaults(func=run_scenario)
    args = parser.parse_args()
    try:
        args.func(args)
    except Exception as exc:
        print(f"j2h-compat: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
