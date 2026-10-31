"""UAX #29 GB3-GB9c and GB11-GB13; Hangul GB6-GB8 remain outside the contract."""
from bisect import bisect_right
from interpreter import grapheme_data as data


def contains(ranges, n):
    i = bisect_right(ranges, (n, 0x110000)) - 1
    return i >= 0 and ranges[i][0] <= n <= ranges[i][1]


def count(text):
    total, previous, ri_run = 0, None, 0
    pict_run = zwj_after_pict = False
    conjunct = 0  # 1: consonant and extenders; 2: at least one linker too
    for ch in text:
        n = ord(ch)
        extend = contains(data.EXTEND, n)
        pict = contains(data.EXTENDED_PICTOGRAPHIC, n)
        control = contains(data.CONTROL, n) or ch in '\r\n'
        ri = 0x1F1E6 <= n <= 0x1F1FF
        consonant = contains(data.INCB_CONSONANT, n)
        joined = False
        if previous is not None:
            p, p_control, p_prepend = previous
            if p == 13 and n == 10:
                joined = True  # GB3 precedes control boundaries
            elif not p_control and not control:
                joined = (extend or n == 0x200D or contains(data.SPACINGMARK, n)
                          or p_prepend or (conjunct == 2 and consonant)
                          or (zwj_after_pict and pict) or (ri and ri_run % 2 == 1))
        total += not joined
        zwj_after_pict = n == 0x200D and pict_run
        pict_run = pict or (extend and pict_run)
        if consonant:
            conjunct = 1
        elif conjunct and contains(data.INCB_LINKER, n):
            conjunct = 2
        elif not contains(data.INCB_EXTEND, n):
            conjunct = 0
        ri_run = ri_run + 1 if ri else 0
        previous = n, control, contains(data.PREPEND, n)
    return total
