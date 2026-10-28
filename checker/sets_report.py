"""Trivial, copyable, shareable and linear membership (Language Reference 6.11, 6.12, 12.7) for every
built-in type and for the declared types of a module, as JSON; compared against a golden file by
`--golden path`. Usage: sets_report.py [module.ft] [--golden conformance/checker/sets_golden.json]"""
import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, ".."))
sys.path.insert(0, os.path.join(ROOT, "conformance", "tools"))
sys.path.insert(0, ROOT)
import lexer  # noqa: E402
import names  # noqa: E402
import parser  # noqa: E402
from checker.decls import DeclChecker  # noqa: E402
from checker.types import Named, OptionalT, TupleT, RecordT, FnT, INT, STRING  # noqa: E402

BUILTINS = ["Void", "Never", "Bool", "Int", "Int8", "Int16", "Int32", "Int64", "UInt", "UInt8", "UInt16", "UInt32", "UInt64", "Byte", "Float32", "Float",
            "Decimal", "BigInt", "Duration", "Instant", "Timestamp", "Scalar", "String", "StringView", "Bytes", "BytesView", "Money", "Currency",
            "Shared[Int]", "Weak[Int]", "Mutex[Int]", "AtomicInt", "AtomicBool", "Future[Int]", "Task[Int]", "Outcome[Int]", "Failure", "Must",
            "List[Int]", "List[String]", "List[Outcome[Int]]", "List[Shared[Int]]", "ListView[Int]", "Array[Int, 4]", "Map[String, Int]", "OrderedMap[String, Int]",
            "Set[Int]", "FixedList[Int, 4]", "FixedMap[String, Int, 4]", "Int?", "Outcome[Int]?", "(Int, Bool)", "(Int, String)", "{ a: Int, b: Bool }", "{ a: Int, b: String }",
            "fn(Int) Int", "Int<0..9>", "Report"]


def type_from_text(dc: DeclChecker, text: str):
    t = dc.lib.rec.parse(lexer.lex("let x: " + text + " = y\n"), "block") if False else None
    src = "import fortis.money { Money, Currency }\nfunc probe(x: " + text + ") Void {}\n"
    tree = dc.lib.rec.parse(lexer.lex(src), "module")
    from tree import find
    return dc.type_of(find(tree, "type"))


def main(argv):
    rec = parser.load_grammar(os.path.join(ROOT, "reference", "grammar", "fortis.ebnf"))
    lib = names.Library(rec)
    files = [a for a in argv if a.endswith(".ft")]
    src = open(files[0], encoding="utf-8").read() if files else ""
    tree = rec.parse(lexer.lex(src), "module") if src else rec.parse(lexer.lex("const nothing = 0\n"), "module")
    dc = DeclChecker(lib, files[0] if files else "<builtins>")
    dc.load(tree)
    if "fortis.money" in lib.modules:                 # the probe names Money and Currency
        dc.load_module_tree(lib.modules["fortis.money"].tree, "fortis.money", "fortis", library=True)
    out = {}
    declared = [d.name for d in dc.table.decls.values() if d.module == "main" and d.kind in ("class", "enum", "type")]
    for text in BUILTINS + declared:
        t = type_from_text(dc, text)
        out[text] = {"trivial": dc.is_trivial(t), "copyable": dc.is_copyable(t), "shareable": dc.is_shareable(t), "linear": dc.is_linear(t)}
    if "--golden" in argv:
        golden = json.load(open(argv[argv.index("--golden") + 1], encoding="utf-8"))
        diffs = [(k, golden.get(k), {f: v[f] for f in golden.get(k, {})}) for k, v in out.items() if k in golden and any(golden[k][f] != v[f] for f in golden[k])]
        missing = [k for k in golden if k not in out]
        for k, g, v in diffs:
            print(f"DIFF {k}: golden {g} computed {v}")
        for k in missing:
            print(f"MISSING {k}: in golden, not computed")
        print(f"sets: {len(out)} types compared, {len(diffs)} differences, {len(missing)} missing")
        return 1 if diffs or missing else 0
    print(json.dumps(out, indent=1))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
