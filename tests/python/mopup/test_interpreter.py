import unittest
from support import ROOT
from interpreter.eval import Interpreter

class TextConversions(unittest.TestCase):
    def test_owned_text_copy_survives_source_and_calls_member_once(self):
        source = '''import fortis.host { Console }
class Text {
  value: String
  convert from String(owned source) { return Text(value: move source) }
}
func make() Text {
  let original: String = "tea"
  return original.trim() as Text
}
func main(console: Console) {
  let text = make()
  console.print(text.value)
}
'''
        interpreter = Interpreter(source, "mopup_copy.ft")
        self.assertEqual(interpreter.run_main([]), 0)
        self.assertEqual(interpreter.out, ["tea"])
