#!/usr/bin/env bash
. $(dirname $0)/common.inc

# --diagnostics-format=json prints each error as one JSON object per line.

cat <<EOF2 | $CC -c -o $t/a.o -xc -
int bar(void);
int main() { return bar(); }
EOF2

cat <<EOF2 | $CC -c -o $t/b.o -xc -
int foo(void) { return 1; }
EOF2

cat <<EOF2 | $CC -c -o $t/c.o -xc -
int foo(void) { return 2; }
EOF2

# Undefined symbol
not $CC -B. -o $t/exe $t/a.o -Wl,--diagnostics-format=json >& $t/log1
grep -E '^\{"schema":1,"type":"diagnostic","code":"L0001","severity":"error","message":"undefined symbol: bar","files":\["[^"]*a\.o"\],"symbols":\["bar"\]' $t/log1
# Every line of the linker's own output is a JSON object
grep '^{' $t/log1 | wc -l | grep -q '^1$'

# Duplicate symbol
not $CC -B. -o $t/exe $t/b.o $t/c.o -Wl,--diagnostics-format=json >& $t/log2
grep -E '"code":"L0002","severity":"error","message":"duplicate symbol: .*"files":\["[^"]*[bc]\.o","[^"]*[bc]\.o"\],"symbols":\["foo"\]' $t/log2

# A fatal error
not $CC -B. -o $t/exe $t/a.o -lno_such_library_xyz -Wl,--diagnostics-format=json >& $t/log3
grep -F '"code":"L0003","severity":"fatal","message":"library not found: no_such_library_xyz"' $t/log3

# Bad argument
not $CC -B. -o $t/exe $t/a.o -Wl,--diagnostics-format=xml >& $t/log4
grep -F 'unknown --diagnostics-format argument: xml' $t/log4

# The plain text is the same with and without --diagnostics-format=text.
not $CC -B. -o $t/exe $t/a.o >& $t/log5
not $CC -B. -o $t/exe $t/a.o -Wl,--diagnostics-format=text >& $t/log6
diff <(grep -v "^+" $t/log5) <(grep -v "^+" $t/log6)
not grep '"code"' $t/log5

# The JSON parses, if python3 is around.
if command -v python3 > /dev/null; then
  grep '^{' $t/log1 $t/log2 $t/log3 | sed 's/^[^:]*://' |
    python3 -c 'import json,sys; [json.loads(l) for l in sys.stdin]'
fi

# RUSTY_AGENT=1 and --agent-mode both switch JSON on; an explicit
# --diagnostics-format=text wins over them.
RUSTY_AGENT=1 not $CC -B. -o $t/exe $t/a.o >& $t/log7
grep -F '"schema":1,"type":"diagnostic","code":"L0001"' $t/log7
not $CC -B. -o $t/exe $t/a.o -Wl,--agent-mode >& $t/log8
grep -F '"schema":1,"type":"diagnostic","code":"L0001"' $t/log8
RUSTY_AGENT=1 not $CC -B. -o $t/exe $t/a.o -Wl,--diagnostics-format=text >& $t/log9
grep -F 'mold: error: undefined symbol: bar' $t/log9
# Any other value leaves the linker alone.
RUSTY_AGENT=0 not $CC -B. -o $t/exe $t/a.o >& $t/log10
not grep '"code"' $t/log10

# --version names the mode only when it is on.
./mold --version > $t/v0
not grep 'agent mode' $t/v0
RUSTY_AGENT=1 ./mold --version > $t/v1
grep -Fx 'agent mode: yes, schema 1' $t/v1
./mold --agent-mode --version | grep -Fx 'agent mode: yes, schema 1'
