#!/usr/bin/env bash
. $(dirname $0)/common.inc

# The `why` record: at normal the chain says each hop once ("via `sym` from
# SECTION", then the root), a removed section asked about with --why-kept is
# worded as "not in the output", a long chain is cut with chain_omitted, ICF
# answers drop the two general sentences, and the grammar is right.

cat <<EOF2 | $CC -O0 -ffunction-sections -c -o $t/a.o -xc -
int used_leaf(int x) { return x + 100; }
int dead_leaf(int x) { return x + 7; }
int same1(int x) { return x * 3 + 1; }
int same2(int x) { return x * 3 + 1; }
int main(int c, char **v) { return used_leaf(c) + same1(c) + same2(c); }
EOF2

check() { python3 -I - "$@"; }

RUSTY_AGENT=1 $CC -B. -o $t/exe $t/a.o -Wl,--gc-sections,--why-kept=used_leaf,--why-kept=dead_leaf,--why-removed=dead_leaf* > $t/out 2> $t/err
[ ! -s $t/err ]
check $t/out <<'EOF2'
import json, sys
recs = [json.loads(l) for l in open(sys.argv[1])]
assert len(recs) == 3, recs
kept, removed_by_kept, removed = recs
for r in recs:
    assert list(r)[:2] == ['schema', 'type'] and r['type'] == 'why', r
    assert len(json.dumps(r, separators=(',', ':'))) < 600, r
assert kept['decision'] == 'kept' and kept['by'] == '--gc-sections', kept
assert kept['chain'][0].startswith('via `used_leaf` from '), kept['chain']
assert kept['chain'][-1].startswith('root: '), kept['chain']
# The section name is not repeated on every line.
assert sum(kept['section'] in c for c in kept['chain']) == 0, kept
# Asked why it is kept, but it is not even in the output: say so.
assert removed_by_kept['decision'] == 'removed'
assert removed_by_kept['chain'][0] == 'it is not in the output: no section that is kept refers to it', removed_by_kept
assert removed['chain'][0] == 'no section that is kept refers to it', removed
EOF2

# Full is the first version's text.
RUSTY_AGENT=1 RUSTY_AGENT_DETAIL=full $CC -B. -o $t/exe $t/a.o -Wl,--gc-sections,--why-kept=used_leaf > $t/out 2> $t/err
check $t/out <<'EOF2'
import json, sys
r = json.loads(open(sys.argv[1]).readline())
assert list(r) == ['schema', 'type', 'query', 'section', 'by', 'decision', 'chain'], list(r)
assert r['chain'][0].endswith(' is referenced via `used_leaf` from ' + r['chain'][1].split(' is ')[0]), r
assert ' is a root: ' in r['chain'][-1], r
EOF2

# ICF: the two general sentences are only at full.
RUSTY_AGENT=1 $CC -B. -o $t/exe $t/a.o -Wl,--icf=all,--why-kept=same2 > $t/out 2> $t/err
RUSTY_AGENT=1 RUSTY_AGENT_DETAIL=full $CC -B. -o $t/exe $t/a.o -Wl,--icf=all,--why-kept=same2 > $t/outf 2> $t/err
check $t/out $t/outf <<'EOF2'
import json, sys
n = json.loads(open(sys.argv[1]).readline())
f = json.loads(open(sys.argv[2]).readline())
assert n['decision'] == f['decision'] == 'folded', (n, f)
assert len(f['chain']) == 3 and len(n['chain']) == 1 and n['chain'][0].startswith('folded into '), (n, f)
EOF2

# Brief: the decision and the first reason.
RUSTY_AGENT=1 RUSTY_AGENT_DETAIL=brief $CC -B. -o $t/exe $t/a.o -Wl,--gc-sections,--why-kept=used_leaf > $t/out 2> $t/err
check $t/out <<'EOF2'
import json, sys
r = json.loads(open(sys.argv[1]).readline())
assert set(r) == {'schema', 'type', 'query', 'section', 'decision', 'reason', 'chain_length'}, r
EOF2

# A long chain is cut, keeping the first hops and the root.
python3 -I - $t <<'EOF2'
import sys
n = 14
with open(sys.argv[1] + '/chain.c', 'w') as f:
    for i in range(n):
        nxt = 'return f%d(x) + 1;' % (i + 1) if i + 1 < n else 'return x;'
        f.write('int f%d(int x);\n' % (i + 1) if i + 1 < n else '')
        f.write('int f%d(int x) { %s }\n' % (i, nxt))
    f.write('int main(int c, char **v) { return f0(c); }\n')
EOF2
$CC -O0 -ffunction-sections -c -o $t/chain.o $t/chain.c
RUSTY_AGENT=1 $CC -B. -o $t/exe $t/chain.o -Wl,--gc-sections,--why-kept=f13 > $t/out 2> $t/err
check $t/out <<'EOF2'
import json, sys
r = json.loads(open(sys.argv[1]).readline())
assert r['decision'] == 'kept' and r['chain_omitted'] >= 5, r
assert len(r['chain']) == 7 and r['chain'][-1].startswith('root: '), r
EOF2

# The grammar of the count of debug sections: one section is singular.
cat <<EOF2 | $CC -c -o $t/g.o -x assembler -
  .section .text.gone,"ax",@progbits
  .globl gone
gone: ret
  .section .text.main,"ax",@progbits
  .globl main
main: ret
  .section .debug_one,"",@progbits
  .quad gone
EOF2
RUSTY_AGENT=1 RUSTY_AGENT_DETAIL=full $CC -B. -o $t/exe $t/g.o -nostdlib -Wl,-e,main,--gc-sections,--why-removed=gone > $t/out 2> $t/err
check $t/out <<'EOF2'
import json, sys
r = json.loads(open(sys.argv[1]).readline())
assert r['chain'][-1] == '1 debug section of the same object is also removed', r['chain']
EOF2

# Text mode is as it was.
$CC -B. -o $t/exe $t/a.o -Wl,--gc-sections,--why-kept=dead_leaf > $t/plain
grep -F 'dead_leaf): removed by --gc-sections' $t/plain
grep -F '  no section that is kept refers to it' $t/plain
