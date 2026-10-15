#!/usr/bin/env bash
. $(dirname $0)/common.inc

# Two versions of one table at one address, the newer one larger (as glibc's
# sys_errlist@GLIBC_2.2.5 and sys_errlist@GLIBC_2.12). A program that takes
# both by copy relocation must see all entries of the larger one.
cat <<EOF | $CC -fPIC -c -o $t/a.o -xc -
int table_v2[6] = { 1, 2, 3, 4, 5, 6 };
__asm__(".globl table_v1\\n.set table_v1, table_v2\\n.type table_v1, @object\\n.size table_v1, 16");
__asm__(".symver table_v1, table@V1");
__asm__(".symver table_v2, table@@V2");
EOF

cat <<EOF > $t/ver
V1 { global: table_v1; };
V2 { global: table_v2; } V1;
EOF

$CC -B. -shared -o $t/b.so $t/a.o -Wl,--version-script=$t/ver

cat <<EOF | $CC -fno-PIC -c -o $t/c.o -xc -
#include <stdio.h>
extern int old_table[4];
extern int new_table[6];
__asm__(".symver old_table, table@V1");
__asm__(".symver new_table, table@V2");
int main() {
  printf("%d %d\\n", old_table[3], new_table[5]);
}
EOF

$CC -B. -no-pie -o $t/exe $t/c.o $t/b.so -Wl,-rpath,$t
$QEMU $t/exe | grep '^4 6$'

# one copy for both versions
[ "$(readelf -rW $t/exe | grep -c R_.*_COPY)" = 1 ]
[ "$(readelf -W --dyn-syms $t/exe | awk '/ table@V[12] / { print $2 }' | sort -u | wc -l)" = 1 ]
