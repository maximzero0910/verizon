import importlib.util
import json
import os
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).resolve().parents[1] / "tools" / "j2h_patcher.py"
SPEC = importlib.util.spec_from_file_location("j2h_patcher", SCRIPT)
patcher = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(patcher)


class J2HPatcherTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.client = self.root / "horizon-client"
        self.client.write_text(
            "#!/usr/bin/env python3\n"
            "import json,sys\n"
            "print(json.dumps({'runtime':'java2horizon','minecraft_version':'26.3','profile_schema':1}))\n"
            "\n",
            encoding="utf-8",
        )
        self.client.chmod(0o755)
        self.resources = self.root / "resources"
        self.resources.mkdir()
        (self.resources / "horizon-resources.json").write_text("{}", encoding="utf-8")
        self.profile = self.root / "profile"

    def tearDown(self):
        self.temp.cleanup()

    def test_client_capability_requires_j2h_marker(self):
        self.assertEqual(patcher.verified_client(self.client), self.client)

    def test_install_creates_isolated_empty_mod_profile_without_touching_source(self):
        args = type("Args", (), {
            "client": str(self.client), "resources": str(self.resources),
            "profile": str(self.profile), "mods": [],
        })()
        patcher.install(args)
        info = json.loads((self.profile / patcher.MARKER).read_text(encoding="utf-8"))
        self.assertEqual(info["runtime"], "java2horizon")
        self.assertEqual(info["mods"], [])
        self.assertTrue((self.profile / info["client"]).is_file())
        self.assertTrue((self.profile / "game" / "mods").is_dir())
        self.assertTrue(self.client.is_file())
        self.assertEqual(patcher.read_profile(self.profile)[0], info)

    def test_install_refuses_nonempty_destination(self):
        self.profile.mkdir()
        (self.profile / "keep.txt").write_text("user data", encoding="utf-8")
        args = type("Args", (), {
            "client": str(self.client), "resources": str(self.resources),
            "profile": str(self.profile), "mods": [],
        })()
        with self.assertRaisesRegex(RuntimeError, "already exists"):
            patcher.install(args)
        self.assertEqual((self.profile / "keep.txt").read_text(encoding="utf-8"), "user data")


if __name__ == "__main__":
    unittest.main()
