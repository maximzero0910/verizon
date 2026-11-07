import importlib.util
import tempfile
import unittest
from pathlib import Path

J2H_COMPAT_PATH = Path(__file__).resolve().parents[1] / "tools" / "j2h_compat.py"
J2H_COMPAT_SPEC = importlib.util.spec_from_file_location("j2h_compat", J2H_COMPAT_PATH)
j2h_compat = importlib.util.module_from_spec(J2H_COMPAT_SPEC)
J2H_COMPAT_SPEC.loader.exec_module(j2h_compat)


class J2HCompatibilityCatalogTests(unittest.TestCase):
    def test_rei_screen_open_does_not_claim_render_success_after_atlas_failure(self):
        screen = "me.shedaniel.rei.impl.client.gui.config.REIConfigScreen"
        log = "screen accepted: " + screen + "\n"
        log += "R2 renderFrame exception screen=" + screen
        log += " java.lang.IllegalStateException: Tried to lookup sprite, but atlas is not initialized"
        evidence = j2h_compat.evaluate_rei_gui_evidence(log)
        self.assertTrue(evidence["rei_config_screen_opened"])
        self.assertFalse(evidence["rei_config_screen_rendered"])
        self.assertIsNone(evidence["rei_config_screen_render_commands"])

    def test_rei_render_success_requires_completion_marker_for_actual_screen(self):
        screen = "me.shedaniel.rei.impl.client.gui.config.REIConfigScreen"
        log = "R2 renderFrame done screen=" + screen
        log += " fill=1 grad=2 text=3 sprite=4 blit=5 rstate=6"
        evidence = j2h_compat.evaluate_rei_gui_evidence(log)
        self.assertFalse(evidence["rei_config_screen_opened"])
        self.assertTrue(evidence["rei_config_screen_rendered"])
        self.assertEqual(evidence["rei_config_screen_render_commands"], {
            "fill": 1, "gradient": 2, "text": 3, "sprite": 4,
            "blit": 5, "render_state_text": 6,
        })

    def test_scroll_probe_ignores_changes_outside_mod_list(self):
        from PIL import Image

        with tempfile.TemporaryDirectory() as tmp:
            before = Path(tmp) / "before.png"
            after = Path(tmp) / "after.png"
            base = Image.new("RGB", (400, 300), "black")
            base.save(before)
            changed_detail = base.copy()
            changed_detail.paste("white", (250, 100, 390, 220))
            changed_detail.save(after)
            self.assertFalse(j2h_compat.screenshots_differ_in_mod_list(before, after))

            changed_list = base.copy()
            changed_list.paste("white", (20, 100, 180, 220))
            changed_list.save(after)
            self.assertTrue(j2h_compat.screenshots_differ_in_mod_list(before, after))

    def test_feature_probe_requires_entrypoint_and_assertion_markers(self):
        entry = {
            "mod_id": "jade_rule_probe",
            "expected_log_markers": ["ENTERED", "ASSERTED"],
            "verifies_mod": "jade",
            "verified_feature": "rule registry entry exists",
        }
        assertions, features = j2h_compat.evaluate_feature_probe(entry, "ENTERED\nASSERTED", {})
        self.assertTrue(assertions["jade_rule_probe_passed"])
        self.assertEqual(features, ["rule registry entry exists"])

        assertions, features = j2h_compat.evaluate_feature_probe(entry, "ENTERED", {})
        self.assertFalse(assertions["jade_rule_probe_passed"])
        self.assertEqual(features, [])

    def test_feature_probe_mod_failure_prevents_feature_credit(self):
        entry = {
            "mod_id": "jade_rule_probe",
            "expected_log_markers": ["ENTERED", "ASSERTED"],
            "verifies_mod": "jade",
            "verified_feature": "rule registry entry exists",
        }
        assertions, features = j2h_compat.evaluate_feature_probe(
            entry, "ENTERED\nASSERTED", {"jade_rule_probe": "failed after logging"}
        )
        self.assertFalse(assertions["jade_rule_probe_passed"])
        self.assertEqual(features, [])

    def test_catalog_and_scorecard_are_pinned_and_evidence_consistent(self):
        self.assertEqual(j2h_compat.validate_catalog(), 12)

    def test_required_dependencies_resolve_in_dependency_first_order(self):
        rei = [entry["mod_id"] for entry in j2h_compat.resolve_mods(["roughlyenoughitems"])]
        self.assertEqual(rei, ["cloth-config", "fabric-api", "architectury", "roughlyenoughitems"])
        iris = [entry["mod_id"] for entry in j2h_compat.resolve_mods(["iris"])]
        self.assertEqual(iris, ["fabric-api", "sodium", "iris"])

    def test_optional_jade_library_is_not_promoted_to_required_dependency(self):
        jade = [entry["mod_id"] for entry in j2h_compat.resolve_mods(["jade"])]
        self.assertEqual(jade, ["fabric-api", "jade"])
        self.assertEqual(j2h_compat.mods_by_id()["jade"]["optional_dependencies"],
                         ["u6dRKJwZ (not pinned in this starter catalog)"])

    def test_nested_cardinal_components_modules_pull_fabric_api(self):
        components = [entry["mod_id"] for entry in j2h_compat.resolve_mods(["cardinal-components"])]
        self.assertEqual(components, ["fabric-api", "cardinal-components"])

    def test_sodium_config_probe_resolves_sodium_after_fabric_api(self):
        entries = [entry["mod_id"] for entry in j2h_compat.resolve_mods(["sodium_config_probe"])]
        self.assertEqual(entries, ["fabric-api", "sodium", "sodium_config_probe"])

    def test_attachment_accessor_probe_resolves_fabric_api_and_architectury_first(self):
        entries = [entry["mod_id"] for entry in j2h_compat.resolve_mods(["architectury_payload_accessor_probe"])]
        self.assertEqual(entries, ["fabric-api", "architectury", "architectury_payload_accessor_probe"])

    def test_fixture_setup_file_is_staged_only_inside_fresh_game_dir(self):
        with tempfile.TemporaryDirectory() as tmp:
            game_dir = Path(tmp) / "game"
            game_dir.mkdir()
            entry = {
                "mod_id": "probe",
                "prelaunch_files": [{
                    "source": "sodium/sodium-options.json",
                    "destination": "config/sodium-options.json",
                }],
            }
            staged = j2h_compat.stage_fixture_files([entry], game_dir)
            self.assertEqual(staged, ["config/sodium-options.json"])
            config = j2h_compat.read_json(game_dir / staged[0])
            self.assertFalse(config["quality"]["hidden_fluid_culling"])

    def test_fixture_setup_rejects_paths_outside_isolated_game_and_fixture_roots(self):
        with tempfile.TemporaryDirectory() as tmp:
            game_dir = Path(tmp) / "game"
            game_dir.mkdir()
            entry = {
                "mod_id": "probe",
                "prelaunch_files": [{"source": "../../outside.json", "destination": "config/options.json"}],
            }
            with self.assertRaisesRegex(RuntimeError, "unsafe prelaunch file path"):
                j2h_compat.stage_fixture_files([entry], game_dir)

    def test_fixture_lock_preserves_feature_probe_metadata_and_dependencies(self):
        with tempfile.TemporaryDirectory() as tmp:
            lab = Path(tmp)
            (lab / "fixtures-lock.json").write_text(
                '{"schema":1,"fixtures":[{"fixture_id":"jade_rule_probe",'
                '"version":"1.0.0","filename":"jade_rule_probe.jar","sha512":"abc",'
                '"depends":["jade"],"scenario":"feature_probe",'
                '"expected_log_markers":["J2H_COMPAT_PROBE_ENTERED:jade_rule_probe",'
                '"J2H_COMPAT_ASSERTION:jade_rule_registered:default=21"]}]}'
            )
            original = j2h_compat.LAB
            j2h_compat.LAB = lab
            try:
                fixture = j2h_compat.all_entries()["jade_rule_probe"]
                self.assertEqual(fixture["requires"], ["jade"])
                self.assertEqual(fixture["scenario"], "feature_probe")
                self.assertEqual(
                    fixture["expected_log_markers"],
                    ["J2H_COMPAT_PROBE_ENTERED:jade_rule_probe",
                     "J2H_COMPAT_ASSERTION:jade_rule_registered:default=21"],
                )
            finally:
                j2h_compat.LAB = original

    def test_artifact_digest_check_rejects_mutated_bytes(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "artifact.jar"
            path.write_bytes(b"pinned bytes")
            entry = {"mod_id": "fixture", "sha512": j2h_compat.digest(path)}
            self.assertEqual(j2h_compat.verify_artifact(entry, path), entry["sha512"])
            path.write_bytes(b"changed bytes")
            with self.assertRaisesRegex(RuntimeError, "SHA-512 mismatch"):
                j2h_compat.verify_artifact(entry, path)

    def test_live_mod_failure_is_not_overwritten_by_successful_game_start(self):
        with tempfile.TemporaryDirectory() as tmp:
            scorecard = Path(tmp) / "scorecard.json"
            scorecard.write_text(
                '{"schema":1,"results":[{"mod_id":"broken","status":"DISCOVERED",'
                '"diagnostics":[],"evidence":[],"regression_history":[],"features_verified":[]}]}'
            )
            original = j2h_compat.SCORECARD
            j2h_compat.SCORECARD = scorecard
            try:
                j2h_compat.update_scorecard(
                    [{"mod_id": "broken"}],
                    {
                        "run_id": "live-failure",
                        "runtime_started": True,
                        "mod_failures": {"broken": "entrypoint threw"},
                        "dependency_skips": {},
                        "access_widener_transformations": [],
                        "j2h_pipeline_failure": None,
                    },
                )
                result = j2h_compat.read_json(scorecard)["results"][0]
                self.assertEqual(result["status"], "BASELINE_FAILED")
                self.assertIn("entrypoint threw", result["diagnostics"])
            finally:
                j2h_compat.SCORECARD = original

    def test_gui_regression_failure_is_not_promoted_by_successful_game_start(self):
        with tempfile.TemporaryDirectory() as tmp:
            scorecard = Path(tmp) / "scorecard.json"
            scorecard.write_text(
                '{"schema":1,"results":[{"mod_id":"modmenu","status":"VERIFIED_COMPATIBLE",'
                '"diagnostics":[],"evidence":[],"regression_history":[],"features_verified":[]}]}'
            )
            original = j2h_compat.SCORECARD
            j2h_compat.SCORECARD = scorecard
            try:
                j2h_compat.update_scorecard(
                    [{"mod_id": "modmenu"}],
                    {
                        "run_id": "gui-failure",
                        "runtime_started": True,
                        "mod_failures": {},
                        "dependency_skips": {},
                        "access_widener_transformations": [],
                        "j2h_pipeline_failure": None,
                        "assertions": {"modmenu_search_filters_to_zero": False},
                    },
                )
                result = j2h_compat.read_json(scorecard)["results"][0]
                self.assertEqual(result["status"], "BASELINE_FAILED")
            finally:
                j2h_compat.SCORECARD = original

    def test_live_feature_probe_marks_only_the_verified_mod_feature_partial(self):
        with tempfile.TemporaryDirectory() as tmp:
            scorecard = Path(tmp) / "scorecard.json"
            scorecard.write_text(
                '{"schema":1,"results":[{"mod_id":"jade","status":"IN_PROGRESS",'
                '"diagnostics":[],"evidence":[],"regression_history":[],"features_verified":[]}]} '
            )
            original = j2h_compat.SCORECARD
            j2h_compat.SCORECARD = scorecard
            try:
                j2h_compat.update_scorecard(
                    [{"mod_id": "jade"}],
                    {
                        "run_id": "jade-rule-probe",
                        "runtime_started": True,
                        "mod_failures": {},
                        "dependency_skips": {},
                        "access_widener_transformations": [],
                        "j2h_pipeline_failure": None,
                        "features_verified_by_mod": {
                            "jade": ["GAME_RULE contains jade:max_position_deviation"]
                        },
                    },
                )
                result = j2h_compat.read_json(scorecard)["results"][0]
                self.assertEqual(result["status"], "PARTIALLY_COMPATIBLE")
                self.assertEqual(result["features_verified"], ["GAME_RULE contains jade:max_position_deviation"])
            finally:
                j2h_compat.SCORECARD = original

    def test_modmenu_combination_regression_gets_combination_status(self):
        with tempfile.TemporaryDirectory() as tmp:
            scorecard = Path(tmp) / "scorecard.json"
            scorecard.write_text(
                '{"schema":1,"results":[{"mod_id":"modmenu","status":"IN_PROGRESS",'
                '"diagnostics":[],"evidence":[],"regression_history":[],"features_verified":[]}]}'
            )
            original = j2h_compat.SCORECARD
            j2h_compat.SCORECARD = scorecard
            try:
                assertions = {f"modmenu_{name}": True for name in (
                    "opens", "button_labels_render", "search_filters_to_zero",
                    "search_restores_result", "scroll_changes_visible_content",
                    "escape_closes", "reopens",
                )}
                j2h_compat.update_scorecard(
                    [{"mod_id": "modmenu"}, {"mod_id": "sodium"}],
                    {
                        "run_id": "modmenu-with-sodium",
                        "runtime_started": True,
                        "mod_failures": {},
                        "dependency_skips": {},
                        "access_widener_transformations": [],
                        "j2h_pipeline_failure": None,
                        "assertions": assertions,
                    },
                )
                result = j2h_compat.read_json(scorecard)["results"][0]
                self.assertEqual(result["status"], "VERIFIED_IN_COMBINATION")
                self.assertTrue(any("with: sodium" in x for x in result["features_verified"]))
            finally:
                j2h_compat.SCORECARD = original


if __name__ == "__main__":
    unittest.main()
