#!/usr/bin/env bash
. $(dirname $0)/common.inc

# In agent mode RUSTY_AGENT_FD=<n> writes every JSON record to the open
# file descriptor n (3 or more), which the linker neither opens nor closes.
# A descriptor that cannot be used is one warning (L0013).

cat <<EOF2 | $CC -O0 -ffunction-sections -c -o $t/a.o -xc -
int used_leaf(int x) { return x + 100; }
int dead_leaf(int x) { return x + 7; }
int main(int c, char **v) { return used_leaf(c); }
EOF2

cat <<EOF2 | $CC -c -o $t/bad.o -xc -
int missing_one(void);
int main() { return missing_one(); }
EOF2

records() {
  python3 - "$@" <<'EOF2'
import json, sys
for path in sys.argv[1:]:
    for line in open(path):
        if line.startswith('{'):
            json.loads(line)
            print(line.rstrip('\n'))
EOF2
}

# A descriptor the shell opened (here 7, to show that 3 is not special):
# the records are what standard error carries.
RUSTY_AGENT=1 RUSTY_AGENT_FD=7 not $CC -B. -o $t/exe1 $t/bad.o \
  -Wl,--echo-command-line,--link-summary=$t/sum1.json 7> $t/fd1.jsonl > $t/o1 2> $t/e1
records $t/e1 > $t/e1.rec
records $t/o1 > $t/o1.rec
records $t/fd1.jsonl > $t/f1.rec
grep -F '"code":"L0001"' $t/f1.rec
grep -E '"type":"(diagnostic|link-summary)"' $t/f1.rec | cmp - $t/e1.rec
sort $t/e1.rec $t/o1.rec | cmp - <(sort $t/f1.rec)

# A working link with the answers; the descriptor is 3 and a pipe.
RUSTY_AGENT=1 RUSTY_AGENT_FD=3 $CC -B. -o $t/exe2 $t/a.o \
  -Wl,--gc-sections,--echo-command-line,--why-kept=used_leaf,--why-removed=dead_leaf \
  -Wl,--size-report,--link-summary=$t/sum2.json 3> >(cat > $t/fd2.jsonl) > $t/o2 2> $t/e2
sleep 1
records $t/o2 $t/e2 | sort | cmp - <(records $t/fd2.jsonl | sort)
for ty in command-line why size-report link-summary; do
  grep -F "\"type\":\"$ty\"" $t/fd2.jsonl
done

# The linker leaves the descriptor open: the shell's own write after the
# link works and lands behind the records (the descriptor is in append
# mode, as the linker's own handle on it is).
rm -f $t/fd3.jsonl
{
  RUSTY_AGENT=1 RUSTY_AGENT_FD=5 $CC -B. -o $t/exe3 $t/a.o -Wl,--echo-command-line
  echo 'after the link' >&5
} 5>> $t/fd3.jsonl > /dev/null
[ "$(tail -n 1 $t/fd3.jsonl)" = 'after the link' ]
grep -F '"type":"command-line"' $t/fd3.jsonl

# Both channels get the records.
rm -f $t/both.jsonl
RUSTY_AGENT=1 RUSTY_AGENT_OUT=$t/both.jsonl RUSTY_AGENT_FD=4 $CC -B. -o $t/exe4 $t/a.o \
  -Wl,--echo-command-line 4> $t/fd4.jsonl > /dev/null
cmp $t/both.jsonl $t/fd4.jsonl
[ -s $t/fd4.jsonl ]

# Bad descriptors: one warning L0013 each, the link works.
check_bad() {
  local name=$1; shift
  "$@" > /dev/null 2> $t/eb
  records $t/eb > $t/eb.rec
  [ "$(wc -l < $t/eb.rec)" -eq 1 ]
  python3 - $t/eb.rec "$name" <<'EOF2'
import json, sys
r = json.loads(open(sys.argv[1]).read())
assert r['code'] == 'L0013' and r['severity'] == 'warning', r
assert 'RUSTY_AGENT_FD' in r['message'], r
assert r['suggestions'], r
EOF2
}
export RUSTY_AGENT=1
RUSTY_AGENT_FD=abc check_bad letters $CC -B. -o $t/exe5 $t/a.o
RUSTY_AGENT_FD= check_bad empty $CC -B. -o $t/exe5 $t/a.o
RUSTY_AGENT_FD=-4 check_bad negative $CC -B. -o $t/exe5 $t/a.o
RUSTY_AGENT_FD=2 check_bad stderr $CC -B. -o $t/exe5 $t/a.o
RUSTY_AGENT_FD=1 check_bad stdout $CC -B. -o $t/exe5 $t/a.o
RUSTY_AGENT_FD=99999999999999999999 check_bad huge $CC -B. -o $t/exe5 $t/a.o
RUSTY_AGENT_FD=57 check_bad closed $CC -B. -o $t/exe5 $t/a.o
RUSTY_AGENT_FD=6 check_bad readonly $CC -B. -o $t/exe5 $t/a.o 6< $t/a.o
unset RUSTY_AGENT

# The summary counts the warning, and the exit status is the link's own.
RUSTY_AGENT=1 RUSTY_AGENT_FD=57 $CC -B. -o $t/exe6 $t/a.o -Wl,--link-summary=$t/sum6.json 2> /dev/null
python3 - $t/sum6.json <<'EOF2'
import json, sys
s = json.loads(open(sys.argv[1]).read())
assert s['status'] == 'ok' and s['warnings'] == 1, s
EOF2
RUSTY_AGENT=1 RUSTY_AGENT_FD=57 not $CC -B. -o $t/exe7 $t/bad.o 2> /dev/null

# Agent mode off: the variable is ignored.
RUSTY_AGENT_FD=8 $CC -B. -o $t/exe8 $t/a.o -Wl,--diagnostics-format=json,--echo-command-line \
  8> $t/off.jsonl > /dev/null 2> $t/e8
[ ! -s $t/off.jsonl ]
[ ! -s $t/e8 ]
RUSTY_AGENT_FD=57 $CC -B. -o $t/exe9 $t/a.o 2> $t/e9
[ ! -s $t/e9 ]
RUSTY_AGENT=1 RUSTY_AGENT_FD=8 $CC -B. -o $t/exe10 $t/a.o -Wl,--no-agent-mode,--echo-command-line \
  8> $t/off2.jsonl > /dev/null 2> $t/e10
[ ! -s $t/off2.jsonl ]
