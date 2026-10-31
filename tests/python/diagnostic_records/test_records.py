"""Published diagnostic schema and PROG-DIA-01/05: records and wire data."""
import json
import unittest
from support import forall, table_tests
import diagnostics


def check(test, record, expect):
    test.assertEqual(not diagnostics.validate(record), expect)
    if expect:
        test.assertEqual(json.loads(diagnostics.to_json_line(record)), record)
        text = diagnostics.render_text(record)
        test.assertIn(record['code'], text)
        test.assertIn(record['message'], text)


def wire(test, value, expect):
    test.assertEqual(diagnostics.to_json_line(value), expect)
    test.assertEqual(json.loads(expect), value)


class Records(unittest.TestCase):
    pass


class Malformed(unittest.TestCase):
    pass


class Wire(unittest.TestCase):
    @forall(lambda r: ''.join(chr(r.choice([0, 9, 10, 34, 92, 127, 233, 955, 129408])) for _ in range(r.randint(0, 25))))
    def test_prog_dia_05_unicode_round_trip(self, value):
        self.assertEqual(json.loads(diagnostics.to_json_line(value)), value)


table_tests(Records, 'diagnostics/records.json', check)
table_tests(Malformed, 'diagnostics/malformed.json', check)
table_tests(Wire, 'diagnostics/wire.json', wire)
