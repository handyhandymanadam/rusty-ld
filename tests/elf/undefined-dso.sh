#!/usr/bin/env bash
. $(dirname $0)/common.inc

cat <<EOF | $CC -fPIC -o $t/a.o -c -xc -
void foo() {}
void bar() {}
EOF

echo 'VER1 { foo; }; VER2 { bar; };' > $t/a.ver
$CC -B. -o $t/a.so -shared $t/a.o -Wl,--version-script=$t/a.ver

cat <<EOF | $CC -fPIC -o $t/b.o -c -xc -
void baz() {}
EOF

# A symbol named by -u or --require-defined that a DSO defines is
# imported, as if an object file referred to it.
$CC -B. -o $t/c.so -shared $t/b.o $t/a.so -Wl,-u,foo -Wl,--require-defined,bar

readelf -W --dyn-syms $t/c.so > $t/log
grep -E ' GLOBAL .* UND foo@VER1' $t/log
grep -E ' GLOBAL .* UND bar@VER2' $t/log
readelf -W --dynamic $t/c.so | grep -F 'a.so]'
readelf -W -V $t/c.so > $t/log2
grep -F 'Name: VER1' $t/log2
grep -F 'Name: VER2' $t/log2

$CC -B. -o $t/d.so -shared $t/b.o $t/a.so
readelf -W --dyn-syms $t/d.so > $t/log3
not grep foo $t/log3

# As in GNU ld, -u alone doesn't make an --as-needed DSO needed.
$CC -B. -o $t/e.so -shared $t/b.o -Wl,--as-needed $t/a.so -Wl,-u,foo
readelf -W --dyn-syms $t/e.so > $t/log4
not grep foo $t/log4
readelf -W --dynamic $t/e.so > $t/log5
not grep -F a.so $t/log5
