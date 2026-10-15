#!/usr/bin/env bash
. $(dirname $0)/common.inc

cat <<EOF | $CC -fPIC -o $t/a.o -c -xc -
#include <stdio.h>
void hello() { printf("Hello"); }
void world() { printf("world"); }
EOF

$CC -B. -o $t/b.so -shared $t/a.o

cat <<EOF | $CC -fPIC -o $t/c.o -c -xc -
#include <stdio.h>
void hello();
void world();
void greet() {
  hello();
  printf(" ");
  world();
  printf("\n");
}
EOF

$CC -B. -o $t/d.so -shared $t/c.o $t/b.so -Wl,-z,mark-plt,-z,now

cat <<EOF | $CC -o $t/e.o -c -xc -
void greet();
int main() { greet(); }
EOF

$CC -B. -o $t/exe $t/e.o $t/d.so $t/b.so -Wl,-rpath,$t

# The tunable makes glibc 2.39 or later rewrite the PLT entries of d.so
# into direct jumps. Older versions ignore it.
GLIBC_TUNABLES=glibc.cpu.plt_rewrite=1 $QEMU $t/exe | grep 'Hello world'

readelf -dW $t/d.so > $t/log1
plt=$(awk '/DT_X86_64_PLT\)/ { print $NF }' $t/log1)
pltsz=$(awk '/DT_X86_64_PLTSZ\)/ { print $NF }' $t/log1)
grep -E 'DT_X86_64_PLTENT\)\s+0x10$' $t/log1

readelf -SW $t/d.so | grep -E " \.plt +PROGBITS +0*${plt#0x} "

# Each R_X86_64_JUMP_SLOT's addend points into a PLT entry.
readelf -rW $t/d.so | grep -c R_X86_64_JUMP_SLOT | grep -qx 3
readelf -rW $t/d.so | awk -v plt=$((plt)) -v end=$((plt + pltsz)) '
  /R_X86_64_JUMP_SLOT/ {
    addend = strtonum("0x" $NF)
    if (addend < plt + 32 || addend >= end || addend % 16 != 0 && addend % 16 != 4)
      exit 1
  }'

readelf -VW $t/d.so | grep -E 'Name: (GLIBC_ABI_DT_X86_64_PLT|GLIBC_2\.36) '

# The marker is not added by default.
$CC -B. -o $t/f.so -shared $t/c.o $t/b.so -Wl,-z,now
readelf -dW $t/f.so > $t/log2
not grep DT_X86_64_PLT $t/log2
readelf -rW $t/f.so | grep R_X86_64_JUMP_SLOT > $t/log3
not grep -v ' + 0$' $t/log3
