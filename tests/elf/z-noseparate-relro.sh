#!/usr/bin/env bash
. $(dirname $0)/common.inc

cat <<EOF | $CC -fPIC -c -o $t/a.o -xc -
#include <stdio.h>
const char *const names[] = { "one", "two", "three" };
int counter = 5;
char buffer[100];
void count(void) {
  counter++;
  snprintf(buffer, sizeof buffer, "%s %s %s %d", names[0], names[1], names[2], counter);
}
EOF

cat <<EOF | $CC -c -o $t/b.o -xc -
#include <stdio.h>
void count(void);
extern char buffer[];
int main() {
  count();
  printf("%s\n", buffer);
}
EOF

# The default: RELRO ends with .relro_padding, and the other writable data
# starts a segment of its own.
$CC -B. -shared -o $t/c.so $t/a.o -Wl,-z,relro,-z,now
readelf -S $t/c.so | grep '\.relro_padding'
[ "$(readelf -lW $t/c.so | grep -E '^ +LOAD .* RW ' | wc -l)" = 2 ]

# -z noseparate-relro: one RW segment, no .relro_padding, and the end of
# PT_GNU_RELRO is a page boundary at or below the start of the other data.
$CC -B. -shared -o $t/d.so $t/a.o -Wl,-z,relro,-z,now,-z,noseparate-relro
readelf -S $t/d.so | not grep '\.relro_padding'
[ "$(readelf -lW $t/d.so | grep -E '^ +LOAD .* RW ' | wc -l)" = 1 ]

relro=$(readelf -lW $t/d.so | awk '$1 == "GNU_RELRO" { print $3, $6 }')
end=$(( $(echo $relro | cut -d' ' -f1) + $(echo $relro | cut -d' ' -f2) ))
[ $((end % 4096)) = 0 ]
data=$(readelf -SW $t/d.so | awk '$2 == ".data" { print "0x" $4 }')
[ $end -le $((data)) ]

$CC -B. -o $t/exe $t/b.o $t/d.so -Wl,-z,noseparate-relro -Wl,-rpath,$t
$QEMU $t/exe | grep 'one two three 6'

# -z separate-relro restores the default
$CC -B. -shared -o $t/e.so $t/a.o -Wl,-z,relro,-z,now,-z,noseparate-relro,-z,separate-relro
readelf -S $t/e.so | grep '\.relro_padding'
