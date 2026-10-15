#!/usr/bin/env bash
. $(dirname $0)/common.inc

# --stable-diagnostics makes the messages of one link identical on every run
# and sorted; the output file does not depend on the number of threads.

for i in 1 2 3 4 5 6 7 8; do
  echo "extern int missing_sym; int f$i() { return missing_sym + $i; }" |
    $CC -c -o $t/u$i.o -xc -
  echo "int dup$i = 1;" | $CC -c -o $t/p$i.o -xc -
  echo "int dup$i = 2;" | $CC -c -o $t/q$i.o -xc -
done
echo 'int main() { return 0; }' | $CC -c -o $t/main.o -xc -

for i in $(seq 12); do
  not $CC -B. -o $t/exe $t/main.o $t/u?.o -Wl,--stable-diagnostics >& $t/u$i.log
  not $CC -B. -o $t/exe $t/main.o $t/p?.o $t/q?.o -Wl,--stable-diagnostics >& $t/d$i.log
done
for i in $(seq 2 12); do
  diff <(grep -v '^+' $t/u1.log) <(grep -v '^+' $t/u$i.log)
  diff <(grep -v '^+' $t/d1.log) <(grep -v '^+' $t/d$i.log)
done

# The three references named are the first three in file order.
grep -A8 'undefined symbol: missing_sym' $t/u1.log | grep -E '^>>> +[^ ]*u[0-9]\.o' > $t/refs
[ "$(wc -l < $t/refs)" = 3 ]
sort -c $t/refs

# Duplicate symbols come out sorted by symbol name.
grep -o 'dup[0-9]$' $t/d1.log > $t/names
[ "$(wc -l < $t/names)" = 8 ]
sort -c $t/names

# Output does not depend on the number of threads.
$CC -B. -o $t/e1 $t/main.o -Wl,--threads=1
$CC -B. -o $t/e2 $t/main.o -Wl,--threads=8
cmp $t/e1 $t/e2
