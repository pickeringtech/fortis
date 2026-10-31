import support
import importlib.util
import unittest
from unittest.mock import patch
from pathlib import Path

path = support.ROOT / "docs/delivery/tools/limitations.py"
if not path.exists():
    raise unittest.SkipTest("maintainers' limitation generator is absent from the public tree")
spec = importlib.util.spec_from_file_location("mopup_limitations", path)
limitations = importlib.util.module_from_spec(spec)
spec.loader.exec_module(limitations)

class Limitations(unittest.TestCase):
    def test_open_release_defect_keeps_its_workaround(self):
        defects = {"D-999": {"release": "v0.1", "status": "open", "kind": "other", "summary": "A valid call is rejected.", "workaround": "Use a loop."}}
        self.assertEqual(limitations.extra_rows(defects, {}, {}, set()), [("checker", "D-999", "A valid call is rejected.", "Use a loop.")])

    def test_fixed_defect_and_future_release_are_not_listed(self):
        defects = {"D-998": {"release": "v0.1", "status": "fixed", "kind": "other"}, "D-999": {"release": "v0.4", "status": "open", "kind": "other"}}
        self.assertEqual(limitations.extra_rows(defects, {}, {}, set()), [])

    def test_triage_subentry_uses_parent_workaround(self):
        defects = {"D-999": {"status": "open", "kind": "other", "workaround": "Bind the value first."}}
        entries = {"D-999.1": {"release": "v0.1", "summary": "A nested value is rejected."}}
        self.assertEqual(limitations.extra_rows(defects, entries, {}, set())[0][3], "Bind the value first.")


class CarriedLimitations(unittest.TestCase):
    def test_previous_release_disagreement_requires_a_row(self):
        defects = {"D-750": {"status": "open", "kind": "backends disagree", "carried_from": "v0.0.2"}}
        with patch.object(limitations, "CARRIED", []):
            self.assertEqual(len(limitations.carried_problems(defects)), 1)
        with patch.object(limitations, "CARRIED", [("D-750", "failure", "workaround")]):
            self.assertEqual(limitations.carried_problems(defects), [])

    def test_fixed_defect_cannot_remain_carried(self):
        defects = {"D-750": {"status": "fixed", "kind": "backends disagree", "carried_from": "v0.0.2"}}
        with patch.object(limitations, "CARRIED", [("D-750", "failure", "workaround")]):
            self.assertEqual(len(limitations.carried_problems(defects)), 1)

    def test_existing_summary_marker_still_requires_a_row(self):
        defects = {"old": {"status": "open", "kind": "wrong-result", "summary": "Pre-existing since v0.0.1"}}
        with patch.object(limitations, "CARRIED", []):
            self.assertEqual(len(limitations.carried_problems(defects)), 1)
