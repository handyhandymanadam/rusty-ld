#!/usr/bin/env bash
. $(dirname $0)/common.inc

test_cflags -static || skip
supports_ifunc || skip

# The resolver calls strlen, which the C library defines as an IFUNC too: the
# library's IRELATIVE relocations must be applied before the program's, as a
# dynamic loader relocates a program's dependencies before the program.
cat <<EOF | $CC -o $t/a.o -c -xc -fno-builtin -
#include <stdio.h>
#include <string.h>

static int one(void) { return 1; }
static int two(void) { return 2; }
static void *resolve_foo(void) { return strlen("abc") == 3 ? (void *)two : (void *)one; }
int foo(void) __attribute__((ifunc("resolve_foo")));

int main() {
  printf("foo=%d\n", foo());
  return 0;
}
EOF

$CC -B. -o $t/exe $t/a.o -static
$QEMU $t/exe | grep 'foo=2'
