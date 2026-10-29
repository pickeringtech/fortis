"""`python3 -I interpreter/run.py FILE.ft [args...]`: run a module's `main` without the static checker
(the harness and the `fortis` command check first); prints its console output; a failing `try main` writes
one runtime-failure record to stderr and exits 1; a dynamic check failure exits 2 with the reason.
`--test` runs the module's test declarations instead and exits 1 when any case fails."""
import os
import sys

sys.path.insert(0, os.path.abspath(os.path.join(os.path.dirname(__file__), "..")))
from interpreter.eval import Interpreter  # noqa: E402
from interpreter.records import shown_path  # noqa: E402
from interpreter.values import FortisError
from interpreter.executor import DefectSignal  # noqa: E402


def main(argv):
    testing = "--test" in argv
    argv = [a for a in argv if a != "--test"]
    path = argv[0]
    src = open(path, encoding="utf-8").read()
    try:
        it = Interpreter(src, shown_path(path))
        it.json_out = not sys.stderr.isatty()
        if testing:
            passed, failed = it.run_tests()
            sys.stdout.write(f"{passed} passed, {failed} failed\n")
            return 1 if failed else 0
        it.stream = sys.stdout
        code = it.run_main(argv[1:])
    except FortisError as e:
        sys.stderr.write(f"dynamic check failed: {e}\n")
        return 2
    except RecursionError:
        sys.stderr.write("defect: stack_exhaustion\n")
        return 3
    except DefectSignal as d:
        return 3                                     # the defect record was emitted; no completion value (10.8)
    return code


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
