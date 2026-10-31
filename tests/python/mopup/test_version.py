import subprocess
import tomllib
import unittest
from support import ROOT

class Version(unittest.TestCase):
    def test_cli_version_matches_workspace(self):
        version = tomllib.loads((ROOT / "compiler/Cargo.toml").read_text())["workspace"]["package"]["version"]
        result = subprocess.run([str(ROOT / "bin/fortis"), "--version"], capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, f"fortis {version}\n")
        self.assertEqual(result.stderr, "")
