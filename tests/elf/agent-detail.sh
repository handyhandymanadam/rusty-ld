#!/usr/bin/env bash
. $(dirname $0)/common.inc

# Detail levels of the JSON records: --agent-detail=brief|normal|full and
# RUSTY_AGENT_DETAIL (the option wins). Normal is the default in agent mode;
# without agent mode the records are as they always were. A size budget per
# record type fails the test when a record grows.

cat <<EOF2 | $CC -g -O0 -ffunction-sections -c -o $t/a.o -xc -
int used_leaf(int x) { return x + 100; }
int dead_leaf(int x) { return x + 7; }
int main(int c, char **v) { return used_leaf(c); }
EOF2
cat <<EOF2 | $CC -g -c -o $t/bad.o -xc -
int missing_one(void);
int missing_two(void);
int main() { return missing_one() + missing_two(); }
EOF2

# Runs a failing link; the diagnostics are in $t/err.
fail() { not $CC -B. -o $t/exe $t/bad.o "$@" 2> $t/err; }
check() { python3 -I - "$@"; }

# Normal is the default in agent mode.
RUSTY_AGENT=1 fail
check $t/err <<'EOF2'
import json, sys
recs = [json.loads(l) for l in open(sys.argv[1]) if l.startswith('{')]
d = [r for r in recs if r['type'] == 'diagnostic']
assert len(d) == 2, d
for r in d:
    assert list(r)[:5] == ['schema', 'type', 'code', 'severity', 'message'], list(r)
    for gone in ('rendered', 'files', 'symbols', 'notes', 'suggestions'):
        assert gone not in r, (gone, r)
    assert len(json.dumps(r, separators=(',', ':'))) < 400, r      # budget: bytes of one record
EOF2

# The option; the environment variable; the option wins.
RUSTY_AGENT=1 fail -Wl,--agent-detail=full
check $t/err <<'EOF2'
import json, sys
r = [json.loads(l) for l in open(sys.argv[1]) if l.startswith('{')][0]
for k in ('rendered', 'files', 'symbols', 'notes', 'suggestions', 'locations'):
    assert k in r, k
assert list(r)[:5] == ['schema', 'type', 'code', 'severity', 'message'], list(r)
EOF2
RUSTY_AGENT=1 RUSTY_AGENT_DETAIL=full fail
grep -F '"rendered"' $t/err
RUSTY_AGENT=1 RUSTY_AGENT_DETAIL=full fail -Wl,--agent-detail=normal
not grep -F '"rendered"' $t/err
# A value that is not a level in the variable is normal; in the option it is an error.
RUSTY_AGENT=1 RUSTY_AGENT_DETAIL=huge fail
not grep -F '"rendered"' $t/err
RUSTY_AGENT=1 fail -Wl,--agent-detail=huge
grep -F 'unknown --agent-detail argument: huge' $t/err
RUSTY_AGENT=1 fail -Wl,--agent-detail
grep -F 'unknown command line option: --agent-detail' $t/err

# Brief: the code, the message, one place, counts.
RUSTY_AGENT=1 fail -Wl,--agent-detail=brief
check $t/err <<'EOF2'
import json, sys
recs = [json.loads(l) for l in open(sys.argv[1]) if l.startswith('{')]
for r in recs:
    if r['type'] != 'diagnostic':
        continue
    assert set(r) <= {'schema', 'type', 'code', 'severity', 'message', 'location', 'fix',
                      'more_locations', 'more_fixes', 'note_count'}, r
    assert 'location' in r and 'line' in r['location'], r
    assert len(json.dumps(r, separators=(',', ':'))) < 300, r
EOF2

# Without agent mode, JSON diagnostics are what they always were (full), and
# the level can still be asked for.
not $CC -B. -o $t/exe $t/bad.o -Wl,--diagnostics-format=json 2> $t/err
grep -F '"rendered"' $t/err
not $CC -B. -o $t/exe $t/bad.o -Wl,--diagnostics-format=json,--agent-detail=normal 2> $t/err
not grep -F '"rendered"' $t/err
# RUSTY_AGENT_DETAIL alone changes nothing.
RUSTY_AGENT_DETAIL=brief not $CC -B. -o $t/exe $t/bad.o -Wl,--diagnostics-format=json 2> $t/err
grep -F '"rendered"' $t/err
# The plain text does not change with any of it.
not $CC -B. -o $t/exe $t/bad.o 2> $t/plain1
RUSTY_AGENT_DETAIL=brief not $CC -B. -o $t/exe $t/bad.o -Wl,--agent-detail=brief 2> $t/plain2
cmp <(grep -v '^+ ' $t/plain1 | sort) <(grep -v '^+ ' $t/plain2 | sort)
grep -F 'mold: error: undefined symbol: missing_one' $t/plain1

# Size budgets of the answers, at normal, for a small link.
RUSTY_AGENT=1 $CC -B. -o $t/exe $t/a.o -Wl,--gc-sections,--echo-command-line,--why-kept=used_leaf,--why-removed=dead_leaf,--size-report,--link-summary=$t/sum.json > $t/out 2> $t/err2
check $t/out $t/err2 <<'EOF2'
import json, sys
budget = {'command-line': 1500, 'why': 600, 'size-report': 2500, 'link-summary': 300}
seen = set()
for path in sys.argv[1:]:
    for l in open(path):
        if not l.startswith('{'):
            continue
        r = json.loads(l)
        assert list(r)[:2] == ['schema', 'type'], r
        seen.add(r['type'])
        assert len(l) < budget[r['type']], (r['type'], len(l))
        assert 'rendered' not in r
assert seen == set(budget), seen
EOF2
# An empty or zero field is absent: the clean summary has no errors or warnings.
check $t/sum.json <<'EOF2'
import json, sys
s = json.load(open(sys.argv[1]))
assert 'errors' not in s and 'warnings' not in s and 'elapsed_seconds' not in s, s
assert s['status'] == 'ok' and s['inputs']['objects'] >= 1, s
EOF2
# Stable mode has no elapsed time; with --no-stable-diagnostics it has.
RUSTY_AGENT=1 $CC -B. -o $t/exe $t/a.o -Wl,--no-stable-diagnostics,--link-summary=$t/sum2.json 2> /dev/null
grep -F '"elapsed_seconds"' $t/sum2.json
# And in full everything of the first version is there.
RUSTY_AGENT=1 RUSTY_AGENT_DETAIL=full $CC -B. -o $t/exe $t/a.o -Wl,--link-summary=$t/sum3.json 2> /dev/null
check $t/sum3.json <<'EOF2'
import json, sys
s = json.load(open(sys.argv[1]))
for k in ('output_size', 'target', 'inputs', 'output_sections', 'errors', 'warnings'):
    assert k in s, k
assert set(s['inputs']) == {'objects', 'objects_from_archives', 'shared_libraries'}, s
EOF2
