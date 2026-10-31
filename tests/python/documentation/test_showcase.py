"""Cheap checks of the documentation harness; backend execution stays in the gate."""
import support

from contextlib import redirect_stdout
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import docset_check as docset
import gate
import showcase_check as showcase


class VmOutput(unittest.TestCase):
    def result(self, code, output, metadata):
        with patch.object(docset, "run", return_value=(code, output, metadata)) as run:
            result = docset.run_backend("vm", "example.ft", False, "example")
        self.assertIn("--stdout", run.call_args.args[0])
        return result

    def test_console_text_is_preserved(self):
        output = "tea | cake\none\ntwo\n\nobserved: refused:literal\n"
        self.assertEqual(self.result(0, output, "trace: \n")[:2], (0, output))

    def test_failing_main_keeps_its_status_and_output(self):
        self.assertEqual(self.result(1, "before failure\n", "trace: enter|fail\n")[:2],
                         (1, "before failure\n"))

    def test_refusal_is_not_a_failing_main(self):
        self.assertEqual(self.result(1, "", "observed: refused:E-TYP-040\n")[0], 4)

    def test_defect_uses_the_documentation_exit_convention(self):
        self.assertEqual(self.result(101, "", "trace: defect\n")[0], 3)

    def test_dynamic_stop_or_tool_error_cannot_pass_as_a_program_failure(self):
        for code in (2, 7, -9):
            with self.subTest(code=code):
                self.assertEqual(self.result(code, "", "trace: stopped\n")[0], 4)


class Showcase(unittest.TestCase):
    SOURCE = '// accept\nfunc main() {}\ntest "example" { expect true }\n'
    OUTPUT = "tea | cake\n\n"

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.program = self.root / "examples/showcase/v0.1.0-coffee.ft"
        self.program.parent.mkdir(parents=True)
        self.program.write_text(self.SOURCE)
        self.program.with_suffix(".stdout").write_text(self.OUTPUT)
        self.page = self.root / "docs/guide/worked-example.md"
        self.page.parent.mkdir(parents=True)
        self.page.write_text(self.guide())
        (self.root / "build").mkdir()

    def guide(self, source=None, output=None):
        source = self.SOURCE if source is None else source
        output = self.OUTPUT if output is None else output
        return f"```fortis\n{source}```\n\n```output\n{output}```\n"

    def run_showcase(self, backend_ok=True):
        output = io.StringIO()
        with patch.object(showcase, "ROOT", self.root), patch.object(docset, "WORK", str(self.root / "build/docset")), \
                patch.object(showcase.sys, "argv", ["showcase_check.py", "--json"]), \
                patch.object(docset, "judge", return_value={"ok": backend_ok, "checks": []}) as judge, \
                redirect_stdout(output):
            code = showcase.main()
        report = json.loads((self.root / "build/showcase.json").read_text())
        return code, report, judge

    def test_matching_source_and_exact_output_reach_backend_checks(self):
        code, report, judge = self.run_showcase()
        self.assertEqual((code, report["problems"]), (0, []))
        judge.assert_called_once()
        fixture = judge.call_args.args[0]
        self.assertEqual(fixture["src"], self.SOURCE)
        self.assertEqual(fixture["expect"], ("output", "tea | cake\n"))

    def test_no_programs_is_a_failure(self):
        self.program.unlink()
        code, report, judge = self.run_showcase()
        self.assertEqual(code, 1)
        self.assertIn("no pinned showcase programs", report["problems"])
        judge.assert_not_called()

    def test_missing_stdout_does_not_run_unpinned_program(self):
        self.program.with_suffix(".stdout").unlink()
        code, report, judge = self.run_showcase()
        self.assertEqual(code, 1)
        self.assertTrue(any("missing .stdout" in p for p in report["problems"]))
        judge.assert_not_called()

    def test_changed_or_duplicated_guide_source_fails(self):
        for page in (self.guide(source=self.SOURCE + "// changed\n"), self.guide() * 2):
            with self.subTest(page=page):
                self.page.write_text(page)
                code, report, _ = self.run_showcase()
                self.assertEqual(code, 1)
                self.assertTrue(any("exactly once" in p for p in report["problems"]))

    def test_changed_guide_output_fails(self):
        self.page.write_text(self.guide(output="different\n"))
        code, report, _ = self.run_showcase()
        self.assertEqual(code, 1)
        self.assertTrue(any("output differs" in p for p in report["problems"]))

    def test_empty_source_is_reported_without_crashing(self):
        self.program.write_text("")
        code, report, _ = self.run_showcase()
        self.assertEqual(code, 1)
        self.assertTrue(any("first line" in p for p in report["problems"]))

    def test_missing_main_or_tests_fails(self):
        for source in ('// accept\nfunc main() {}\n', '// accept\ntest "x" { expect true }\n'):
            with self.subTest(source=source):
                self.program.write_text(source)
                self.page.write_text(self.guide(source=source))
                code, report, _ = self.run_showcase()
                self.assertEqual(code, 1)
                self.assertTrue(any("main and tests" in p for p in report["problems"]))

    def test_backend_failure_fails_the_showcase(self):
        self.assertEqual(self.run_showcase(backend_ok=False)[0], 1)

    def test_quick_gate_keeps_full_showcase_and_one_owning_shard(self):
        self.assertIn("showcase", [s[0] for s in gate.tier_steps("quick")])
        self.assertNotIn("showcase", gate.QUICK_FORMS)
        self.assertIn("showcase", gate.NATIVE_LANE)
        shards = gate.plan_shards("quick", 14, 3, 100)
        self.assertEqual(sum(name == "showcase" and role == "own" for shard in shards
                             for name, _, _, role in shard), 1)
