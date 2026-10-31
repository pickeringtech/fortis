from contextlib import redirect_stdout
import copy
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import ci_plan
import gate
from support import ROOT, table_tests


class Cases(unittest.TestCase):
    pass


def check_case(test, source, expected):
    result = ci_plan.plan(source["paths"], source.get("cited", ()), source.get("full", False))
    test.assertEqual({k: result[k] for k in expected}, expected)
    test.assertEqual(len(result["reasons"]), len(set(source["paths"])) or 1)


table_tests(Cases, "ci/plans.json", check_case)


class Diff(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.git("init", "-q")
        self.git("config", "user.name", "CI test")
        self.git("config", "user.email", "ci@example.invalid")

    def git(self, *args):
        return ci_plan.git(self.root, *args).decode().strip()

    def write(self, path, text="fixture\n"):
        file = self.root / path
        file.parent.mkdir(parents=True, exist_ok=True)
        file.write_text(text)

    def commit(self):
        self.git("add", ".")
        self.git("commit", "-qm", "fixture")
        return self.git("rev-parse", "HEAD")

    def test_rename_includes_old_unknown_path_and_new_document(self):
        self.write("compiler/old.rs")
        base = self.commit()
        self.git("mv", "compiler/old.rs", "CONTRIBUTING.md")
        head = self.commit()
        paths, merge_base = ci_plan.changed_paths(self.root, base, head)
        self.assertEqual(paths, ["CONTRIBUTING.md", "compiler/old.rs"])
        self.assertEqual(merge_base, base)
        self.assertIn("full", ci_plan.plan(paths)["classes"])

    def test_deletions_and_newline_paths_are_not_lost(self):
        self.write("tests/deleted.py")
        self.write("unknown\npath.rs")
        base = self.commit()
        self.git("rm", "tests/deleted.py", "unknown\npath.rs")
        head = self.commit()
        paths, _ = ci_plan.changed_paths(self.root, base, head)
        self.assertEqual(paths, ["tests/deleted.py", "unknown\npath.rs"])
        self.assertIn("full", ci_plan.plan(paths)["classes"])

    def test_merge_base_excludes_changes_only_on_base_branch(self):
        self.write("AGENTS.md")
        self.commit()
        self.git("checkout", "-qb", "base")
        self.write("compiler/base_only.rs")
        base = self.commit()
        self.git("checkout", "-qb", "topic", "HEAD~1")
        self.write("CONTRIBUTING.md")
        head = self.commit()
        paths, _ = ci_plan.changed_paths(self.root, base, head)
        self.assertEqual(paths, ["CONTRIBUTING.md"])

    def test_local_plan_includes_staged_unstaged_and_untracked(self):
        self.write("AGENTS.md")
        base = self.commit()
        self.write("AGENTS.md", "changed\n")
        self.write("CONTRIBUTING.md")
        self.git("add", "CONTRIBUTING.md")
        self.write("new.py")
        paths, _ = ci_plan.changed_paths(self.root, base)
        self.assertEqual(paths, ["AGENTS.md", "CONTRIBUTING.md", "new.py"])

    def test_deleted_and_directory_citations_add_docs_checks(self):
        self.write("docs/note.md", "[tests](../tests/unit/) [old](../tests/deleted.py#heading)\n")
        self.commit()
        paths = ["tests/unit/new.py", "tests/deleted.py", "tests/uncited.py"]
        self.assertEqual(ci_plan.cited_tests(self.root, paths), set(paths[:2]))

    def test_missing_base_fails_instead_of_narrowing(self):
        with self.assertRaises(subprocess.CalledProcessError):
            ci_plan.changed_paths(self.root, "missing")


class Plans(unittest.TestCase):
    def test_cli_publishes_the_same_json_plan_and_job_matrices(self):
        for plan in (ci_plan.plan(["AGENTS.md"]), ci_plan.plan(["README.md"]), ci_plan.plan(["tests/new.py"])):
            with tempfile.TemporaryDirectory() as tmp, patch.object(ci_plan, "from_diff", return_value=plan), \
                    redirect_stdout(io.StringIO()):
                output, report = Path(tmp, "output"), Path(tmp, "plan.json")
                code = ci_plan.main(["--base", "base", "--head", "head", "--github-output", str(output), "--json", str(report)])
                values = dict(line.split("=", 1) for line in output.read_text().splitlines())
                self.assertEqual(code, 0)
                self.assertEqual(json.loads(report.read_text()), plan)
                self.assertEqual(json.loads(values["plan"]), plan)
                self.assertEqual(json.loads(values["build"]), plan["build"])
                self.assertEqual(json.loads(values["rust_tests"]), plan["rust_tests"])
                self.assertEqual(json.loads(values["python_matrix"]) + json.loads(values["toolchain_matrix"]),
                                 list(range(1, plan["shards"] + 1)))

    def test_ci_shard_cli_records_the_plan_and_combines(self):
        plan = ci_plan.plan(["AGENTS.md"])
        calls = []
        def run(tree, name, command, minutes):
            calls.append(name)
            return {"step": name, "status": "pass", "seconds": 0, "tail": []}
        with tempfile.TemporaryDirectory() as tmp, patch.object(gate, "run_step", side_effect=run), \
                redirect_stdout(io.StringIO()):
            report = Path(tmp, "shard.json")
            args = ["--tree", tmp, "--tier", "quick", "--ci-plan", json.dumps(plan)]
            self.assertEqual(gate.main(args + ["--shard", "1/1", "--json", str(report)]), 0)
            self.assertEqual(gate.main(args + ["--combine", str(report)]), 0)
            self.assertEqual(json.loads(report.read_text())["plan"], plan)
        self.assertEqual(calls, plan["steps"])

    def reports(self, plan):
        shards = gate.plan_shards("quick", plan["shards"], plan["py_shards"], plan["build_seconds"], plan["steps"])
        return [{"tier": "quick", "commit": "same", "verdict": "PASS", "plan": plan,
                 "shard": f"{i}/{len(shards)}", "steps": [
                     {"step": nm, "status": "pass", "seconds": 1, "role": role}
                     for nm, _, _, role in shard]} for i, shard in enumerate(shards, 1)]

    def combine(self, reports, plan):
        with tempfile.TemporaryDirectory() as tmp:
            paths = []
            for i, report in enumerate(reports):
                path = Path(tmp, f"{i}.json")
                path.write_text(json.dumps(report))
                paths.append(str(path))
            with redirect_stdout(io.StringIO()):
                return gate.combine(paths, plan)[0]

    def test_every_class_combines_and_full_split_is_unchanged(self):
        for path in ("AGENTS.md", "README.md", "tests/new.py", "compiler/new.rs"):
            with self.subTest(path=path):
                plan = ci_plan.plan([path])
                for step in plan["steps"]:
                    self.assertTrue(set(gate.PREREQS.get(step, ())) <= set(plan["steps"]))
                self.assertEqual(self.combine(self.reports(plan), plan), "PASS")
        full = ci_plan.plan(["compiler/new.rs"])
        self.assertEqual(gate.plan_shards("quick", 14, 3, 100, full["steps"]), gate.plan_shards("quick", 14, 3, 100))

    def test_combine_rejects_missing_duplicate_extra_failed_and_skipped_steps(self):
        plan = ci_plan.plan(["AGENTS.md"])
        for fault in ("missing", "duplicate", "extra", "fail", "skipped", "plan", "verdict", "role", "tier", "no_reports"):
            reports = copy.deepcopy(self.reports(plan))
            rows = reports[0]["steps"]
            if fault == "missing":
                rows.pop()
            elif fault == "duplicate":
                rows.append(copy.deepcopy(rows[0]))
            elif fault == "extra":
                rows.append({"step": "not-a-step", "status": "pass", "seconds": 1, "role": "own"})
            elif fault in ("fail", "skipped"):
                rows[0]["status"] = fault
            elif fault == "plan":
                reports[0]["plan"]["steps"].remove("plan")
            elif fault == "verdict":
                reports[0]["verdict"] = "FAIL"
            elif fault == "role":
                rows[0]["role"] = "prerequisite"
            elif fault == "tier":
                reports[0]["tier"] = "full"
            else:
                reports = []
            with self.subTest(fault=fault):
                self.assertEqual(self.combine(reports, plan), "FAIL")

    def test_split_step_missing_part_or_shard_fails(self):
        plan = ci_plan.plan(["README.md"])
        reports = self.reports(plan)
        self.assertEqual(self.combine(reports[:-1], plan), "FAIL")
        part = next(r for r in reports if any(s["step"].startswith("docset[") for s in r["steps"]))
        part["steps"] = [s for s in part["steps"] if not s["step"].startswith("docset[")]
        self.assertEqual(self.combine(reports, plan), "FAIL")

    def test_local_changed_runs_exact_plan_without_a_build(self):
        plan = ci_plan.plan(["AGENTS.md"])
        calls = []
        def run(tree, name, command, minutes):
            calls.append(name)
            return {"step": name, "status": "pass", "seconds": 0, "tail": []}
        with tempfile.TemporaryDirectory() as tmp, patch.object(ci_plan, "from_diff", return_value=plan), \
                patch.object(gate, "run_step", side_effect=run), redirect_stdout(io.StringIO()) as output:
            code = gate.main(["--tree", tmp, "--tier", "quick", "--changed", "origin/master", "--jobs", "1"])
        self.assertEqual(code, 0)
        self.assertEqual(calls, plan["steps"])
        self.assertIn("change plan: process", output.getvalue())

    def test_document_inventories_cannot_silently_outgrow_the_table(self):
        import docset_check
        import extract
        spec = importlib.util.spec_from_file_location("ci_website", ROOT / "website" / "build.py")
        website = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(website)
        paths = set(docset_check.doc_pages()) | set(extract.DOCS.values())
        paths |= set(website.rendered_pages()) | set(website.docsite.pages())
        paths |= {note["source"].split("#")[0] for note in json.loads((ROOT / "website/src/notes.json").read_text())}
        for path in paths:
            with self.subTest(path=path):
                self.assertEqual(ci_plan.classify(path)[0], "documentation")
