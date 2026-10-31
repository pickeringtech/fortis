"""Standard-library helpers for isolated Python unit tests. See tests/README.md."""
from functools import wraps
import json
import os
from pathlib import Path
import random
import re
import sys

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))
# The reference frontend's modules import lexer, parser and names by these names.
sys.path.insert(0, str(ROOT / "conformance" / "tools"))


def forall(generator, cases=200, seed=None):
    """Decorate a check(value), or a unittest method(self, value).

    generator takes a private random.Random. An environment seed overrides the
    supplied seed; otherwise the default is 0. A failure prints its replay data.
    """
    if cases < 1:
        raise ValueError("cases must be positive")

    def decorate(check):
        @wraps(check)
        def run(*args, **kwargs):
            chosen = int(os.environ.get("FORTIS_TEST_SEED", seed if seed is not None else 0))
            if not 0 <= chosen < 2**64:
                raise ValueError("seed must be an unsigned 64-bit integer")
            rng = random.Random(chosen)
            for index in range(cases):
                value = "<generator did not return>"
                try:
                    value = generator(rng)
                    check(*args, value, **kwargs)
                except Exception as exc:
                    raise AssertionError(
                        f"FORTIS_TEST_SEED={chosen} case={index} value={value!r}: {exc}"
                    ) from exc
        return run
    return decorate


def read_cases(path):
    """Read tests/cases/<path>, validating the shared schema before any test runs."""
    def pairs(items):
        obj = {}
        for key, value in items:
            if key in obj:
                raise ValueError(f"duplicate key: {key}")
            obj[key] = value
        return obj

    rows = json.loads((ROOT / "tests" / "cases" / path).read_text(encoding="utf-8"),
                      object_pairs_hook=pairs)

    def value_ok(value):
        if value is None or type(value) is bool:
            return True
        if type(value) is int:
            return -(2**63) <= value < 2**63
        if isinstance(value, str):
            return all(not 0xD800 <= ord(c) <= 0xDFFF for c in value)
        if isinstance(value, list):
            return all(value_ok(v) for v in value)
        if isinstance(value, dict):
            return all(value_ok(k) and value_ok(v) for k, v in value.items())
        return False

    if not isinstance(rows, list) or not rows or not value_ok(rows):
        raise ValueError(f"{path}: expected a non-empty array of cases with JSON integers in i64")
    names = set()
    for row in rows:
        if not isinstance(row, dict) or set(row) != {"name", "input", "expect"}:
            raise ValueError(f"{path}: each case needs exactly name, input, expect")
        name = row["name"]
        if not isinstance(name, str) or not re.fullmatch(r"[a-z][a-z0-9_]*", name) or name in names:
            raise ValueError(f"{path}: invalid or duplicate case name: {name!r}")
        names.add(name)
    return rows


def table_tests(test_class, path, check):
    """Install one discoverable unittest method per row, with its stable case name."""
    for row in read_cases(path):
        def run(self, row=row):
            check(self, row["input"], row["expect"])
        name = "test_" + row["name"]
        if hasattr(test_class, name):
            raise ValueError(f"duplicate test method: {name}")
        run.__name__ = name
        setattr(test_class, name, run)
    return test_class
