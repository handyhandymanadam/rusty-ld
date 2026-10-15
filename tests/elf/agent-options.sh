#!/usr/bin/env bash
. $(dirname $0)/common.inc

# Option handling in agent mode: --link-summary without =FILE says what it
# needs; a --link-summary file that cannot be written is a warning (L0014),
# not a failed link; --no-agent-mode wins wherever it stands; --why-removed
# without a removal pass warns; --no-stable-diagnostics is accepted and turns
# the sorting off; the link summary goes to standard error too, with the same
# text as the file and the copy channel.

cat <<EOF2 | $CC -O0 -ffunction-sections -c -o $t/a.o -xc -
int used_leaf(int x) { return x + 100; }
int dead_leaf(int x) { return x + 7; }
int main(int c, char **v) { return used_leaf(c); }
EOF2

records() {
  python3 -I - "$@" <<'EOF2'
import json, sys
for path in sys.argv[1:]:
    for line in open(path):
        if line.startswith('{'):
            json.loads(line)
            print(line.rstrip('\n'))
EOF2
}

# A1: --link-summary with no =FILE.
RUSTY_AGENT=1 not $CC -B. -o $t/exe $t/a.o -Wl,--link-summary 2> $t/e1
records $t/e1 | python3 -I -c '
import json, sys
r = json.loads(sys.stdin.readline())
assert r["code"] == "L0006" and "needs =FILE" in r["message"], r
'
# Plain mode keeps the usual text.
not $CC -B. -o $t/exe $t/a.o -Wl,--link-summary 2>&1 | grep -F 'unknown command line option: --link-summary'
not $CC -B. -o $t/exe $t/a.o -Wl,--link-summary 2>&1 | not grep -F 'needs'

# A1: an unwritable summary path is a warning; the link works.
RUSTY_AGENT=1 $CC -B. -o $t/exe2 $t/a.o -Wl,--link-summary=$t/no/such/dir/s.json 2> $t/e2
$t/exe2 || true
records $t/e2 | python3 -I -c '
import json, sys
recs = [json.loads(l) for l in sys.stdin]
w = [r for r in recs if r["type"] == "diagnostic"]
assert len(w) == 1 and w[0]["code"] == "L0014" and w[0]["severity"] == "warning", recs
assert "cannot write" in w[0]["message"], w
'
# Without agent mode it still fails the link, as it always did.
not $CC -B. -o $t/exe3 $t/a.o -Wl,--link-summary=$t/no/such/dir/s.json 2> $t/e3
grep -F -- '--link-summary: cannot write' $t/e3

# A2: --no-agent-mode after a bad option still wins: plain text.
RUSTY_AGENT=1 not $CC -B. -o $t/exe4 $t/a.o -Wl,--bogus-option,--no-agent-mode 2> $t/e4
grep -F 'mold: fatal: unknown command line option: --bogus-option' $t/e4
not grep -F '{"schema"' $t/e4
# and with --agent-mode before it on the command line.
not $CC -B. -o $t/exe4 $t/a.o -Wl,--agent-mode,--bogus-option,--no-agent-mode 2> $t/e4b
not grep -F '{"schema"' $t/e4b
# The other order: the last one wins, so agent mode is on.
not $CC -B. -o $t/exe4 $t/a.o -Wl,--no-agent-mode,--bogus-option,--agent-mode 2> $t/e4c
grep -F '{"schema":1' $t/e4c

# A3: --why-removed without --gc-sections or --icf.
RUSTY_AGENT=1 $CC -B. -o $t/exe5 $t/a.o -Wl,--why-removed=dead_leaf > $t/o5 2> $t/e5
records $t/e5 | python3 -I -c '
import json, sys
r = json.loads(sys.stdin.readline())
assert r["severity"] == "warning" and "no removal pass" in r["message"], r
'
# With --gc-sections there is no warning and there is an answer.
RUSTY_AGENT=1 $CC -B. -o $t/exe5 $t/a.o -Wl,--gc-sections,--why-removed=dead_leaf > $t/o5 2> $t/e5
[ ! -s $t/e5 ]
grep -F '"decision":"removed"' $t/o5

# A5: --no-stable-diagnostics is accepted, also inside agent mode.
$CC -B. -o $t/exe6 $t/a.o -Wl,--no-stable-diagnostics
RUSTY_AGENT=1 $CC -B. -o $t/exe6 $t/a.o -Wl,--no-stable-diagnostics,--stable-diagnostics,--no-stable-diagnostics

# A5: the link summary is on standard error and on the channel, the same text
# as the file, for a good link and for a failed one.
cat <<EOF2 | $CC -c -o $t/bad.o -xc -
int nowhere(void);
int main() { return nowhere(); }
EOF2
rm -f $t/ch.jsonl
RUSTY_AGENT=1 RUSTY_AGENT_OUT=$t/ch.jsonl $CC -B. -o $t/exe7 $t/a.o -Wl,--link-summary=$t/s7.json 2> $t/e7
records $t/e7 | grep -F '"type":"link-summary"' | cmp - $t/s7.json
records $t/ch.jsonl | grep -F '"type":"link-summary"' | cmp - $t/s7.json
rm -f $t/ch.jsonl
RUSTY_AGENT=1 RUSTY_AGENT_OUT=$t/ch.jsonl not $CC -B. -o $t/exe8 $t/bad.o -Wl,--link-summary=$t/s8.json 2> $t/e8
records $t/e8 | grep -F '"type":"link-summary"' | cmp - $t/s8.json
records $t/ch.jsonl | grep -F '"type":"link-summary"' | cmp - $t/s8.json
# Every record on standard error is in the channel, in the same order.
records $t/e8 | cmp - <(records $t/ch.jsonl)
# Without agent mode, nothing about the summary goes to standard error.
$CC -B. -o $t/exe9 $t/a.o -Wl,--link-summary=$t/s9.json 2> $t/e9
[ ! -s $t/e9 ]
[ -s $t/s9.json ]
