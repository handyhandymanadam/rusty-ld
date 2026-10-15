#!/usr/bin/env bash
. $(dirname $0)/common.inc

# The size budget: at normal and brief, once the records on standard error
# pass 12 KB only errors go on, and past 20 KB nothing but the summary. A
# `records-omitted` record says how many were left out. The copy channel
# (RUSTY_AGENT_OUT) gets every record, and full has no cap.

python3 -I - $t <<'EOF2'
import sys
with open(sys.argv[1] + '/many.c', 'w') as f:
    for i in range(300):
        f.write('int missing_symbol_number_%d(void);\n' % i)
    f.write('int main(void) { return 0\n')
    for i in range(300):
        f.write('  + missing_symbol_number_%d()\n' % i)
    f.write('; }\n')
EOF2
$CC -c -o $t/many.o $t/many.c

check() { python3 -I - "$@"; }

rm -f $t/all.jsonl
RUSTY_AGENT=1 RUSTY_AGENT_OUT=$t/all.jsonl not $CC -B. -o $t/exe $t/many.o -Wl,--link-summary=$t/s.json 2> $t/err
check $t/err $t/all.jsonl <<'EOF2'
import json, os, sys
shown = [json.loads(l) for l in open(sys.argv[1]) if l.startswith('{')]
every = [json.loads(l) for l in open(sys.argv[2])]
assert sum(r['type'] == 'diagnostic' for r in every) == 300
d = [r for r in shown if r['type'] == 'diagnostic']
assert 0 < len(d) < 300, len(d)
text = sum(len(l) for l in open(sys.argv[1]) if l.startswith('{'))
assert text < 22000, text                       # about 20 KB, plus the closing records
om = [r for r in shown if r['type'] == 'records-omitted']
assert len(om) == 1 and om[0]['by_type'] == {'diagnostic': 300 - len(d)}, om
assert om[0]['records_omitted'] == 300 - len(d)
assert shown[-1]['type'] == 'link-summary', shown[-1]['type']    # the summary is last, and kept
assert not any(r['type'] == 'records-omitted' for r in every)    # the file has no gap
EOF2

# Full has no cap.
RUSTY_AGENT=1 RUSTY_AGENT_DETAIL=full not $CC -B. -o $t/exe $t/many.o 2> $t/err
[ "$(grep -c '"type":"diagnostic"' $t/err)" -eq 300 ]
not grep -F 'records-omitted' $t/err

# Without agent mode, there is no budget either.
not $CC -B. -o $t/exe $t/many.o -Wl,--diagnostics-format=json 2> $t/err
[ "$(grep -c '"type":"diagnostic"' $t/err)" -eq 300 ]

# A small link has no `records-omitted`.
echo 'int main() { return 0; }' | $CC -c -o $t/ok.o -xc -
RUSTY_AGENT=1 $CC -B. -o $t/exe $t/ok.o 2> $t/err
[ ! -s $t/err ]
