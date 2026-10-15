#!/usr/bin/env bash
. $(dirname $0)/common.inc

# --echo-command-line in JSON: below full the compiler driver's start-up
# files and -L options are left out, the user's inputs and flags stay, and a
# long list shows its head and tail with a marker for the gap and the count
# in args_omitted. At full the first version's 20 + 5 stays.

echo 'static int x; int main() { return x; }' | $CC -c -o $t/main.o -xc -
echo 'static int y;' | $CC -c -o $t/empty.o -xc -
objs=""
for i in $(seq 1 60); do cp $t/empty.o $t/o$i.o; objs="$objs $t/o$i.o"; done

check() { python3 -I - "$@"; }

# A short command line.
RUSTY_AGENT=1 $CC -B. -o $t/exe -Wl,--echo-command-line $t/main.o > $t/out 2> /dev/null
check $t/out $t/main.o <<'EOF2'
import json, sys
r = json.loads(open(sys.argv[1]).readline())
assert r['type'] == 'command-line' and list(r)[:2] == ['schema', 'type'], r
a = r['args']
assert sys.argv[2] in a and '--echo-command-line' in a, a
assert not [x for x in a if x.startswith('-L')], a
assert not [x for x in a if x.rsplit('/', 1)[-1].startswith('crt')], a
assert r['args_total'] > len(a) and r['args_omitted'] == r['args_total'] - len(a), r
assert 'response_files' not in r, r
assert len(json.dumps(r)) < 1500
EOF2

# A long one: 60 inputs.
RUSTY_AGENT=1 $CC -B. -o $t/exe -Wl,--echo-command-line $t/main.o $objs > $t/out 2> /dev/null
check $t/out <<'EOF2'
import json, sys
r = json.loads(open(sys.argv[1]).readline())
a = r['args']
markers = [x for x in a if x.startswith('[') and x.endswith(' arguments omitted]')]
assert len(markers) == 1, a
gap = int(markers[0][1:].split()[0])
assert not [x for x in a if x.rsplit('/', 1)[-1].startswith('crt') or x.startswith('-L')], a
# Every argument is either shown, left out as boilerplate or inside the gap.
assert r['args_total'] == (len(a) - 1) + r['args_omitted'], (len(a), r['args_omitted'], r['args_total'])
assert any(x.endswith('main.o') for x in a) and any(x.endswith('o60.o') for x in a), a   # head and tail keep inputs
assert '--echo-command-line' in a
assert len(a) <= 31, len(a)
assert len(json.dumps(r)) < 3500
EOF2

# Full: the first version's shape (20 first, 5 last).
RUSTY_AGENT=1 RUSTY_AGENT_DETAIL=full $CC -B. -o $t/exe -Wl,--echo-command-line $t/main.o $objs > $t/out 2> /dev/null
check $t/out <<'EOF2'
import json, sys
r = json.loads(open(sys.argv[1]).readline())
assert len(r['args']) == 25 and r['args_omitted'] == r['args_total'] - 25, r
assert r['response_files'] == [], r
assert any(x.rsplit('/', 1)[-1].startswith('crt') for x in r['args']), r['args']
EOF2

# Brief shows less.
RUSTY_AGENT=1 RUSTY_AGENT_DETAIL=brief $CC -B. -o $t/exe -Wl,--echo-command-line $t/main.o $objs > $t/out 2> /dev/null
check $t/out <<'EOF2'
import json, sys
r = json.loads(open(sys.argv[1]).readline())
assert len(r['args']) <= 13, r['args']
EOF2

# A response file is listed with its own (shaped) arguments.
printf '%s\n' "$t/main.o" > $t/rsp
RUSTY_AGENT=1 ./mold -o $t/exe2 @$t/rsp --echo-command-line --no-dynamic-linker -e main > $t/out 2> /dev/null || true
check $t/out $t/rsp <<'EOF2'
import json, sys
r = json.loads(open(sys.argv[1]).readline())
assert r['response_files'][0]['path'].lstrip('@') == sys.argv[2], r
assert r['response_files'][0]['args'][0].endswith('main.o'), r
EOF2

# Plain mode: the text line is unchanged.
$CC -B. -o $t/exe -Wl,--echo-command-line $t/main.o > $t/plain
grep -F 'command line: ' $t/plain
grep -F 'crt1.o' $t/plain
