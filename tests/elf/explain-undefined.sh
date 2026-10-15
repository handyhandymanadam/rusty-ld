#!/usr/bin/env bash
. $(dirname $0)/common.inc

# --explain-undefined names the library that defines a missing symbol, the
# -l option to add, and a near-miss spelling.

mkdir -p $t/lib

cat <<EOF2 | $CC -fPIC -shared -o $t/lib/libwidget.so -xc -
int widget_draw(int x) { return x + 1; }
EOF2

cat <<EOF2 | $CC -c -o $t/a.o -xc -
int widget_draw(int);
int helper_cout(void);
int main() { return widget_draw(1) + helper_cout(); }
EOF2

cat <<EOF2 | $CC -c -o $t/b.o -xc -
int helper_count(void) { return 1; }
EOF2

# Without the option, the message is the plain one.
not $CC -B. -o $t/exe $t/a.o $t/b.o >& $t/log0
not grep 'hint:' $t/log0

# With the option, the hints appear.
not $CC -B. -o $t/exe $t/a.o $t/b.o -L$t/lib -Wl,--explain-undefined >& $t/log1
grep -F 'undefined symbol: widget_draw' $t/log1
grep -F 'libwidget.so; add -lwidget' $t/log1
grep -F 'did you mean `helper_count`? (defined in ' $t/log1

# --agent-mode turns it on too.
not $CC -B. -o $t/exe $t/a.o $t/b.o -L$t/lib -Wl,--agent-mode >& $t/log2
grep -F '"option":"-lwidget"' $t/log2

# Adding the library fixes the link, so the hint is correct.
not $CC -B. -o $t/exe $t/a.o $t/b.o -L$t/lib -lwidget >& $t/log3
not grep widget_draw $t/log3
grep -F 'undefined symbol: helper_cout' $t/log3
