# Lookahead report (generated parser, DEL-02)

Grammar sha256 `4658cfa29c3c521c4fa321e822fec871287f97d0fbf34e8ee559ac5d9cb1ceec`; generator `conformance/tools/gen_parser.py`; emitted parser `conformance/generated/fortis_parser.py`.

**Lookahead bound: k = 3**, confirmed by the generator: at k = 3 every one of the 298 decision points is separated by token classes alone or covered by a documented policy (0 undocumented conflicts). No decision consults a name: lookahead tables hold literal token texts and lexer kinds only.

| k | decisions | conflicts | without a documented policy |
|---|---|---|---|
| 1 | 298 | 86 | 11 |
| 2 | 298 | 43 | 2 |
| 3 | 298 | 36 | 0 |

Conflicts at k = 3 by resolution policy (Language Reference 3.2, X-94): P2: 23, P3: 13

| Rule | Decision | Branches | Example shared prefix | Policy |
|---|---|---|---|---|
| funcSignature | alternative | [0, 1] | `{ KIND:LOWER :` | P3 |
| paramType | option | [0, 1] | `editable ( (` | P2 |
| statement | alternative | [1, 19] | `KIND:LOWER . KIND:KW` | P3 |
| statement | alternative | [2, 19] | `if ! !` | P3 |
| statement | alternative | [6, 19] | `{ KIND:LOWER :` | P3 |
| letStmt | alternative | [0, 1] | `let KIND:LOWER =` | P3 |
| forStmt | alternative | [0, 1] | `move KIND:LOWER .` | P3 |
| forStmt | alternative | [0, 2] | `move KIND:LOWER .` | P3 |
| forStmt | alternative | [1, 2] | `! ! !` | P3 |
| outcomeExpr | option | [0, 1] | `catch KIND:LOWER {` | P2 |
| coalesceExpr | option | [0, 1] | `?? ! !` | P2 |
| orExpr | repetition | [0, 1] | `|| ! !` | P2 |
| andExpr | repetition | [0, 1] | `&& ! !` | P2 |
| bitOrExpr | repetition | [0, 1] | `| ! !` | P2 |
| bitXorExpr | repetition | [0, 1] | `^ ! !` | P2 |
| bitAndExpr | repetition | [0, 1] | `& ! !` | P2 |
| equalityExpr | option | [0, 1] | `!= ! !` | P2 |
| relationalExpr | option | [0, 1] | `< ! !` | P2 |
| shiftExpr | repetition | [0, 1] | `<< ! !` | P2 |
| additiveExpr | repetition | [0, 1] | `+ ! !` | P2 |
| multiplicativeExpr | repetition | [0, 1] | `% ! !` | P2 |
| asExpr | repetition | [0, 1] | `as KIND:LOWER .` | P2 |
| markerExpr | option | [0, 1] | `( ! !` | P2 |
| markerExpr | repetition | [0, 1] | `( ! !` | P2 |
| postfixExpr | repetition | [0, 1] | `( ! !` | P2 |
| postfix | alternative | [0, 1] | `. KIND:KW (` | P3 |
| primaryExpr | alternative | [4, 5] | `( ! !` | P3 |
| primaryExpr | option | [0, 1] | `. KIND:KW !=` | P2 |
| primaryExpr | option | [0, 1] | `( ! !` | P2 |
| typeRef | option | [0, 1] | `[ ( (` | P2 |
| matchExpr | option | [0, 1] | `move KIND:LOWER !=` | P2 |
| matchArm | alternative | [0, 1] | `{ KIND:LOWER :` | P3 |
| place | repetition | [0, 1] | `. KIND:KW !=` | P2 |
| type | alternative | [1, 2] | `( ( (` | P3 |
| namedType | alternative | [1, 2] | `( ( (` | P3 |
| fnType | option | [0, 1] | `from KIND:LOWER )` | P2 |

## Agreement with the recogniser

1162 fixtures: 0 disagreements.

## Ambiguity corpus

30 programs, 30 with exactly one parse (no undocumented fallback); documented ordered choices used: 76.

## Timing (pinned conditions)

Warm process, median of 20 parses per fixture, Linux-7.2.5-3-omarchy-x86_64-with-glibc2.44, Python 3.12.12; 1128 fixtures listed by sha256 in check.json; slowest median 12.79 ms (PRG-exec_loan_owned_part, 1405 tokens); 0 at or over 50 ms.
