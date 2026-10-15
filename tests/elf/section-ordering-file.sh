#!/usr/bin/env bash
. $(dirname $0)/common.inc

cat <<EOF | $CC -o $t/a.o -c -xc -ffunction-sections -fdata-sections -
#include <stdio.h>
void fn1() {}
void fn2() {}
void fn3() {}
void fn4() {}
int var1 = 1;
int var2 = 2;
int var3 = 3;
int main() { printf("Hello world\n"); }
EOF

cat <<EOF | $CC -o $t/b.o -c -xc -ffunction-sections -fdata-sections -
void fn5() {}
void fn6() {}
EOF

cat <<EOF | $CC -o $t/c.o -c -xc -ffunction-sections -fdata-sections -
void fn7() {}
EOF

rm -f $t/d.a
ar crs $t/d.a $t/c.o

# Sections that match one statement keep their input order, and the
# first statement that matches a section takes it.
cat <<EOF > $t/order1
/* comment */
.text : {
  *(.text.fn3)
  *(.text.fn4 .text.fn1)
  *(.text.fn3 .text.fn6)
}
.data : { *(.data.var3) ; *(.data.var2) }
EOF

$CC -B. -o $t/exe1 $t/a.o $t/b.o -Wl,--section-ordering-file=$t/order1
$QEMU $t/exe1 | grep 'Hello world'

nm -n $t/exe1 | awk '$3 ~ /^(fn[0-9]|var[0-9])$/ { print $3 }' | tr '\n' ' ' > $t/log1
grep -Fx 'fn3 fn1 fn4 fn6 fn2 fn5 var3 var2 var1 ' $t/log1

# File patterns
cat <<EOF > $t/order2
.text : {
  *b.o(.text.*)
  *d.a:c.o(.text.fn7)
  $t/a.o(.text.fn4)
}
EOF

$CC -B. -o $t/exe2 $t/a.o $t/b.o -Wl,--whole-archive $t/d.a -Wl,--no-whole-archive \
  -Wl,--section-ordering-file=$t/order2
$QEMU $t/exe2 | grep 'Hello world'

nm -n $t/exe2 | awk '$3 ~ /^fn[0-9]$/ { print $3 }' | tr '\n' ' ' > $t/log2
grep -Fx 'fn5 fn6 fn7 fn4 fn1 fn2 fn3 ' $t/log2

# `:c.o` matches only a c.o outside archives
echo '.text : { :c.o(.text.fn7) *(.text.fn6) }' > $t/order3

$CC -B. -o $t/exe3 $t/a.o $t/b.o -Wl,--whole-archive $t/d.a -Wl,--no-whole-archive \
  -Wl,--section-ordering-file=$t/order3
nm -n $t/exe3 | awk '$3 ~ /^fn[0-9]$/ { print $3 }' | tr '\n' ' ' > $t/log3
grep -Fx 'fn6 fn1 fn2 fn3 fn4 fn5 fn7 ' $t/log3

# Linker script commands other than input section descriptions are errors
echo '.text : { KEEP(*(.text.fn1)) }' > $t/order4
not $CC -B. -o $t/exe4 $t/a.o -Wl,--section-ordering-file=$t/order4 |&
  grep 'not supported in a section ordering file'
