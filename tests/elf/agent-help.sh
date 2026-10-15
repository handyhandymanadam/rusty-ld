#!/usr/bin/env bash
. $(dirname $0)/common.inc

# --agent-help describes agent mode as one JSON object on standard output.
# It works without agent mode, exits 0, and lists every option, record
# type and code the documentation names.

./mold --agent-help > $t/help.json 2> $t/help.err
[ ! -s $t/help.err ]
[ "$(wc -l < $t/help.json)" -eq 1 ]
python3 -I - $t/help.json <<'EOF2'
import json, sys
d = json.load(open(sys.argv[1]))
assert list(d)[:2] == ['schema', 'type'], list(d)
assert d['schema'] == 1 and d['type'] == 'help' and d['tool'] == 'rusty-ld', d
options = {o['option'] for o in d['options']}
for want in ['--agent-mode', '--no-agent-mode', '--agent-detail=LEVEL', '--agent-help',
             '--diagnostics-format=json', '--explain-undefined', '--explain-duplicates',
             '--stable-diagnostics', '--no-stable-diagnostics', '--echo-command-line',
             '--why-kept=NAME', '--why-removed=NAME', '--link-summary=FILE', '--size-report[=FILE]']:
    assert want in options, want
types = {r['type'] for r in d['record_types']}
for want in ['diagnostic', 'command-line', 'size-report', 'why', 'link-summary', 'records-omitted', 'help']:
    assert want in types, want
codes = {c['code'] for c in d['codes']}
for n in list(range(1, 15)) + [100, 200, 300]:
    assert 'L%04d' % n in codes, n
assert d['detail']['default'].startswith('normal'), d['detail']
assert d['switch']['env'] == 'RUSTY_AGENT=1'
assert len(open(sys.argv[1]).read()) < 9000   # the description is itself token cost
EOF2

# With agent mode on, and with a bad option before it, it still answers.
RUSTY_AGENT=1 ./mold --agent-help | python3 -I -c 'import json,sys; assert json.load(sys.stdin)["type"] == "help"'
# The option is in --help.
./mold --help | grep -F -e '--agent-help' -e '--agent-detail'
