#!/usr/bin/env bash
. $(dirname $0)/common.inc

# --link-summary=FILE writes a JSON summary of the link, also for a link
# that fails.

cat <<EOF2 | $CC -c -o $t/a.o -xc -
int main() { return 0; }
EOF2

cat <<EOF2 | $CC -c -o $t/b.o -xc -
int main() { return 1; }
EOF2

rm -f $t/ok.json $t/fail.json
$CC -B. -o $t/exe $t/a.o -Wl,--link-summary=$t/ok.json
grep -E '^\{"schema":1,"type":"link-summary","status":"ok","output":"[^"]*exe","output_size":[0-9]+,"target":"[^"]+","elapsed_seconds":[0-9.]+,"inputs":\{"objects":[0-9]+,"objects_from_archives":0,"shared_libraries":[0-9]+\},"output_sections":[0-9]+,"errors":0,"warnings":0\}$' $t/ok.json

not $CC -B. -o $t/exe2 $t/a.o $t/b.o -Wl,--link-summary=$t/fail.json 2> /dev/null
grep -E '^\{"schema":1,"type":"link-summary","status":"failed","elapsed_seconds":[0-9.]+,"errors":1,"warnings":0\}$' $t/fail.json

if command -v python3 > /dev/null; then
  python3 -c '
import json, os, sys
d = json.load(open(sys.argv[1]))
assert d["output_size"] == os.path.getsize(sys.argv[2])
' $t/ok.json $t/exe
fi

# The output file is the same with and without the option.
$CC -B. -o $t/exe3 $t/a.o
cmp $t/exe $t/exe3
