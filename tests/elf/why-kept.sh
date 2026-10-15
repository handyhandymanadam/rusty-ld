#!/usr/bin/env bash
. $(dirname $0)/common.inc

# --why-kept says why --gc-sections kept or removed a section and why
# --icf folded it or left it alone.

cat <<EOF2 | $CC -O0 -ffunction-sections -c -o $t/a.o -xc -
int used_leaf(int x) { return x + 100; }
int used_mid(int x) { return used_leaf(x) * 2; }
int dead_leaf(int x) { return x + 7; }
int dead_top(int x) { return dead_leaf(x) * 3; }
int twin_a(int x) { return x * 11 + 5; }
int twin_b(int x) { return x * 11 + 5; }
int main(int c, char **v) { return used_mid(c) + twin_a(c) + twin_b(c + 1); }
EOF2

$CC -B. -o $t/exe $t/a.o -Wl,--gc-sections,--icf=all \
  -Wl,--why-kept=used_leaf -Wl,--why-removed=dead_leaf \
  -Wl,--why-kept=twin_b -Wl,--why-kept=twin_a -Wl,--why-kept=no_such_name > $t/log1

# A kept section: the chain goes back to a root.
grep -F 'a.o:(.text.used_leaf): kept by --gc-sections' $t/log1
grep -F 'is referenced via `used_leaf` from ' $t/log1
grep -F 'a.o:(.text.used_mid) is referenced via `used_mid` from ' $t/log1
grep -F 'is a root: it defines `_start`' $t/log1

# A removed section
grep -F 'a.o:(.text.dead_leaf): removed by --gc-sections' $t/log1
grep -F 'referenced via `dead_leaf` from ' $t/log1

# ICF
grep -F 'a.o:(.text.twin_b): folded by --icf' $t/log1
grep -E 'folded into .*a\.o:\(\.text\.twin_a\)' $t/log1
grep -F 'a.o:(.text.twin_a): not-folded by --icf' $t/log1
grep -F 'folded into it: ' $t/log1

# An unknown name
grep -F 'no_such_name: unknown by --gc-sections' $t/log1

# JSON
$CC -B. -o $t/exe $t/a.o -Wl,--gc-sections -Wl,--why-kept=dead_leaf -Wl,--diagnostics-format=json > $t/log2
grep -E '^\{"schema":1,"type":"why","query":"dead_leaf","section":"[^"]*dead_leaf\)","by":"--gc-sections","decision":"removed","chain":\[' $t/log2

# Nothing is printed, and the output is the same, without the option.
$CC -B. -o $t/exe3 $t/a.o -Wl,--gc-sections,--icf=all > $t/log3
[ ! -s $t/log3 ]
$CC -B. -o $t/exe4 $t/a.o -Wl,--gc-sections,--icf=all -Wl,--why-kept=main > /dev/null
cmp $t/exe3 $t/exe4

# In JSON the removed section's own debug sections are counted, not listed;
# the text form lists them.
cat <<EOF2 | $CC -g -O0 -ffunction-sections -c -o $t/g.o -xc -
int dead_g(int x) { return x + 7; }
int main() { return 0; }
EOF2
$CC -B. -o $t/exe5 $t/g.o -Wl,--gc-sections,--why-removed=dead_g > $t/log5
grep -F 'referenced via `.text.dead_g` from ' $t/log5 | grep -F '.debug_'
$CC -B. -o $t/exe5 $t/g.o -Wl,--gc-sections,--why-removed=dead_g,--diagnostics-format=json > $t/log6
python3 -c '
import json, sys
r = json.loads(open(sys.argv[1]).readline())
assert r["decision"] == "removed", r
assert not any(".debug_" in l for l in r["chain"] if "referenced via" in l), r
assert any(l.endswith(" debug sections of the same object are also removed") for l in r["chain"]), r
' $t/log6
