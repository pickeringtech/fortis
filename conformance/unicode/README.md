# Grapheme properties

`grapheme-properties.json` records the Unicode 15.1 properties used by UAX #29
GB3-GB9c and GB11-GB13. Its `sources` list names the Unicode Consortium data files.
Hangul GB6-GB8 remain unimplemented, as the library contract says.

Python's `unicodedata` exposes general categories, not grapheme or emoji properties.
The generator starts Extend with Mn/Me and SpacingMark with Mc, then applies the
published properties' exact additions and removals. The other properties are kept
as ranges. This avoids treating every symbol as an emoji or every spacing mark as
a grapheme extender. Indic conjunct properties implement GB9c; controls and Prepend
implement GB4, GB5 and GB9b. GB11 requires the preceding pictograph before the ZWJ.

Regenerate offline with Python 3.12 (Unicode data 15.0.0):

```bash
python3 -I conformance/tools/gen_graphemes.py
python3 -I conformance/tools/gen_graphemes.py --check
```

The unit gate checks both generated files. The interpreter imports the Python
table; the VM and native runtime share the Rust table and segmentation function.
Updating the Unicode version requires updating the recorded property differences
against that version's published files, then regenerating and running the shared
cases in `tests/cases/text/graphemes.json`.

That table also retains all 754 cases from Unicode 15.1's
`auxiliary/GraphemeBreakTest.txt` that contain no Hangul scalars, with their original
source line numbers. The other 433 official cases exercise the documented gap.

The property data is distributed under the Unicode licence in `LICENSE.txt`.
