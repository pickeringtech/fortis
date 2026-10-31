#!/usr/bin/env python3
"""Discover and run Python unit tests in-process, including under python3 -I."""
import argparse
from contextlib import redirect_stdout
import json
from pathlib import Path
import sys
import time
import unittest

ROOT = Path(__file__).resolve().parents[2]
# -I deliberately supplies no script directory. support adds the repository root.
sys.path.insert(0, str(ROOT / "tests" / "python"))
import support  # noqa: E402,F401


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--json", action="store_true", help="write the summary as JSON on stdout")
    args = ap.parse_args(argv)
    start = time.perf_counter()
    # Test diagnostics must not corrupt the machine-readable summary on stdout.
    with redirect_stdout(sys.stderr):
        suite = unittest.defaultTestLoader.discover(str(ROOT / "tests" / "python"))
        result = unittest.TextTestRunner(stream=sys.stderr, verbosity=1).run(suite)
    ok = result.wasSuccessful() and result.testsRun > 0
    report = {"verdict": "PASS" if ok else "FAIL", "tests": result.testsRun,
              "failures": len(result.failures), "errors": len(result.errors),
              "skipped": len(result.skipped), "seconds": round(time.perf_counter() - start, 4)}
    print(json.dumps(report) if args.json else
          f"unit: {report['verdict']} ({report['tests']} tests, {report['seconds']:.4f} s)")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
