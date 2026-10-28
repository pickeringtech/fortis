# Disagreement decision record — <fixture id>

A disagreement between two implementations over a conformance fixture blocks release until it is
resolved by a versioned change to the specification or to one implementation (Programme 6.3,
PROG-VM-02). This record is never an exception that lets a disagreement stand.

- Fixture: <id> (sha256 of the source at the time: <hash>)
- Comparison: <left> vs <right> at commit <hash>; report: conformance/differential/<file>.json
- Kind: verdict | code | output | trace
- Left observed: <…>
- Right observed: <…>

## Root cause
<which implementation deviated from which sentence of the reference, or which sentence was ambiguous>

## Resolution (exactly one)
- Specification change: register row X-nn, section …; both implementations now agree at commit <hash>.
- Implementation change: <implementation>, commit <hash>; the specification stands.

## Verification
Re-run: `python3 -I conformance/tools/differential.py --left <left> --right <right>` → 0 disagreements for this fixture.
