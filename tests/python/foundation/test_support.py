import io
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from support import forall, read_cases, table_tests


class Properties(unittest.TestCase):
    def test_seed_replays(self):
        def sample():
            seen = []
            forall(lambda r: r.randint(-1000, 1000), cases=8, seed=42)(seen.append)()
            return seen
        with patch.dict(os.environ, {}, clear=True):
            self.assertEqual(sample(), sample())
            self.assertEqual(len(sample()), 8)

    def test_environment_overrides_seed(self):
        def sample(seed):
            seen = []
            forall(lambda r: r.getrandbits(64), cases=4, seed=seed)(seen.append)()
            return seen
        with patch.dict(os.environ, {"FORTIS_TEST_SEED": "17"}):
            self.assertEqual(sample(1), sample(2))

    def test_failure_reports_seed_and_value(self):
        @forall(lambda r: {"bound": 17}, cases=1, seed=42)
        def fail(value):
            raise ValueError("broken check")
        with patch.dict(os.environ, {}, clear=True):
            with self.assertRaisesRegex(AssertionError, "FORTIS_TEST_SEED=42 case=0 value=.*17.*broken check"):
                fail()

    def test_zero_cases_rejected(self):
        with self.assertRaises(ValueError):
            forall(lambda r: 0, cases=0)


class Tables(unittest.TestCase):
    def read(self, text):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "cases.json"
            path.write_text(text, encoding="utf-8")
            return read_cases(path)

    def test_supported_values(self):
        expected = [{"name": "values", "input": [None, True, -9223372036854775808, "🙂"], "expect": {"ok": 3}}]
        self.assertEqual(self.read(json.dumps(expected)), expected)

    def test_invalid_schema_rejected(self):
        for text in ('[]', '{}', '[{"name":"a","input":0}]',
                     '[{"name":"A","input":0,"expect":0}]',
                     '[{"name":"a","input":1.5,"expect":0}]',
                     '[{"name":"a","input":9223372036854775808,"expect":0}]'):
            with self.subTest(text=text), self.assertRaises(ValueError):
                self.read(text)

    def test_duplicate_names_rejected(self):
        row = '{"name":"same","input":0,"expect":0}'
        with self.assertRaisesRegex(ValueError, "duplicate case name"):
            self.read(f"[{row},{row}]")

    def test_named_rows_are_independent_tests(self):
        class Sample(unittest.TestCase):
            pass
        rows = [{"name": "good", "input": 1, "expect": 1},
                {"name": "bad", "input": 1, "expect": 2}]
        with patch("support.read_cases", return_value=rows):
            table_tests(Sample, "unused", lambda test, x, y: test.assertEqual(x, y))
        result = unittest.TextTestRunner(stream=io.StringIO()).run(unittest.defaultTestLoader.loadTestsFromTestCase(Sample))
        self.assertEqual(result.testsRun, 2)
        self.assertEqual(len(result.failures), 1)
        self.assertIn("test_bad", result.failures[0][0].id())
