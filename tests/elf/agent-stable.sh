#!/usr/bin/env bash
. $(dirname $0)/common.inc

# Stable mode in agent mode: no elapsed time, and the random part of a
# temporary file name is written XXXXXX, so two runs of one link print the
# same bytes.

mkdir -p $t/tmpdir
echo 'int main() { return 0; }' | $CC -c -o $t/ok.o -xc -
echo 'int missing_x(void); int main() { return missing_x(); }' | $CC -g -c -o $t/bad.o -xc -
cp $t/bad.o $t/tmpdir/ccAb12Zq.o
cp $t/bad.o $t/tmpdir/x-a1b2c3.o

RUSTY_AGENT=1 TMPDIR=$t/tmpdir not $CC -B. -o $t/exe $t/tmpdir/ccAb12Zq.o $t/tmpdir/x-a1b2c3.o \
  -Wl,--echo-command-line,--link-summary=$t/s.json > $t/out 2> $t/err
grep -F 'ccXXXXXX.o' $t/err
grep -F 'x-XXXXXX.o' $t/err
not grep -F 'ccAb12Zq' <(grep -v '^+ ' $t/err)
grep -F 'ccXXXXXX.o' $t/out
not grep -F 'elapsed_seconds' $t/s.json
