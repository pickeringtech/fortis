from contextlib import redirect_stdout
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import gate
import unit_check


class Gate(unittest.TestCase):
    def test_quick_includes_units_and_cargo(self):
        steps = gate.tier_steps("quick")
        self.assertEqual(steps[0][0], "unit")
        self.assertIn("cargo-test", [s[0] for s in steps])
        self.assertTrue(gate.python_only("unit"))

    def test_ci_shards_cover_units_and_run_them_first(self):
        shards = gate.plan_shards("quick", 14, 3, 100)
        self.assertEqual(sum(name == "unit" and role == "own" for shard in shards
                             for name, _, _, role in shard), 1)
        for shard in shards:
            self.assertEqual(shard[0][0], "unit")
            self.assertNotIn("cargo-test", [s[0] for s in shard])

    def test_unit_failure_stops_sequential_and_parallel_gate(self):
        for jobs in (1, 4):
            calls = []
            def run(tree, name, cmd, minutes):
                calls.append(name)
                return {"step": name, "command": cmd, "status": "fail (exit 1)", "seconds": 0, "tail": ["broken unit"]}
            with self.subTest(jobs=jobs), tempfile.TemporaryDirectory() as tmp:
                report = Path(tmp) / "gate.json"
                with patch.object(gate, "run_step", side_effect=run), redirect_stdout(io.StringIO()):
                    code = gate.main(["--tree", tmp, "--tier", "quick", "--jobs", str(jobs),
                                      "--only", "unit,hygiene", "--json", str(report)])
                self.assertEqual(code, 1)
                self.assertEqual(calls, ["unit"])
                data = json.loads(report.read_text())
                self.assertEqual(data["verdict"], "FAIL")
                self.assertEqual(data["steps"][1]["status"], "not run (unit failed)")


class Runner(unittest.TestCase):
    def run_suite(self, suite):
        out = io.StringIO()
        with patch.object(unit_check.unittest.defaultTestLoader, "discover", return_value=suite):
            with redirect_stdout(out), patch("sys.stderr", new=io.StringIO()):
                code = unit_check.main(["--json"])
        return code, json.loads(out.getvalue())

    def test_empty_discovery_fails(self):
        code, report = self.run_suite(unittest.TestSuite())
        self.assertEqual((code, report["tests"]), (1, 0))

    def test_test_output_keeps_json_summary_parseable(self):
        suite = unittest.TestSuite([unittest.FunctionTestCase(lambda: print("test diagnostic"))])
        code, report = self.run_suite(suite)
        self.assertEqual((code, report["tests"]), (0, 1))

    def test_failure_exits_nonzero(self):
        def fail():
            raise AssertionError("deliberate failure")
        code, report = self.run_suite(unittest.TestSuite([unittest.FunctionTestCase(fail)]))
        self.assertEqual((code, report["tests"], report["failures"]), (1, 1, 1))

    def test_discovery_import_error_exits_nonzero(self):
        with tempfile.TemporaryDirectory() as tmp:
            Path(tmp, "test_broken.py").write_text("raise ImportError('broken module')\n")
            suite = unittest.TestLoader().discover(tmp)
            code, report = self.run_suite(suite)
        self.assertEqual((code, report["errors"]), (1, 1))
