"""X-149: parser correctness always gates; timing is opt-in for the v1.0 check."""
from contextlib import redirect_stdout
import io
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from support import ROOT
import genparser_check


class ParserTiming(unittest.TestCase):
    def run_check(self, timing="", defect=None, milliseconds=60):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            grammar = root / "reference/grammar/fortis.ebnf"
            grammar.parent.mkdir(parents=True)
            grammar.symlink_to(ROOT / "reference/grammar/fortis.ebnf")
            generated = root / "conformance/generated"
            generated.mkdir(parents=True)
            (root / "sample.ft").write_text("// accept\nfunc f() {}\n")
            (root / "conformance/results.json").write_text(json.dumps({"results": [
                {"id": "sample", "path": "sample.ft", "detail": "module"}
            ]}))
            ambiguity = root / "ambiguity"
            if defect == "ambiguity":
                ambiguity.mkdir()
                (ambiguity / "choice.ft").write_text("// accept\nfunc f() {}\n")
            real_parse = genparser_check.gen_parse

            def parse(tokens, start):
                ok, parser, why = real_parse(tokens, start)
                if defect == "acceptance":
                    return False, parser, "simulated disagreement"
                if defect == "tree":
                    parser.tree = ("different_rule", [])
                if defect == "ambiguity":
                    parser.fallbacks = 1
                return ok, parser, why

            output = io.StringIO()
            with patch.object(genparser_check, "ROOT", tmp), \
                 patch.object(genparser_check, "AMB", str(ambiguity)), \
                 patch.object(genparser_check, "gen_parse", side_effect=parse), \
                 patch.object(genparser_check.time, "perf_counter", side_effect=[0, milliseconds / 1000] * 20), \
                 patch.dict(os.environ, {"FORTIS_PARSER_TIMING": timing}), redirect_stdout(output):
                code = genparser_check.main([])
            return code, json.loads((generated / "check.json").read_text()), output.getvalue()

    def test_slow_parse_is_recorded_without_failing_before_v1(self):
        code, report, output = self.run_check()
        self.assertEqual(code, 0)
        self.assertFalse(report["timing_enforced"])
        self.assertEqual(report["timing_budget_ms"], 50)
        self.assertEqual(report["timing"][0]["median_ms"], 60)
        self.assertEqual(report["timing"][0]["p95_ms"], 60)
        self.assertIn("recorded only", output)

    def test_v1_timing_opt_in_enforces_the_same_budget(self):
        for milliseconds, expected in ((49, 0), (50, 1), (60, 1)):
            with self.subTest(milliseconds=milliseconds):
                code, report, output = self.run_check("1", milliseconds=milliseconds)
                self.assertEqual(code, expected)
                self.assertTrue(report["timing_enforced"])
                self.assertIn("timing budget: enforced", output)

    def test_correctness_failures_always_fail(self):
        for timing in ("", "1"):
            for defect in ("acceptance", "tree", "ambiguity"):
                with self.subTest(timing=timing, defect=defect):
                    code, report, _ = self.run_check(timing, defect, milliseconds=1)
                    self.assertEqual(code, 1)
                    if defect == "acceptance":
                        self.assertFalse(report["agreement"][0]["agree"])
                    elif defect == "tree":
                        self.assertFalse(report["agreement"][0]["same_tree"])
                    else:
                        self.assertFalse(report["ambiguity"][0]["ok"])
