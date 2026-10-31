"""D-730: canonical source checks use the real CLI contract and cover guide packages."""
import unittest
from pathlib import Path
from subprocess import CompletedProcess
from unittest.mock import patch
from support import table_tests
import docset_check
import fmt_check


def check(test, data, expect):
    result = CompletedProcess([], data["code"], data["stdout"], data["stderr"])
    with patch.object(fmt_check.subprocess, "run", return_value=result) as run:
        row = fmt_check.canonical("example.ft", "guide#1")
    test.assertEqual(row["ok"], expect)
    test.assertEqual(row["exit"], data["code"])
    test.assertEqual(run.call_args.args[0], [fmt_check.FORTISC, "fmt", "--check", "example.ft"])
    test.assertGreater(run.call_args.kwargs["timeout"], 0)


class Canonical(unittest.TestCase):
    def test_shipped_inventory_and_guide_context(self):
        fixtures = [dict(id=f"docs/guide/a.md#{i}", page="docs/guide/a.md", verdict=v, codes=c, src="// test\n")
                    for i, v, c in [(1, "accept", []), (2, "reject", ["E-TYP-040"]),
                                    (3, "illustrative", []), (4, "reject", ["E-SYN-001"])]]
        fixtures.append(dict(id="docs/guide/a.md#shop", page="docs/guide/a.md", verdict="accept",
                             package="shop", files={"shop/src/main.ft": "// main\n", "shop/src/model.ft": "// model\n"},
                             manifest="package: shop\n", blocks=[dict(codes=[])]))
        seen = []
        def canonical(path, label):
            seen.append(label)
            if label.endswith("#shop"):
                root = Path(path)
                self.assertEqual((root / "project.yaml").read_text(), "package: shop\n")
                self.assertEqual((root / "src/model.ft").read_text(), "// model\n")
            return dict(path=label, ok=True, detail="")
        with patch.object(docset_check, "doc_pages", return_value=["README.md", "docs/guide/a.md", "docs/agent/a.md"]), \
                patch.object(docset_check, "collect", return_value=(fixtures, [])) as collect, \
                patch.object(fmt_check, "canonical", side_effect=canonical):
            rows = fmt_check.shipped_sources()
        collect.assert_called_once_with(["docs/guide/a.md"])
        self.assertEqual(seen, ["examples", "pilot/ledger", "docs/guide/a.md#1", "docs/guide/a.md#2", "docs/guide/a.md#shop"])
        self.assertTrue(all(row["ok"] for row in rows))

    def test_malformed_guide_is_a_failure(self):
        with patch.object(docset_check, "doc_pages", return_value=[]), \
                patch.object(docset_check, "collect", return_value=([], [dict(id="guide#1", detail="unclosed fence")])), \
                patch.object(fmt_check, "canonical", return_value=dict(ok=True)):
            rows = fmt_check.shipped_sources()
        self.assertEqual(rows[-1], dict(path="guide#1", ok=False, detail="unclosed fence"))


table_tests(Canonical, "formatter/canonical.json", check)
