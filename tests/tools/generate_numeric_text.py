"""Regenerate the F2 numeric execution fixture from its cross-runtime case table."""
from pathlib import Path
import os
ROOT = Path(__file__).resolve().parents[2]
os.chdir(ROOT)
import json
rows=json.loads(Path('tests/cases/runtime/numeric_text.json').read_text())
def lit(s):
 return '"'+''.join({'"':'\\"','\\':'\\\\','\n':'\\n','\r':'\\r','\t':'\\t'}.get(c,c if c.isprintable() else '\\u{'+format(ord(c),'x')+'}') for c in s)+'"'
expected=[r['expect']['code'] if isinstance(r['expect'],dict) else r['expect'] for r in rows]
s='// execute: '+'|'.join(expected)+'\n// requires: runtime\n// covers: LANG-06-14\n// D-655 and D-680: the shared numeric_text.json table, through every backend.\nimport fortis.host { Console }\n\n'
for target in dict.fromkeys(r['input']['target'] for r in rows):
 s+=f'''recovers func read{target}(text: StringView) String {{
  let value = text as {target} catch e {{ return e.code as String }}
  return value as String
}}
\n'''
s+='recovers func main(console: Console) {\n'+''.join(f'  console.print(read{r["input"]["target"]}({lit(r["input"]["value"])}))\n' for r in rows)+'}\n'
Path('conformance/programs/exec_f2_numeric_text.ft').write_text(s)
