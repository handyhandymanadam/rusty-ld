#!/usr/bin/env bash
. $(dirname $0)/common.inc

# --explain-duplicates shows the source file and line of each definition of
# a duplicate symbol, read from the debug info of the object files.

cd $t
printf '\n\nint foo(int x) {\n  return x + 1;\n}\nint counter = 5;\n' > d1.c
printf 'int bar;\n\n\n\nint foo(int x) { return x; }\nint counter = 7;\nint main() { return 0; }\n' > d2.c
cd - > /dev/null

for opts in '-g' '-g -gdwarf-4 -ffunction-sections'; do
  $CC $opts -c -o $t/d1.o $t/d1.c
  $CC $opts -c -o $t/d2.o $t/d2.c

  not $CC -B. -o $t/exe $t/d1.o $t/d2.o -Wl,--explain-duplicates >& $t/log1
  grep -E 'd2\.o defines it at .*d2\.c:5$' $t/log1
  grep -E 'd1\.o defines it at .*d1\.c:3$' $t/log1
  grep -E 'd2\.o defines it at .*d2\.c:6$' $t/log1
  grep -E 'd1\.o defines it at .*d1\.c:6$' $t/log1

  # Off by default
  not $CC -B. -o $t/exe $t/d1.o $t/d2.o >& $t/log2
  not grep 'defines it at' $t/log2

  # In JSON
  not $CC -B. -o $t/exe $t/d1.o $t/d2.o -Wl,--agent-mode >& $t/log3
  grep -E '"locations":\[\{"object":"[^"]*d[12]\.o","file":"[^"]*d[12]\.c","line":[0-9]+\}' $t/log3
done

# Without debug info there is nothing to add, and no error from the search.
$CC -c -o $t/d1.o $t/d1.c
$CC -c -o $t/d2.o $t/d2.c
not $CC -B. -o $t/exe $t/d1.o $t/d2.o -Wl,--explain-duplicates >& $t/log4
grep -F 'duplicate symbol: ' $t/log4
not grep 'defines it at' $t/log4
