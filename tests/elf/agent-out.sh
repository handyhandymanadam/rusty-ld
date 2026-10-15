#!/usr/bin/env bash
. $(dirname $0)/common.inc

# In agent mode RUSTY_AGENT_OUT=<path> gets a copy of every JSON record: the
# diagnostics on standard error, the answers on standard output, the link
# summary and the size report. A bad path is one warning (L0012); with agent
# mode off the variable is not read at all.

cat <<EOF2 | $CC -O0 -ffunction-sections -c -o $t/a.o -xc -
int used_leaf(int x) { return x + 100; }
int dead_leaf(int x) { return x + 7; }
int main(int c, char **v) { return used_leaf(c); }
EOF2

cat <<EOF2 | $CC -c -o $t/bad.o -xc -
int missing_one(void);
int missing_two(void);
int main() { return missing_one() + missing_two(); }
EOF2

# Reads the JSON lines of the given files and prints them one per line.
records() {
  python3 - "$@" <<'EOF2'
import json, sys
for path in sys.argv[1:]:
    for line in open(path):
        if line.startswith('{'):
            json.loads(line)          # every record must be valid JSON
            print(line.rstrip('\n'))
EOF2
}

# A failing link: the file holds the diagnostics in the order of standard
# error, then the command-line record and the failed summary.
rm -f $t/out1.jsonl
RUSTY_AGENT=1 RUSTY_AGENT_OUT=$t/out1.jsonl not $CC -B. -o $t/exe1 $t/bad.o \
  -Wl,--echo-command-line,--link-summary=$t/sum1.json > $t/o1 2> $t/e1
records $t/e1 > $t/e1.rec
records $t/o1 > $t/o1.rec
records $t/out1.jsonl > $t/f1.rec
[ "$(grep -c '"type":"diagnostic"' $t/e1.rec)" -ge 2 ]
grep -F '"code":"L0001"' $t/e1.rec
grep -F '"type":"command-line"' $t/o1.rec
# The summary is on standard error too (not only in its file), and it is the same text.
grep -F '"status":"failed"' $t/e1.rec
grep '"type":"link-summary"' $t/e1.rec | cmp - $t/sum1.json
# The diagnostics are the same lines in the same order.
grep -E '"type":"(diagnostic|link-summary)"' $t/f1.rec | cmp - $t/e1.rec
# Everything on either stream is in the file, and nothing else.
sort $t/e1.rec $t/o1.rec | cmp - <(sort $t/f1.rec)

# A working link with every kind of record. The runs append.
rm -f $t/out2.jsonl
for i in 1 2; do
  RUSTY_AGENT=1 RUSTY_AGENT_OUT=$t/out2.jsonl $CC -B. -o $t/exe2 $t/a.o \
    -Wl,--gc-sections,--echo-command-line,--why-kept=used_leaf,--why-removed=dead_leaf \
    -Wl,--size-report,--link-summary=$t/sum2.$i.json > $t/o2.$i 2> $t/e2.$i
  records $t/o2.$i $t/e2.$i > $t/o2.$i.rec
done
for ty in command-line why size-report link-summary; do
  grep -F "\"type\":\"$ty\"" $t/o2.1.rec
done
[ "$(grep -c '"type":"why"' $t/o2.1.rec)" -eq 2 ]
records $t/out2.jsonl > $t/f2.rec
n=$(wc -l < $t/o2.1.rec)
[ "$(wc -l < $t/f2.rec)" -eq $((n + $(wc -l < $t/o2.2.rec))) ]
# Each run's records, in any order between the streams, are in the file
# in turn, with the same text.
head -n $n $t/f2.rec | sort | cmp - <(sort $t/o2.1.rec)
tail -n +$((n + 1)) $t/f2.rec | sort | cmp - <(sort $t/o2.2.rec)

# --agent-mode on the command line works like the variable.
rm -f $t/out3.jsonl
RUSTY_AGENT_OUT=$t/out3.jsonl $CC -B. -o $t/exe3 $t/a.o -Wl,--agent-mode,--echo-command-line > $t/o3
grep -F '"type":"command-line"' $t/out3.jsonl

# A path that cannot be opened: one warning L0012, the link works, nothing
# is created and the summary counts the warning.
RUSTY_AGENT=1 RUSTY_AGENT_OUT=$t/no/such/dir/x.jsonl $CC -B. -o $t/exe4 $t/a.o \
  -Wl,--link-summary=$t/sum4.json > $t/o4 2> $t/e4
records $t/e4 > $t/e4.rec
[ "$(grep -c '"type":"diagnostic"' $t/e4.rec)" -eq 1 ]
python3 -I - $t/e4.rec $t/sum4.json <<'EOF2'
import json, sys
r = json.loads(open(sys.argv[1]).readline())
assert r['code'] == 'L0012' and r['severity'] == 'warning', r
assert 'RUSTY_AGENT_OUT' in r['message'], r
assert r['suggestions'][0]['message'] == 'check the path or unset RUSTY_AGENT_OUT', r
s = json.loads(open(sys.argv[2]).read())
assert s['status'] == 'ok' and s['warnings'] == 1 and s.get('errors', 0) == 0, s
EOF2
[ ! -e $t/no ]
$t/exe4 || true

# An empty path is the same warning.
RUSTY_AGENT=1 RUSTY_AGENT_OUT= $CC -B. -o $t/exe5 $t/a.o 2> $t/e5
records $t/e5 | grep -F '"code":"L0012"'
[ "$(records $t/e5 | wc -l)" -eq 1 ]

# A path that is a folder is also only a warning.
RUSTY_AGENT=1 RUSTY_AGENT_OUT=$t $CC -B. -o $t/exe6 $t/a.o 2> $t/e6
records $t/e6 | grep -F '"code":"L0012"'

# Agent mode off: the variable is ignored (not even opened), also with
# --no-agent-mode after RUSTY_AGENT=1.
rm -f $t/off1.jsonl $t/off2.jsonl
RUSTY_AGENT_OUT=$t/off1.jsonl $CC -B. -o $t/exe7 $t/a.o \
  -Wl,--diagnostics-format=json,--echo-command-line > $t/o7 2> $t/e7
grep -F '"type":"command-line"' $t/o7
[ ! -e $t/off1.jsonl ]
RUSTY_AGENT_OUT=$t/no/such/x.jsonl $CC -B. -o $t/exe8 $t/a.o 2> $t/e8
[ ! -s $t/e8 ]
RUSTY_AGENT=1 RUSTY_AGENT_OUT=$t/off2.jsonl $CC -B. -o $t/exe9 $t/a.o \
  -Wl,--no-agent-mode,--echo-command-line > $t/o9 2> $t/e9
[ ! -e $t/off2.jsonl ]
[ ! -s $t/e9 ]
grep -F 'command line: ' $t/o9
